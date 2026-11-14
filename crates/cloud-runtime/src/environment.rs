//! Task-scoped environment preparation. Resume reuses the admitted recipe and receipts.
use crate::identity::atomic_write;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt};
use zuse_protocol::cloud_computers::{EnvironmentRecipe, EnvironmentSource};

#[cfg(target_os = "linux")]
mod services;
#[cfg(target_os = "linux")]
pub use services::{
    Services, start_services, start_services_with_environment, start_services_with_protection,
};

/// Presentation-only protection; never injected into commands or environment.
#[derive(Clone, Default)]
pub struct OutputProtection {
    #[cfg(feature = "engine")]
    registry: Option<std::sync::Arc<zuse_providers::credentials::redaction::Secrets>>,
}
impl OutputProtection {
    #[cfg(feature = "engine")]
    pub fn new(registry: std::sync::Arc<zuse_providers::credentials::redaction::Secrets>) -> Self {
        Self {
            registry: Some(registry),
        }
    }
    fn mask(&self, bytes: &[u8]) -> Vec<u8> {
        #[cfg(feature = "engine")]
        if let Some(registry) = &self.registry {
            return registry.mask(&String::from_utf8_lossy(bytes)).into_bytes();
        }
        bytes.to_vec()
    }
    fn stream(&self) -> OutputStream {
        OutputStream {
            #[cfg(feature = "engine")]
            filter: self
                .registry
                .clone()
                .map(zuse_providers::credentials::redaction::Text::new),
        }
    }
}
struct OutputStream {
    #[cfg(feature = "engine")]
    filter: Option<zuse_providers::credentials::redaction::Text>,
}
impl OutputStream {
    fn push(&mut self, bytes: &[u8], finished: bool) -> Vec<u8> {
        #[cfg(feature = "engine")]
        if let Some(filter) = &mut self.filter {
            let mut clean = filter.push(bytes);
            if finished {
                clean.extend(filter.finish());
            }
            return clean;
        }
        let _ = finished;
        bytes.to_vec()
    }
}

pub type PrivateEnvironment = std::collections::BTreeMap<String, zeroize::Zeroizing<String>>;
/// Carry a possible suffix across pipe chunks, so a split secret is never emitted.
struct Redactor<'a> {
    pending: Vec<u8>,
    secrets: &'a PrivateEnvironment,
}
impl Drop for Redactor<'_> {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.pending.zeroize();
    }
}
impl<'a> Redactor<'a> {
    fn new(secrets: &'a PrivateEnvironment) -> Self {
        Self {
            pending: Vec::new(),
            secrets,
        }
    }
    fn push(&mut self, input: &[u8], finished: bool) -> Vec<u8> {
        self.pending.extend_from_slice(input);
        let mut result = Vec::new();
        let mut at = 0;
        while at < self.pending.len() {
            let rest = &self.pending[at..];
            if let Some(secret) = self
                .secrets
                .values()
                .filter(|s| !s.is_empty())
                .filter(|s| rest.starts_with(s.as_bytes()))
                .max_by_key(|s| s.len())
            {
                result.extend_from_slice(b"[redacted]");
                at += secret.len();
            } else if !finished
                && self
                    .secrets
                    .values()
                    .any(|s| !s.is_empty() && s.as_bytes().starts_with(rest))
            {
                break;
            } else {
                result.push(self.pending[at]);
                at += 1;
            }
        }
        use zeroize::Zeroize;
        self.pending[..at].zeroize();
        self.pending.drain(..at);
        result
    }
}
fn redact_output(input: &[u8], secrets: &PrivateEnvironment) -> Vec<u8> {
    let mut redactor = Redactor::new(secrets);
    let mut result = redactor.push(input, false);
    if !redactor.pending.is_empty() {
        result.extend_from_slice(b"[redacted]");
    }
    result
}

const MAX_RECIPE: u64 = 256 * 1024;
const MAX_LOG: usize = 32 * 1024;

