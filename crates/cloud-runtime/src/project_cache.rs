//! Credential-free project preparation and exact snapshot overlay consumption.
//! This module never enrolls a runtime, opens Core or starts an agent/service.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use zuse_protocol::cloud_computers::EnvironmentRecipe;

pub mod capture;

pub const CAPABILITY: &str = "rust-project-environment-cache-v1";

pub const ROOT: &str = "/opt/zuse/project-caches";
const MAX_PLAN: u64 = 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub cache_key: String,
    pub operation_id: String,
    pub identity: Value,
    pub recipe: EnvironmentRecipe,
    pub source_bundle_sha256: String,
    pub home: PathBuf,
    pub cache_root: PathBuf,
    pub bundle: PathBuf,
    pub supervisor: bool,
}
fn hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn git(checkout: Option<&Path>, args: &[&std::ffi::OsStr]) -> Result<String> {
    let mut c = Command::new("git");
    c.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(
            if args
                .iter()
                .any(|arg| *arg == os("rev-parse") || *arg == os("branch"))
            {
                Stdio::piped()
            } else {
                Stdio::null()
            },
        )
        .args(["-c", "core.hooksPath=/dev/null"]);
    if let Some(path) = checkout {
        c.arg("-C").arg(path);
    }
    let output = c.args(args).output()?;
    ensure!(output.status.success(), "project_cache_git_failed");
    ensure!(
        output.stdout.len() <= 1024 * 1024,
        "project_cache_git_output_invalid"
    );
    Ok(String::from_utf8(output.stdout)?.trim().into())
}
fn os(s: &str) -> &std::ffi::OsStr {
    std::ffi::OsStr::new(s)
}
fn digest_file(file: &Path, max: u64) -> Result<String> {
    let metadata = fs::symlink_metadata(file)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= max,
        "project_cache_file_invalid"
    );
    let mut f = fs::File::open(file)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn recipe_digest(recipe: &EnvironmentRecipe) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(recipe)?)))
}
/// A reusable platform base must not contain any account's prepared project.
pub(crate) fn assert_empty(root: &Path) -> Result<()> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
        Ok(metadata) => ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "managed_base_contains_project_cache"
        ),
    }
    for entry in fs::read_dir(root)? {
        ensure!(
            entry?.file_name() == ".workspace-runtime.lock",
            "managed_base_contains_project_cache"
        );
    }
    Ok(())
}
fn directory(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "project_cache_path_invalid");
    fs::create_dir_all(path)?;
    let actual = fs::canonicalize(path)?;
    ensure!(actual == path, "project_cache_path_symlink");
    Ok(())
}
fn bounded_json(file: &Path) -> Result<Value> {
    let metadata = fs::symlink_metadata(file)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= MAX_PLAN,
        "project_cache_receipt_invalid"
    );
    Ok(serde_json::from_slice(&fs::read(file)?)?)
}
fn receipt(plan: &Plan) -> Value {
    json!({"version":1,"cacheKey":plan.cache_key,"operationId":plan.operation_id,"runtimeSha256":plan.identity["baseRuntimeSha256"],"recipeDigest":plan.identity["recipeDigest"],"dependencyDigest":dependency_digest(&plan.identity),"sourceCommit":plan.identity["sourceCommit"],"sourceBundleSha256":plan.source_bundle_sha256,"credentialsRemoved":true,"taskStorageAbsent":true,"agentProfilesAbsent":true,"secretFreeSource":true})
}
fn dependency_digest(identity: &Value) -> String {
    // CacheIdentity hashes this tuple, with dependency paths in sorted order.
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                identity["sourceCommit"].as_str().unwrap_or_default(),
                &identity["dependencyInputs"]
            ))
            .expect("JSON serializes")
        )
    )
}
fn validate(plan: &Plan) -> Result<()> {
    plan.recipe.validate().map_err(anyhow::Error::msg)?;
    ensure!(
        plan.version == 1
            && hash(&plan.cache_key)
            && hash(&plan.source_bundle_sha256)
            && !plan.operation_id.is_empty()
            && plan.operation_id.len() <= 160
            && plan
                .operation_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-:.".contains(&b)),
        "project_cache_plan_invalid"
    );
    ensure!(
        plan.recipe.secret_bindings.is_empty(),
        "project_cache_runtime_secrets_forbidden"
    );
    ensure!(
        plan.identity["recipeDigest"] == recipe_digest(&plan.recipe)?,
        "project_cache_recipe_mismatch"
    );
    let commit = plan.identity["sourceCommit"].as_str().unwrap_or_default();
    ensure!(
        matches!(commit.len(), 40 | 64)
            && commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "project_cache_commit_invalid"
    );
    ensure!(
        plan.identity["baseRuntimeSha256"]
            == digest_file(&std::env::current_exe()?, 512 * 1024 * 1024)?,
        "project_cache_runtime_mismatch"
    );
    Ok(())
}
/// Called only on a fresh, isolated managed-base child. The Git bundle was
/// generated outside the VM; there is no askpass/token file or authority client.
pub async fn prepare(plan: &Plan) -> Result<Value> {
    validate(plan)?;
    directory(&plan.home)?;
    directory(&plan.cache_root)?;
    crate::image::clean_home(&plan.home, plan.supervisor)?;
    ensure!(
        fs::read_dir(plan.home.join("workspace"))?.next().is_none(),
        "project_cache_task_checkout_present"
    );
    ensure!(
        digest_file(&plan.bundle, 2 * 1024 * 1024 * 1024)? == plan.source_bundle_sha256,
        "project_cache_bundle_mismatch"
    );
    let _base_lock = crate::DataLock::acquire(&plan.cache_root, None)?;
    for entry in fs::read_dir(&plan.cache_root)? {
        let entry = entry?;
        ensure!(
            entry.file_name() == ".workspace-runtime.lock"
                || entry.file_name() == plan.cache_key.as_str(),
            "project_cache_foreign_artifact_present"
        );
    }
    let root = plan.cache_root.join(&plan.cache_key);
    directory(&root)?;
    let _lock = crate::DataLock::acquire(&root, None)?;
    let stamp = root.join("receipt.json");
    let expected = receipt(plan);
    if stamp.exists() {
        let previous = bounded_json(&stamp)?;
        ensure!(
            previous["receipt"] == expected && previous["identity"] == plan.identity,
            "project_cache_preparation_identity_changed"
        );
        return Ok(expected);
    }
    let checkpoint = root.join("preparation-intent.json");
    ensure!(
        !checkpoint.exists(),
        "project_cache_preparation_outcome_uncertain"
    );
    crate::identity::atomic_write(
        &checkpoint,
        &serde_json::to_vec(&json!({"operationId":plan.operation_id,"cacheKey":plan.cache_key}))?,
    )?;
    let checkout = root.join("checkout");
    ensure!(!checkout.exists(), "project_cache_checkout_already_present");
    git(
        None,
        &[
            os("clone"),
            os("--no-checkout"),
            os("--"),
            plan.bundle.as_os_str(),
            checkout.as_os_str(),
        ],
    )?;
    let commit = plan.identity["sourceCommit"]
        .as_str()
        .expect("validated commit");
    git(
        Some(&checkout),
        &[os("checkout"), os("--detach"), os(commit), os("--")],
    )?;
    let selected = crate::environment::selected_recipe(&checkout, &plan.recipe)?;
    ensure!(
        recipe_digest(&selected)? == plan.identity["recipeDigest"],
        "project_cache_recipe_mismatch"
    );
    let data = root.join("preparation");
    directory(&data)?;
    crate::environment::prepare_with_environment(
        &data,
        &checkout,
        &format!("cache:{}", plan.cache_key),
        &plan.operation_id,
        &selected,
        &Default::default(),
    )
    .await?;
    ensure!(
        recipe_digest(&crate::environment::selected_recipe(
            &checkout,
            &plan.recipe
        )?)? == plan.identity["recipeDigest"],
        "project_cache_recipe_changed_during_setup"
    );
    crate::environment::refresh_project_cache(&checkout, &selected).await?;
    ensure!(
        recipe_digest(&crate::environment::selected_recipe(
            &checkout,
            &plan.recipe
        )?)? == plan.identity["recipeDigest"],
        "project_cache_recipe_changed_during_setup"
    );
    // Keep a bare source cache for ordinary repository enrollment; keep its
    // separate overlay for installed dependencies and tested setup changes.
    git(
        None,
        &[
            os("clone"),
            os("--bare"),
            os("--"),
            checkout.as_os_str(),
            root.join("repository.git").as_os_str(),
        ],
    )?;
    git(Some(&checkout), &[os("remote"), os("remove"), os("origin")])?;
    crate::image::clean_home(&plan.home, plan.supervisor)?;
    ensure!(
        digest_file(&plan.bundle, 2 * 1024 * 1024 * 1024)? == plan.source_bundle_sha256,
        "project_cache_bundle_changed_during_setup"
    );
    fs::remove_dir_all(&data)?;
    crate::identity::atomic_write(
        &stamp,
        &serde_json::to_vec(&json!({"version":1,"identity":plan.identity,"receipt":expected}))?,
    )?;
    Ok(expected)
}
fn assert_clean_checkout(checkout: &Path) -> Result<()> {
    // Bound output to one byte: filenames and file contents are never logged.
    let mut child = Command::new("git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ])
        .arg("-C")
        .arg(checkout)
        .args([
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--ignored=matching",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut first = [0u8; 1];
    let read = child.stdout.take().expect("piped stdout").read(&mut first);
    if !matches!(read, Ok(0)) {
        let _ = child.kill();
    }
    let status = child.wait()?;
    ensure!(
        read? == 0 && status.success(),
        "project_cache_checkout_modified"
    );
    Ok(())
}

/// Apply once before the model preparation gate opens. Retained resume returns
/// from its task-bound receipt and never overwrites uncommitted work/software.
pub fn consume(
    root: &Path,
    data: &Path,
    checkout: &Path,
    workspace: &str,
    incarnation: &str,
    cache: &Value,
    configured: &EnvironmentRecipe,
) -> Result<()> {
    let key = cache["cacheKey"]
        .as_str()
        .filter(|s| hash(s))
        .ok_or_else(|| anyhow::anyhow!("project_cache_key_invalid"))?;
    let installed = data.join("project-cache-import.json");
    let binding =
        json!({"version":1,"cacheKey":key,"workspace":workspace,"incarnation":incarnation});
    if installed.exists() {
        ensure!(
            bounded_json(&installed)? == binding,
            "project_cache_import_identity_changed"
        );
        return Ok(());
    }
    ensure!(
        !data.join("project-cache-importing.json").exists(),
        "project_cache_import_requires_recovery"
    );
    let root = fs::canonicalize(root)?;
    let prepared = root.join(key);
    ensure!(
        fs::canonicalize(&prepared)? == prepared,
        "project_cache_path_symlink"
    );
    let stamp = bounded_json(&prepared.join("receipt.json"))?;
    ensure!(
        stamp["version"] == 1
            && stamp["identity"] == cache["identity"]
            && stamp["receipt"] == cache["receipt"]
            && stamp["receipt"]["cacheKey"] == key,
        "project_cache_receipt_mismatch"
    );
    for field in [
        "credentialsRemoved",
        "taskStorageAbsent",
        "agentProfilesAbsent",
        "secretFreeSource",
    ] {
        ensure!(
            stamp["receipt"][field] == true,
            "project_cache_contains_private_state"
        );
    }
    ensure!(
        stamp["receipt"]["runtimeSha256"]
            == digest_file(&std::env::current_exe()?, 512 * 1024 * 1024)?,
        "project_cache_runtime_mismatch"
    );
    ensure!(
        git(Some(checkout), &[os("rev-parse"), os("HEAD")])? == stamp["receipt"]["sourceCommit"],
        "project_cache_source_mismatch"
    );
    ensure!(
        !data.join("environment-ready.json").exists(),
        "project_cache_environment_already_initialized"
    );
    assert_clean_checkout(checkout)?;
    crate::identity::atomic_write(
        &data.join("project-cache-importing.json"),
        &serde_json::to_vec(&binding)?,
    )?;
    let overlay = prepared.join("checkout");
    ensure!(
        fs::canonicalize(&overlay)? == overlay,
        "project_cache_path_symlink"
    );
    copy_overlay(&overlay, checkout, checkout, 0)?;
    crate::environment::adopt_prepared_cache(
        data,
        checkout,
        workspace,
        incarnation,
        configured,
        stamp["receipt"]["recipeDigest"]
            .as_str()
            .unwrap_or_default(),
    )?;
    crate::identity::atomic_write(&installed, &serde_json::to_vec(&binding)?)?;
    fs::remove_file(data.join("project-cache-importing.json"))?;
    Ok(())
}
fn copy_overlay(source: &Path, destination: &Path, checkout: &Path, depth: usize) -> Result<()> {
    ensure!(depth <= 128, "project_cache_overlay_too_deep");
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if depth == 0 && entry.file_name() == ".git" {
            continue;
        }
        let target = destination.join(entry.file_name());
        let parent = fs::canonicalize(destination)?;
        ensure!(
            parent.starts_with(checkout),
            "project_cache_overlay_outside_checkout"
        );
        if let Ok(metadata) = fs::symlink_metadata(&target) {
            if metadata.is_dir() {
                fs::remove_dir_all(&target)?;
            } else {
                fs::remove_file(&target)?;
            }
        }
        let metadata = entry.file_type()?;
        if metadata.is_dir() {
            fs::create_dir(&target)?;
            copy_overlay(&entry.path(), &target, checkout, depth + 1)?;
        } else if metadata.is_file() {
            fs::copy(entry.path(), &target)?;
        } else if metadata.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(fs::read_link(entry.path())?, &target)?;
            #[cfg(not(unix))]
            anyhow::bail!("project_cache_symlinks_require_unix");
        } else {
            anyhow::bail!("project_cache_overlay_special_file");
        }
    }
    Ok(())
}

/// Self-reported protocol availability; provider certification remains separate.
pub fn capabilities() -> Result<Value> {
    Ok(
        json!({"version":1,"runtimeSha256":digest_file(&std::env::current_exe()?,512*1024*1024)?,"environmentCacheVersion":1,"secretFreeSourceBundleVersion":1,"retainedTaskStorageAbsentRequired":true,"credentialFreePreparationRequired":true,"preparationCaptureVersion":1}),
    )
}
/// Fixed managed filesystem layout only; arbitrary paths are not a public CLI.
pub async fn prepare_cli(file: &Path) -> Result<Value> {
    let plan: Plan = serde_json::from_value(bounded_json(file)?)?;
    ensure!(
        plan.home == Path::new("/home/zuse")
            && plan.cache_root == Path::new(ROOT)
            && plan.bundle == Path::new("/opt/zuse/preparation/source.bundle"),
        "project_cache_path_invalid"
    );
    ensure!(
        std::env::var_os("HOME").as_deref() == Some(plan.home.as_os_str()),
        "project_cache_home_invalid"
    );
    #[cfg(unix)]
    ensure!(
        unsafe { libc::geteuid() } == 10001,
        "project_cache_user_invalid"
    );
    prepare(&plan).await
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn source(root: &Path) -> (PathBuf, String) {
        let source = root.join("source");
        fs::create_dir_all(&source).unwrap();
        git(None, &[os("init"), source.as_os_str()]).unwrap();
        git(
            Some(&source),
            &[
                os("config"),
                os("user.email"),
                os("cache-test@example.test"),
            ],
        )
        .unwrap();
        git(
            Some(&source),
            &[os("config"), os("user.name"), os("Cache fixture")],
        )
        .unwrap();
        fs::write(source.join("README.md"), "original\n").unwrap();
        git(Some(&source), &[os("add"), os("README.md")]).unwrap();
        git(Some(&source), &[os("commit"), os("-m"), os("fixture")]).unwrap();
        let commit = git(Some(&source), &[os("rev-parse"), os("HEAD")]).unwrap();
        (source, commit)
    }
    pub(super) fn plan(root: &Path, source: &Path, commit: &str, key: &str, script: &str) -> Plan {
        let bundle = root.join(format!("{key}.bundle"));
        git(
            Some(source),
            &[os("bundle"), os("create"), bundle.as_os_str(), os("--all")],
        )
        .unwrap();
        let home = root.join("home");
        fs::create_dir_all(home.join("workspace")).unwrap();
        let mut recipe = EnvironmentRecipe::default();
        recipe.scripts.initialize = Some(script.into());
        let identity = json!({"baseRuntimeSha256":digest_file(&std::env::current_exe().unwrap(),512*1024*1024).unwrap(),"recipeDigest":recipe_digest(&recipe).unwrap(),"sourceCommit":commit,"dependencyInputs":{}});
        Plan {
            version: 1,
            cache_key: key.into(),
            operation_id: format!("prep-{key}"),
            identity,
            recipe,
            source_bundle_sha256: digest_file(&bundle, 512 * 1024 * 1024).unwrap(),
            home,
            cache_root: root.join("caches"),
            bundle,
            supervisor: false,
        }
    }
    #[test]
    fn initial_import_rejects_tracked_untracked_and_ignored_user_work() {
        let temp = tempfile::tempdir().unwrap();
        let (source, _) = source(temp.path());
        assert_clean_checkout(&source).unwrap();
        fs::write(source.join("README.md"), "user work").unwrap();
        assert!(assert_clean_checkout(&source).is_err());
        git(Some(&source), &[os("checkout"), os("--"), os("README.md")]).unwrap();
        fs::write(source.join("untracked"), "user work").unwrap();
        assert!(assert_clean_checkout(&source).is_err());
        fs::remove_file(source.join("untracked")).unwrap();
        fs::write(source.join(".git/info/exclude"), "node_modules/\n").unwrap();
        fs::create_dir(source.join("node_modules")).unwrap();
        fs::write(source.join("node_modules/keep"), "user installation").unwrap();
        assert!(assert_clean_checkout(&source).is_err());
        assert_eq!(
            fs::read_to_string(source.join("node_modules/keep")).unwrap(),
            "user installation"
        );
    }
    #[test]
    fn managed_base_cannot_include_a_project_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        assert_empty(&root).unwrap();
        fs::create_dir(&root).unwrap();
        assert_empty(&root).unwrap();
        fs::create_dir(root.join("private-project")).unwrap();
        assert!(assert_empty(&root).is_err());
    }
    #[tokio::test]
    async fn offline_preparation_runs_once_and_retained_import_does_not_overwrite_work() {
        let temp = tempfile::tempdir().unwrap();
        let (source, commit) = source(temp.path());
        let mut plan = plan(
            temp.path(),
            &source,
            &commit,
            &"a".repeat(64),
            "printf x >> initialize-count; mkdir -p node_modules; printf installed > node_modules/proof",
        );
        plan.recipe.scripts.refresh = Some("printf y >> refresh-count".into());
        plan.identity["recipeDigest"] = json!(recipe_digest(&plan.recipe).unwrap());
        let receipt = prepare(&plan).await.unwrap();
        assert_eq!(receipt, prepare(&plan).await.unwrap());
        let artifact = plan.cache_root.join(&plan.cache_key);
        assert_eq!(
            fs::read_to_string(artifact.join("checkout/initialize-count")).unwrap(),
            "x"
        );
        assert_eq!(
            fs::read_to_string(artifact.join("checkout/refresh-count")).unwrap(),
            "y"
        );
        assert!(!artifact.join("preparation").exists());
        assert!(!plan.home.join(".zuse-data").exists());
        let checkout = temp.path().join("task");
        git(
            None,
            &[
                os("clone"),
                os("--"),
                source.as_os_str(),
                checkout.as_os_str(),
            ],
        )
        .unwrap();
        git(
            Some(&checkout),
            &[os("checkout"), os("-b"), os("task-branch")],
        )
        .unwrap();
        let data = temp.path().join("task-data");
        fs::create_dir(&data).unwrap();
        let cache = json!({"version":1,"cacheKey":plan.cache_key,"identity":plan.identity,"receipt":receipt});
        consume(
            &plan.cache_root,
            &data,
            &checkout,
            "task",
            "disk",
            &cache,
            &plan.recipe,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(checkout.join("initialize-count")).unwrap(),
            "x"
        );
        assert_eq!(
            fs::read_to_string(checkout.join("node_modules/proof")).unwrap(),
            "installed"
        );
        assert_eq!(
            git(Some(&checkout), &[os("branch"), os("--show-current")]).unwrap(),
            "task-branch"
        );
        crate::environment::prepare(&data, &checkout, "task", "disk", &plan.recipe)
            .await
            .unwrap();
        assert_eq!(
            fs::read_to_string(checkout.join("initialize-count")).unwrap(),
            "x"
        );
        fs::write(checkout.join("README.md"), "retained dirty work").unwrap();
        fs::write(checkout.join("node_modules/proof"), "retained installation").unwrap();
        consume(
            &plan.cache_root,
            &data,
            &checkout,
            "task",
            "disk",
            &cache,
            &plan.recipe,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(checkout.join("README.md")).unwrap(),
            "retained dirty work"
        );
        assert_eq!(
            fs::read_to_string(checkout.join("node_modules/proof")).unwrap(),
            "retained installation"
        );
        assert!(
            consume(
                &plan.cache_root,
                &data,
                &checkout,
                "foreign-task",
                "disk",
                &cache,
                &plan.recipe
            )
            .is_err()
        );
        let mut changed = plan.clone();
        changed.operation_id = "other-operation".into();
        assert!(prepare(&changed).await.is_err());
        assert_eq!(
            fs::read_to_string(artifact.join("checkout/initialize-count")).unwrap(),
            "x"
        );
    }
    #[tokio::test]
    async fn failed_setup_is_not_replayed_and_no_receipt_is_published() {
        let temp = tempfile::tempdir().unwrap();
        let (source, commit) = source(temp.path());
        let plan = plan(
            temp.path(),
            &source,
            &commit,
            &"b".repeat(64),
            "printf x >> side-effect; exit 1",
        );
        assert!(prepare(&plan).await.is_err());
        assert!(
            prepare(&plan)
                .await
                .unwrap_err()
                .to_string()
                .contains("outcome_uncertain")
        );
        let artifact = plan.cache_root.join(&plan.cache_key);
        assert_eq!(
            fs::read_to_string(artifact.join("checkout/side-effect")).unwrap(),
            "x"
        );
        assert!(!artifact.join("receipt.json").exists());
    }
    #[test]
    fn overlay_never_follows_a_destination_symlink_or_replaces_git() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("dest");
        let outside = temp.path().join("outside");
        for path in [&source, &dest, &outside] {
            fs::create_dir(path).unwrap();
        }
        fs::create_dir(source.join("node_modules")).unwrap();
        fs::write(source.join("node_modules/new"), "new").unwrap();
        fs::create_dir(source.join(".git")).unwrap();
        fs::write(source.join(".git/untrusted"), "untrusted").unwrap();
        fs::create_dir(dest.join(".git")).unwrap();
        fs::write(dest.join(".git/retained"), "retained").unwrap();
        fs::write(outside.join("proof"), "untouched").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, dest.join("node_modules")).unwrap();
        copy_overlay(&source, &dest, &dest, 0).unwrap();
        assert_eq!(
            fs::read_to_string(outside.join("proof")).unwrap(),
            "untouched"
        );
        assert!(!outside.join("new").exists());
        assert!(dest.join("node_modules/new").is_file());
        assert!(dest.join(".git/retained").is_file());
        assert!(!dest.join(".git/untrusted").exists());
    }
}
