//! Prepare an isolated image's repository caches. Never runs against enrolled runtime storage.
use crate::{DataLock, identity::atomic_write};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use zeroize::Zeroizing;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImagePlan {
    pub build_id: String,
    pub template_version: String,
    pub configuration_digest: String,
    pub home: PathBuf,
    pub cache_root: PathBuf,
    pub status_directory: PathBuf,
    pub secrets_root: PathBuf,
    pub max_cache_bytes: u64,
    pub repositories: Vec<ImageRepository>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageRepository {
    pub project_id: String,
    pub repository_identity: String,
    pub repository_url: String,
    pub default_branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_file: Option<PathBuf>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedRepository {
    pub project_id: String,
    pub repository_identity: String,
    pub path: PathBuf,
    pub default_branch: String,
    pub source_commit: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust_launch_assets_version: Option<u32>,
    pub version: u32,
    pub build_id: String,
    pub template_version: String,
    pub configuration_digest: String,
    pub plan_digest: String,
    pub runtime_rpc_protocol: String,
    pub rust_runtime_storage_format: String,
    pub rust_session_command_version: u32,
    pub credentials_removed: bool,
    pub repositories: Vec<PreparedRepository>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress<'a> {
    build_id: &'a str,
    phase: &'a str,
    project_id: Option<&'a str>,
    updated_at_ms: i64,
}
struct Tokens {
    files: Vec<PathBuf>,
    askpass: PathBuf,
}
impl Drop for Tokens {
    fn drop(&mut self) {
        for file in &self.files {
            let _ = fs::remove_file(file);
        }
        let _ = fs::remove_file(&self.askpass);
    }
}
fn progress(plan: &ImagePlan, phase: &str, project: Option<&str>) -> Result<()> {
    atomic_write(
        &plan.status_directory.join("progress.json"),
        &serde_json::to_vec(&Progress {
            build_id: &plan.build_id,
            phase,
            project_id: project,
            updated_at_ms: crate::now_ms(),
        })?,
    )?;
    // Log only validated identifiers and phases; Git/provider diagnostics may contain credentials.
    use std::io::Write;
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(plan.status_directory.join("build.log"))?;
    writeln!(
        log,
        "{} {} {}",
        crate::now_ms(),
        phase,
        project.unwrap_or("")
    )?;
    Ok(())
}
fn owned_directory(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "image_directory_not_absolute");
    fs::create_dir_all(path)?;
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "image_directory_is_symlink"
    );
    Ok(())
}
fn clean_home(home: &Path) -> Result<()> {
    for name in [
        ".codex/auth.json",
        ".claude/.credentials.json",
        ".grok/auth.json",
        ".config/gh",
        ".git-credentials",
        ".netrc",
        ".zuse/cloud-auth",
        ".zuse-image/provider-secrets.json",
        ".zuse-data/cloud-runtime-identity.json",
        ".zuse-data/rust-repository-ready.json",
    ] {
        ensure!(
            fs::symlink_metadata(home.join(name))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound),
            "image_contains_account_credentials_or_runtime_identity"
        );
    }
    let data = home.join(".zuse-data");
    if data.exists() {
        ensure!(
            fs::read_dir(data)?.next().is_none(),
            "image_contains_runtime_data"
        );
    }
    let ssh = home.join(".ssh");
    if ssh.exists() {
        ensure!(
            fs::read_dir(ssh)?.next().is_none(),
            "image_contains_ssh_identity"
        );
    }
    Ok(())
}
fn validate_repo(repo: &ImageRepository) -> Result<()> {
    ensure!(
        !repo.project_id.is_empty()
            && repo.project_id.len() <= 256
            && !repo.project_id.chars().any(char::is_control),
        "image_repository_id_invalid"
    );
    let url = reqwest::Url::parse(&repo.repository_url)?;
    let local = cfg!(test) && url.scheme() == "http" && url.host_str() == Some("127.0.0.1");
    ensure!(
        (url.scheme() == "https" || local)
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "image_repository_url_invalid"
    );
    if repo.token_file.is_some() {
        ensure!(
            local
                || (url.host_str() == Some("github.com")
                    && url.port_or_known_default() == Some(443)),
            "image_git_credentials_host_invalid"
        );
    }
    let path = url.path().trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts = path.split('/').collect::<Vec<_>>();
    ensure!(
        parts.len() == 2
            && parts.iter().all(|p| !p.is_empty()
                && p.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))),
        "image_repository_url_invalid"
    );
    let identity = format!(
        "{}/{}",
        url.host_str().unwrap().to_lowercase(),
        path.to_lowercase()
    );
    ensure!(
        repo.repository_identity == identity,
        "image_repository_identity_mismatch"
    );
    ensure!(
        !repo.default_branch.is_empty()
            && !repo.default_branch.starts_with('-')
            && !repo.default_branch.chars().any(char::is_control),
        "image_repository_branch_invalid"
    );
    Ok(())
}
fn git(
    plan: &ImagePlan,
    args: &[&std::ffi::OsStr],
    token: Option<&Path>,
    askpass: &Path,
) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.env_clear()
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into()),
        )
        .env("HOME", &plan.home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_HTTP_LOW_SPEED_LIMIT", "1")
        .env("GIT_HTTP_LOW_SPEED_TIME", "30")
        .env(
            "GIT_ALLOW_PROTOCOL",
            if cfg!(test) { "http:https" } else { "https" },
        )
        .args([
            "-c",
            "credential.helper=",
            "-c",
            "credential.useHttpPath=true",
        ])
        .args(args)
        .stdin(Stdio::null());
    if let Some(token) = token {
        cmd.env("GIT_ASKPASS", askpass)
            .env("ZUSE_GIT_TOKEN_FILE", token);
    }
    let result = cmd.output()?;
    ensure!(
        result.status.success(),
        "image_repository_git_command_failed"
    );
    ensure!(
        result.stdout.len() <= 65536,
        "image_repository_git_output_too_large"
    );
    Ok(String::from_utf8(result.stdout)?.trim().into())
}
fn bytes_under(path: &Path) -> Result<u64> {
    let mut bytes = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        ensure!(
            !entry.file_type()?.is_symlink(),
            "image_cache_symlink_refused"
        );
        bytes = bytes
            .checked_add(if metadata.is_dir() {
                bytes_under(&entry.path())?
            } else {
                metadata.len()
            })
            .ok_or_else(|| anyhow::anyhow!("image_cache_size_overflow"))?;
    }
    Ok(bytes)
}
/// Runs only inside a fresh, isolated image builder. The control plane fences the
/// machine/operation before invoking this command and before publishing its snapshot.
pub fn prepare(plan: &ImagePlan) -> Result<ImageManifest> {
    ensure!(
        !plan.build_id.is_empty()
            && plan.build_id.len() <= 256
            && !plan.build_id.chars().any(char::is_control),
        "image_build_id_invalid"
    );
    ensure!(
        !plan.template_version.is_empty()
            && !plan.configuration_digest.is_empty()
            && plan.max_cache_bytes > 0
            && !plan.repositories.is_empty(),
        "image_plan_invalid"
    );
    for repo in &plan.repositories {
        validate_repo(repo)?;
    }
    owned_directory(&plan.home)?;
    clean_home(&plan.home)?;
    let workspace = plan.home.join("workspace");
    owned_directory(&workspace)?;
    ensure!(
        fs::read_dir(&workspace)?.next().is_none(),
        "image_contains_user_checkout"
    );
    owned_directory(&plan.cache_root)?;
    owned_directory(&plan.status_directory)?;
    owned_directory(&plan.secrets_root)?;
    let cache = fs::canonicalize(&plan.cache_root)?;
    let secrets = fs::canonicalize(&plan.secrets_root)?;
    ensure!(
        cache != secrets && !secrets.starts_with(&cache) && !cache.starts_with(&secrets),
        "image_cache_secrets_overlap"
    );
    let _lock = DataLock::acquire(&cache, None)?;
    let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(plan)?));
    let ready = plan.status_directory.join("manifest.json");
    if ready.exists() {
        let prior: ImageManifest = serde_json::from_slice(&fs::read(&ready)?)?;
        ensure!(
            prior.plan_digest == digest
                && prior.build_id == plan.build_id
                && prior.credentials_removed,
            "image_build_identity_mismatch"
        );
        ensure!(
            fs::read_dir(&secrets)?.next().is_none(),
            "image_contains_unconsumed_secrets"
        );
        ensure!(
            prior.version == 1
                && prior.template_version == plan.template_version
                && prior.configuration_digest == plan.configuration_digest
                && prior.runtime_rpc_protocol == zuse_protocol::cloud_gateway::RUST_RPC
                && prior.rust_runtime_storage_format
                    == zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT
                && prior.rust_session_command_version == 1
                && prior.repositories.len() == plan.repositories.len(),
            "image_manifest_invalid"
        );
        for (repository, expected) in prior.repositories.iter().zip(&plan.repositories) {
            let key = format!(
                "{:x}",
                Sha256::digest(expected.repository_identity.as_bytes())
            );
            ensure!(
                repository.project_id == expected.project_id
                    && repository.repository_identity == expected.repository_identity
                    && repository.default_branch == expected.default_branch
                    && repository.path == cache.join(format!("{key}.git"))
                    && !fs::symlink_metadata(&repository.path)?
                        .file_type()
                        .is_symlink(),
                "image_manifest_repository_mismatch"
            );
            let reference = format!("refs/heads/{}^{{commit}}", repository.default_branch);
            let commit = git(
                plan,
                &[
                    "-C".as_ref(),
                    repository.path.as_os_str(),
                    "rev-parse".as_ref(),
                    "--verify".as_ref(),
                    "--end-of-options".as_ref(),
                    reference.as_ref(),
                ],
                None,
                &secrets.join("image-git-askpass"),
            )?;
            ensure!(
                commit == repository.source_commit,
                "image_cache_changed_after_ready"
            );
            git(
                plan,
                &[
                    "-C".as_ref(),
                    repository.path.as_os_str(),
                    "fsck".as_ref(),
                    "--no-dangling".as_ref(),
                ],
                None,
                &secrets.join("image-git-askpass"),
            )?;
        }
        return Ok(prior);
    }
    let askpass = secrets.join("image-git-askpass");
    let mut tokens = Tokens {
        files: vec![],
        askpass: askpass.clone(),
    };
    for repo in &plan.repositories {
        if let Some(path) = &repo.token_file {
            let metadata = fs::symlink_metadata(path)?;
            ensure!(
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && fs::canonicalize(path)?.parent() == Some(secrets.as_path()),
                "image_git_token_path_invalid"
            );
            ensure!(
                path.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("github-installation-")),
                "image_git_token_path_invalid"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    metadata.uid() == unsafe { libc::geteuid() }
                        && metadata.mode() & 0o777 == 0o600
                        && metadata.nlink() == 1,
                    "image_git_token_permissions_invalid"
                );
            }
            let token = Zeroizing::new(fs::read_to_string(path)?);
            ensure!(
                !token.trim().is_empty() && !token.contains(['\r', '\n', '\0']),
                "image_git_token_invalid"
            );
            if !tokens.files.contains(path) {
                tokens.files.push(path.clone());
            }
        }
    }
    atomic_write(&askpass,b"#!/bin/sh\ncase \"$1\" in *Username*) printf '%s\\n' x-access-token;; *Password*) cat -- \"$ZUSE_GIT_TOKEN_FILE\";; *) exit 1;; esac\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&askpass, fs::Permissions::from_mode(0o700))?;
    }
    let result = (|| -> Result<ImageManifest> {
        progress(plan, "initializing", None)?;
        let mut prepared = Vec::new();
        let mut identities = std::collections::HashSet::new();
        for repo in &plan.repositories {
            ensure!(
                identities.insert(&repo.repository_identity),
                "image_duplicate_repository"
            );
            git(
                plan,
                &[
                    "check-ref-format".as_ref(),
                    "--branch".as_ref(),
                    repo.default_branch.as_ref(),
                ],
                None,
                &askpass,
            )?;
            let key = format!("{:x}", Sha256::digest(repo.repository_identity.as_bytes()));
            let destination = cache.join(format!("{key}.git"));
            progress(plan, "fetching", Some(&repo.project_id))?;
            if destination.exists() {
                ensure!(
                    !fs::symlink_metadata(&destination)?.file_type().is_symlink(),
                    "image_cache_symlink_refused"
                );
                let origin = git(
                    plan,
                    &[
                        "-C".as_ref(),
                        destination.as_os_str(),
                        "remote".as_ref(),
                        "get-url".as_ref(),
                        "origin".as_ref(),
                    ],
                    None,
                    &askpass,
                )?;
                ensure!(origin == repo.repository_url, "image_cache_origin_mismatch");
                ensure!(
                    git(
                        plan,
                        &[
                            "-C".as_ref(),
                            destination.as_os_str(),
                            "rev-parse".as_ref(),
                            "--is-bare-repository".as_ref()
                        ],
                        None,
                        &askpass
                    )? == "true",
                    "image_cache_not_bare"
                );
                git(
                    plan,
                    &[
                        "-C".as_ref(),
                        destination.as_os_str(),
                        "fetch".as_ref(),
                        "--quiet".as_ref(),
                        "--prune".as_ref(),
                        "origin".as_ref(),
                        "+refs/heads/*:refs/heads/*".as_ref(),
                        "+refs/tags/*:refs/tags/*".as_ref(),
                    ],
                    repo.token_file.as_deref(),
                    &askpass,
                )?;
            } else {
                let staged = cache.join(format!(".{key}-{}", uuid::Uuid::new_v4().simple()));
                let clone = git(
                    plan,
                    &[
                        "clone".as_ref(),
                        "--quiet".as_ref(),
                        "--bare".as_ref(),
                        "--".as_ref(),
                        repo.repository_url.as_ref(),
                        staged.as_os_str(),
                    ],
                    repo.token_file.as_deref(),
                    &askpass,
                );
                if let Err(error) = clone {
                    if staged.exists() {
                        fs::remove_dir_all(&staged)?;
                    }
                    return Err(error);
                }
                fs::rename(staged, &destination)?;
            }
            ensure!(
                bytes_under(&cache)? <= plan.max_cache_bytes,
                "image_repository_cache_limit_exceeded"
            );
            let reference = format!("refs/heads/{}^{{commit}}", repo.default_branch);
            let commit = git(
                plan,
                &[
                    "-C".as_ref(),
                    destination.as_os_str(),
                    "rev-parse".as_ref(),
                    "--verify".as_ref(),
                    "--end-of-options".as_ref(),
                    reference.as_ref(),
                ],
                None,
                &askpass,
            )?;
            ensure!(
                (commit.len() == 40 || commit.len() == 64)
                    && commit.bytes().all(|b| b.is_ascii_hexdigit()),
                "image_source_commit_invalid"
            );
            git(
                plan,
                &[
                    "-C".as_ref(),
                    destination.as_os_str(),
                    "fsck".as_ref(),
                    "--no-dangling".as_ref(),
                ],
                None,
                &askpass,
            )?;
            prepared.push(PreparedRepository {
                project_id: repo.project_id.clone(),
                repository_identity: repo.repository_identity.clone(),
                path: destination,
                default_branch: repo.default_branch.clone(),
                source_commit: commit,
            });
        }
        progress(plan, "sanitizing", None)?;
        drop(tokens);
        clean_home(&plan.home)?;
        ensure!(
            fs::read_dir(&secrets)?.next().is_none(),
            "image_contains_unconsumed_secrets"
        );
        let manifest = ImageManifest {
            rust_launch_assets_version: Some(1),
            version: 1,
            build_id: plan.build_id.clone(),
            template_version: plan.template_version.clone(),
            configuration_digest: plan.configuration_digest.clone(),
            plan_digest: digest,
            runtime_rpc_protocol: zuse_protocol::cloud_gateway::RUST_RPC.into(),
            rust_runtime_storage_format: zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT.into(),
            rust_session_command_version: 1,
            credentials_removed: true,
            repositories: prepared,
        };
        progress(plan, "publishing", None)?;
        atomic_write(&ready, &serde_json::to_vec(&manifest)?)?;
        progress(plan, "ready", None)?;
        Ok(manifest)
    })();
    if result.is_err() {
        let _ = progress(plan, "failed", None);
    }
    result
}
#[cfg(test)]
mod tests;