/// Private setup output is explicitly requested by the account owner. The guest
/// strips its current platform credentials and recipe environment values first.
/// Arbitrary secrets printed by a user command cannot be inferred reliably.
pub fn preparation_log(
    data: &Path,
    secrets: &[&str],
) -> Result<Option<zuse_protocol::cloud_computers::PreparationLog>> {
    let file = data.join("environment-initialize.json");
    let metadata = match std::fs::symlink_metadata(&file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_file() && metadata.len() <= 512 * 1024,
        "environment_log_invalid"
    );
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(file)?
        .take(512 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 512 * 1024, "environment_log_invalid");
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let mut protected: Vec<String> = secrets
        .iter()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    let receipt = data.join("environment-ready.json");
    if let Ok(metadata) = std::fs::symlink_metadata(&receipt) {
        ensure!(
            metadata.is_file() && metadata.len() <= 2 * 1024 * 1024,
            "environment_receipt_invalid"
        );
        let mut bytes = Vec::new();
        std::fs::File::open(receipt)?
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= 2 * 1024 * 1024,
            "environment_receipt_invalid"
        );
        let receipt: PreparedEnvironment = serde_json::from_slice(&bytes)?;
        protected.extend(
            receipt
                .recipe
                .environment
                .values()
                .filter(|s| !s.is_empty())
                .cloned(),
        );
    }
    protected.sort_unstable_by_key(|s| std::cmp::Reverse(s.len()));
    protected.dedup();
    let clean = |text: &str| {
        let mut text = text
            .chars()
            .filter(|ch| !ch.is_control() || matches!(*ch, '\n' | '\t' | '\r'))
            .collect::<String>();
        for secret in &protected {
            text = text.replace(secret, "[redacted]");
        }
        let limit = zuse_protocol::cloud_computers::MAX_PREPARATION_LOG_BYTES;
        let truncated = text.len() > limit;
        let mut end = limit.min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        (text, truncated)
    };
    let stdout = value["stdout"].as_str().unwrap_or_default();
    let stderr = value["stderr"].as_str().unwrap_or_default();
    let (out, out_truncated) = clean(stdout);
    let (err, err_truncated) = clean(stderr);
    Ok(Some(zuse_protocol::cloud_computers::PreparationLog {
        stdout: out,
        stderr: err,
        success: value["success"].as_bool().unwrap_or(false),
        truncated: out_truncated
            || err_truncated
            || stdout.len() >= MAX_LOG
            || stderr.len() >= MAX_LOG,
    }))
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedEnvironment {
    version: u32,
    workspace: String,
    incarnation: String,
    checkout: PathBuf,
    pub source: EnvironmentSource,
    pub recipe: EnvironmentRecipe,
    pub digest: String,
    state: String,
}
/// Install a task-bound environment receipt after an exact verified cache overlay.
/// Startup services and runtime secret delivery remain separate fresh operations.
pub(crate) fn adopt_prepared_cache(
    data: &Path,
    checkout: &Path,
    workspace: &str,
    incarnation: &str,
    configured: &EnvironmentRecipe,
    expected_digest: &str,
) -> Result<()> {
    let checkout = std::fs::canonicalize(checkout)?;
    let (recipe, source) = load_recipe(&checkout, configured)?;
    recipe.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        recipe.secret_bindings.is_empty(),
        "project_cache_runtime_secrets_forbidden"
    );
    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&recipe)?));
    ensure!(digest == expected_digest, "project_cache_recipe_mismatch");
    ensure!(
        !data.join("environment-ready.json").exists(),
        "project_cache_environment_already_initialized"
    );
    record(
        &data.join("environment-ready.json"),
        &PreparedEnvironment {
            version: 1,
            workspace: workspace.into(),
            incarnation: incarnation.into(),
            checkout,
            source,
            recipe,
            digest,
            state: "ready".into(),
        },
    )
}
/// A new cache build explicitly refreshes dependency inputs. Retained task
/// resume does not call this operation. Its caller journals the original build
/// intent before either initialization or refresh can have effects.
pub(crate) async fn refresh_project_cache(
    checkout: &Path,
    recipe: &EnvironmentRecipe,
) -> Result<()> {
    ensure!(
        recipe.secret_bindings.is_empty(),
        "project_cache_runtime_secrets_forbidden"
    );
    if let Some(command) = &recipe.scripts.refresh {
        let (success, stdout, stderr) =
            run_command_with_environment(checkout, recipe, command, &PrivateEnvironment::new())
                .await?;
        let _stdout = zeroize::Zeroizing::new(stdout);
        let _stderr = zeroize::Zeroizing::new(stderr);
        ensure!(success, "environment_dependency_refresh_failed");
    }
    Ok(())
}
fn record(path: &Path, receipt: &PreparedEnvironment) -> Result<()> {
    atomic_write(path, &serde_json::to_vec(receipt)?)
}
fn load_recipe(
    checkout: &Path,
    configured: &EnvironmentRecipe,
) -> Result<(EnvironmentRecipe, EnvironmentSource)> {
    let file = checkout.join(".zuse/setup.json");
    match std::fs::symlink_metadata(&file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok((configured.clone(), EnvironmentSource::Project))
        }
        Err(error) => Err(error.into()),
        Ok(_) => {
            let file = std::fs::canonicalize(file)?;
            ensure!(
                file.starts_with(checkout),
                "environment_recipe_outside_repository"
            );
            let metadata = std::fs::metadata(&file)?;
            ensure!(
                metadata.is_file() && metadata.len() <= MAX_RECIPE,
                "environment_recipe_too_large"
            );
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(file)?
                .take(MAX_RECIPE + 1)
                .read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_RECIPE,
                "environment_recipe_too_large"
            );
            Ok((
                serde_json::from_slice(&bytes)
                    .map_err(|_| anyhow::anyhow!("environment_recipe_invalid"))?,
                EnvironmentSource::Repository,
            ))
        }
    }
}

pub fn selected_recipe(
    checkout: &Path,
    configured: &EnvironmentRecipe,
) -> Result<EnvironmentRecipe> {
    let (recipe, _) = load_recipe(checkout, configured)?;
    recipe.validate().map_err(anyhow::Error::msg)?;
    Ok(recipe)
}

/// Caller holds the task's lifetime runtime data lock. An uncertain command is
/// deliberately not replayed: installation scripts may have external side effects.
pub async fn prepare(
    data: &Path,
    checkout: &Path,
    workspace: &str,
    incarnation: &str,
    configured: &EnvironmentRecipe,
) -> Result<PreparedEnvironment> {
    prepare_with_environment(
        data,
        checkout,
        workspace,
        incarnation,
        configured,
        &PrivateEnvironment::new(),
    )
    .await
}
pub async fn prepare_with_environment(
    data: &Path,
    checkout: &Path,
    workspace: &str,
    incarnation: &str,
    configured: &EnvironmentRecipe,
    secrets: &PrivateEnvironment,
) -> Result<PreparedEnvironment> {
    prepare_with_protection(
        data,
        checkout,
        workspace,
        incarnation,
        configured,
        secrets,
        OutputProtection::default(),
    )
    .await
}
pub async fn prepare_with_protection(
    data: &Path,
    checkout: &Path,
    workspace: &str,
    incarnation: &str,
    configured: &EnvironmentRecipe,
    secrets: &PrivateEnvironment,
    protection: OutputProtection,
) -> Result<PreparedEnvironment> {
    let checkout = std::fs::canonicalize(checkout)?;
    let marker = data.join("environment-ready.json");
    match std::fs::read(&marker) {
        Ok(bytes) => {
            ensure!(
                bytes.len() <= 2 * 1024 * 1024,
                "environment_receipt_invalid"
            );
            let receipt: PreparedEnvironment = serde_json::from_slice(&bytes)?;
            ensure!(
                receipt.version == 1
                    && receipt.workspace == workspace
                    && receipt.incarnation == incarnation
                    && receipt.checkout == checkout,
                "environment_identity_mismatch"
            );
            ensure!(
                receipt.state == "ready",
                "environment_preparation_requires_recovery"
            );
            return Ok(receipt);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let (recipe, source) = load_recipe(&checkout, configured)?;
    recipe.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        recipe.secret_bindings.is_empty() || !secrets.is_empty(),
        "environment_secret_delivery_required"
    );
    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&recipe)?));
    let mut receipt = PreparedEnvironment {
        version: 1,
        workspace: workspace.into(),
        incarnation: incarnation.into(),
        checkout,
        source,
        recipe,
        digest,
        state: "running".into(),
    };
    record(&marker, &receipt)?;
    if let Some(command) = &receipt.recipe.scripts.initialize {
        let result =
            run_command_with_environment(&receipt.checkout, &receipt.recipe, command, secrets)
                .await;
        match result {
            Ok((success, stdout, stderr)) => {
                let stdout = zeroize::Zeroizing::new(stdout);
                let stderr = zeroize::Zeroizing::new(stderr);
                let stdout = protection.mask(&redact_output(&stdout, secrets));
                let stderr = protection.mask(&redact_output(&stderr, secrets));
                atomic_write(
                    &data.join("environment-initialize.json"),
                    &serde_json::to_vec(
                        &serde_json::json!({"success":success,"stdout":String::from_utf8_lossy(&stdout),"stderr":String::from_utf8_lossy(&stderr)}),
                    )?,
                )?;
                if !success {
                    receipt.state = "failed".into();
                    record(&marker, &receipt)?;
                    anyhow::bail!("environment_initialize_failed");
                }
            }
            Err(_) => {
                receipt.state = "uncertain".into();
                record(&marker, &receipt)?;
                anyhow::bail!("environment_initialize_interrupted");
            }
        }
    }
    receipt.state = "ready".into();
    record(&marker, &receipt)?;
    Ok(receipt)
}

struct CommandGroup {
    child: tokio::process::Child,
    pid: Option<u32>,
}
impl CommandGroup {
    fn terminate_descendants(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.take() {
            // Own process group only; no shell interpolation or external kill tool.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
impl Drop for CommandGroup {
    fn drop(&mut self) {
        self.terminate_descendants();
    }
}
async fn bounded_output(mut input: impl AsyncRead + Unpin) -> std::io::Result<Vec<u8>> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = input.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        kept.extend_from_slice(&chunk[..read.min(MAX_LOG.saturating_sub(kept.len()))]);
    }
    Ok(kept)
}
#[cfg(test)]
async fn run_command(
    checkout: &Path,
    recipe: &EnvironmentRecipe,
    script: &str,
) -> Result<(bool, Vec<u8>, Vec<u8>)> {
    run_command_with_environment(checkout, recipe, script, &PrivateEnvironment::new()).await
}
async fn run_command_with_environment(
    checkout: &Path,
    recipe: &EnvironmentRecipe,
    script: &str,
    secrets: &PrivateEnvironment,
) -> Result<(bool, Vec<u8>, Vec<u8>)> {
    let mut command = environment_command(checkout, recipe);
    command.envs(secrets.iter().map(|(name, value)| (name, value.as_str())));
    command.args([
        "--noprofile",
        "--norc",
        "-e",
        "-o",
        "pipefail",
        "-c",
        script,
    ]);
    let spawned = command.spawn()?;
    let mut child = CommandGroup {
        pid: spawned.id(),
        child: spawned,
    };
    let stdout = child.child.stdout.take().expect("stdout piped");
    let stderr = child.child.stderr.take().expect("stderr piped");
    let output = tokio::time::timeout(
        Duration::from_secs(recipe.setup_timeout_seconds.into()),
        async {
            let wait = async {
                let status = child.child.wait().await?;
                child.terminate_descendants();
                Ok::<_, std::io::Error>(status.success())
            };
            tokio::try_join!(wait, bounded_output(stdout), bounded_output(stderr))
        },
    )
    .await
    .map_err(|_| anyhow::anyhow!("environment_command_timeout"))??;
    Ok(output)
}

fn environment_command(checkout: &Path, recipe: &EnvironmentRecipe) -> tokio::process::Command {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/home/zuse".into()));
    let mut command = tokio::process::Command::new("/bin/bash");
    command
        .current_dir(checkout)
        .env_clear()
        .env("PATH", environment_path(&home))
        .env("LANG", "C.UTF-8")
        .env("HOME", &home)
        .envs(&recipe.environment)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    command
}

/// Repository scripts see what the machine's setup installed: the task user's
/// `npm i -g` lands in ~/.local/bin (the base sets that npm prefix).
fn environment_path(home: &Path) -> String {
    format!(
        "{}:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
        home.join(".local/bin").display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repository_scripts_find_the_task_users_global_npm_tools() {
        assert_eq!(
            environment_path(Path::new("/home/zuse")),
            "/home/zuse/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
        );
    }
    fn directories() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        let checkout = root.path().join("checkout");
        std::fs::create_dir(&data).unwrap();
        std::fs::create_dir(&checkout).unwrap();
        (root, data, checkout)
    }

    #[tokio::test]
    async fn repository_setup_wins_and_resume_keeps_the_original_recipe_without_reinstalling() {
        let (_root, data, checkout) = directories();
        std::fs::create_dir(checkout.join(".zuse")).unwrap();
        let mut configured = EnvironmentRecipe::default();
        configured.scripts.initialize = Some("false".into());
        let mut repository = EnvironmentRecipe::default();
        repository.scripts.initialize = Some("printf x >> installed".into());
        repository.scripts.refresh = Some("touch unexpected-refresh".into());
        let setup = checkout.join(".zuse/setup.json");
        std::fs::write(&setup, serde_json::to_vec(&repository).unwrap()).unwrap();
        let first = prepare(&data, &checkout, "task-one", "disk-one", &configured)
            .await
            .unwrap();
        assert_eq!(first.source, EnvironmentSource::Repository);
        std::fs::write(&setup, b"invalid edited setup").unwrap();
        let resumed = prepare(&data, &checkout, "task-one", "disk-one", &configured)
            .await
            .unwrap();
        assert_eq!(resumed.digest, first.digest);
        assert_eq!(std::fs::read(checkout.join("installed")).unwrap(), b"x");
        assert!(!checkout.join("unexpected-refresh").exists());
        assert!(
            prepare(&data, &checkout, "task-one", "different-disk", &configured)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn failed_or_uncertain_setup_does_not_replay_side_effects_on_process_restart() {
        let (_root, data, checkout) = directories();
        let mut recipe = EnvironmentRecipe::default();
        recipe.scripts.initialize = Some("printf x >> attempts; false".into());
        assert!(
            prepare(&data, &checkout, "task", "disk", &recipe)
                .await
                .is_err()
        );
        assert!(
            prepare(&data, &checkout, "task", "disk", &recipe)
                .await
                .is_err()
        );
        let marker = data.join("environment-ready.json");
        let mut state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&marker).unwrap()).unwrap();
        state["state"] = serde_json::json!("running");
        std::fs::write(&marker, serde_json::to_vec(&state).unwrap()).unwrap();
        assert!(
            prepare(&data, &checkout, "task", "disk", &recipe)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(checkout.join("attempts")).unwrap(), b"x");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_kills_descendants_and_logs_remain_bounded() {
        let (_root, data, checkout) = directories();
        let mut recipe = EnvironmentRecipe::default();
        recipe.setup_timeout_seconds = 1;
        recipe.scripts.initialize = Some("(sleep 2; touch orphan) & wait".into());
        assert!(
            prepare(&data, &checkout, "task", "disk", &recipe)
                .await
                .is_err()
        );
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!checkout.join("orphan").exists());
        let (_, out, err) = run_command(
            &checkout,
            &EnvironmentRecipe::default(),
            "head -c 100000 /dev/zero; head -c 100000 /dev/zero >&2",
        )
        .await
        .unwrap();
        assert_eq!(out.len(), MAX_LOG);
        assert_eq!(err.len(), MAX_LOG);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repository_recipe_cannot_follow_a_symlink_outside_its_checkout() {
        let (root, data, checkout) = directories();
        std::fs::create_dir(checkout.join(".zuse")).unwrap();
        let outside = root.path().join("outside.json");
        std::fs::write(&outside, b"{}").unwrap();
        std::os::unix::fs::symlink(outside, checkout.join(".zuse/setup.json")).unwrap();
        assert!(
            prepare(
                &data,
                &checkout,
                "task",
                "disk",
                &EnvironmentRecipe::default()
            )
            .await
            .is_err()
        );
        assert!(!data.join("environment-ready.json").exists());
    }
    #[tokio::test]
    async fn preparation_output_is_private_bounded_utf8_and_redacts_platform_and_recipe_values() {
        let (_root, data, checkout) = directories();
        let mut recipe = EnvironmentRecipe::default();
        recipe
            .environment
            .insert("PROJECT_SETTING".into(), "private-recipe-value".into());
        recipe
            .environment
            .insert("SMALL_SETTING".into(), "xy".into());
        prepare(&data, &checkout, "task", "disk", &recipe)
            .await
            .unwrap();
        let output = format!(
            "private-runtime-token private-recipe-value xy {}",
            "é".repeat(20_000)
        );
        atomic_write(
            &data.join("environment-initialize.json"),
            &serde_json::to_vec(&serde_json::json!({"success":true,"stdout":output,"stderr":""}))
                .unwrap(),
        )
        .unwrap();
        let log = preparation_log(&data, &["private-runtime-token"])
            .unwrap()
            .unwrap();
        assert!(!log.stdout.contains("private-runtime-token"));
        assert!(!log.stdout.contains("private-recipe-value"));
        assert!(!log.stdout.contains("xy"));
        assert!(log.stdout.contains("[redacted]"));
        assert!(log.stdout.len() <= zuse_protocol::cloud_computers::MAX_PREPARATION_LOG_BYTES);
        assert!(log.truncated);
        assert!(!format!("{log:?}").contains("[redacted]"));
    }
    #[cfg(feature = "engine")]
    #[tokio::test]
    async fn known_git_grant_output_is_masked_before_preparation_file_and_receipt() {
        let (_root, data, checkout) = directories();
        let key = "synthetic-github-grant";
        std::fs::write(checkout.join("private-fixture-source"), key).unwrap();
        let registry =
            std::sync::Arc::new(zuse_providers::credentials::redaction::Secrets::default());
        registry.record(key).unwrap();
        let mut recipe = EnvironmentRecipe::default();
        recipe.scripts.initialize = Some("cat private-fixture-source".into());
        prepare_with_protection(
            &data,
            &checkout,
            "task",
            "disk",
            &recipe,
            &PrivateEnvironment::new(),
            OutputProtection::new(registry),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(checkout.join("private-fixture-source")).unwrap(),
            key,
            "private operational source stays untouched"
        );
        let log = std::fs::read_to_string(data.join("environment-initialize.json")).unwrap();
        assert!(!log.contains(key));
        assert!(log.contains("[redacted]"));
        assert!(
            !std::fs::read_to_string(data.join("environment-ready.json"))
                .unwrap()
                .contains(key)
        );
    }
}

#[cfg(test)]
mod private_tests;
