//! Repository snapshots made on the worker: a depth-1 fetch of the default
//! branch into a fresh bare repository, packed as a tar of `HEAD`, `shallow`,
//! `packed-refs`, `refs` and `objects` only — no config, remote, hook or
//! credential. Private repositories are fetched with a short-lived,
//! repository-scoped token the API mints for this build; it lives only in the
//! environment of that one Git process and never reaches the builder.
use super::repositories::{
    MAX_REPOSITORY_BYTES, PlannedRepository, RepositorySource, SourceError, SourceSnapshot,
};
use crate::error::ApiError;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroizing;

/// Longest one snapshot fetch may run; a worker tick has to finish its lease.
const FETCH_TIMEOUT: Duration = Duration::from_secs(70);
/// Leftover snapshot directories older than this are removed.
const STALE_SECS: u64 = 24 * 60 * 60;

/// Mints a fetch token for one private repository of one building image.
#[async_trait::async_trait]
pub trait RepositoryCredentials: Send + Sync {
    async fn token(&self, image_id: &str, project_id: &str) -> Result<Zeroizing<String>, ApiError>;
}

pub struct GitRepositorySource {
    root: PathBuf,
    credentials: Option<Arc<dyn RepositoryCredentials>>,
    origin: String,
}

impl GitRepositorySource {
    /// Snapshots live under `$TMPDIR/zuse-user-image-repos/<image>/<repo>`.
    pub fn new(credentials: Option<Arc<dyn RepositoryCredentials>>) -> Self {
        Self {
            root: std::env::temp_dir().join("zuse-user-image-repos"),
            credentials,
            origin: "https://github.com".into(),
        }
    }
    #[cfg(test)]
    pub(crate) fn for_test(root: PathBuf, origin: String) -> Self {
        Self {
            root,
            credentials: None,
            origin,
        }
    }
    fn reap(&self) {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return;
        };
        for entry in entries.flatten() {
            let old = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > STALE_SECS);
            if old {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }
}

fn directory_bytes(root: &Path) -> u64 {
    let mut pending = vec![root.to_path_buf()];
    let mut total = 0u64;
    while let Some(path) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Ok(meta) = std::fs::symlink_metadata(entry.path()) {
                if meta.is_dir() {
                    pending.push(entry.path());
                } else {
                    total = total.saturating_add(meta.len());
                }
            }
        }
    }
    total
}

/// Run Git with no user or system configuration. A token, when present, is
/// an HTTP header scoped to the GitHub origin, set through the environment of
/// this process only; it never appears in arguments, files or the snapshot.
async fn git(
    root: &Path,
    args: &[&str],
    header: Option<&str>,
    origin: &str,
    watch: Option<&Path>,
) -> Result<String, SourceError> {
    let mut command = tokio::process::Command::new("git");
    command
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "credential.helper=",
            "-c",
            "http.followRedirects=false",
            "-c",
            "protocol.ext.allow=never",
            "-c",
            "fetch.recurseSubmodules=false",
            "-c",
            "fetch.unpackLimit=1",
            "-c",
            "pack.threads=2",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    if !origin.starts_with("file://") {
        command.args(["-c", "protocol.file.allow=never"]);
    }
    if let Some(header) = header {
        command
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", format!("http.{origin}/.extraHeader"))
            .env("GIT_CONFIG_VALUE_0", header);
    }
    command.args(args);
    let mut child = command
        .spawn()
        .map_err(|_| SourceError::Retry("repository_git_unavailable"))?;
    let mut stdout = child.stdout.take().expect("piped");
    let reader = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut out = Vec::new();
        let _ = (&mut stdout).take(64 * 1024).read_to_end(&mut out).await;
        out
    });
    let deadline = tokio::time::Instant::now() + FETCH_TIMEOUT;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status.map_err(|_| SourceError::Retry("repository_fetch_failed"))?,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                if tokio::time::Instant::now() >= deadline {
                    let _ = child.kill().await;
                    return Err(SourceError::Retry("repository_fetch_timeout"));
                }
                if let Some(path) = watch {
                    let path = path.to_path_buf();
                    let bytes = tokio::task::spawn_blocking(move || directory_bytes(&path)).await.unwrap_or(0);
                    if bytes > MAX_REPOSITORY_BYTES {
                        let _ = child.kill().await;
                        return Err(SourceError::Skip("repository_too_large"));
                    }
                }
            }
        }
    };
    let out = reader.await.unwrap_or_default();
    if !status.success() {
        return Err(SourceError::Retry("repository_fetch_failed"));
    }
    String::from_utf8(out)
        .map(|s| s.trim().to_owned())
        .map_err(|_| SourceError::Retry("repository_fetch_failed"))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Receipt {
    commit: String,
    sha256: String,
    bytes: u64,
}

#[async_trait::async_trait]
impl RepositorySource for GitRepositorySource {
    async fn snapshot(
        &self,
        image_id: &str,
        _owner: &str,
        repository: &PlannedRepository,
    ) -> Result<SourceSnapshot, SourceError> {
        if !super::image_id_valid(image_id) {
            return Err(SourceError::Skip("repository_invalid"));
        }
        self.reap();
        let directory = self.root.join(image_id).join(repository.key());
        let file = directory.join("snapshot.tar");
        let receipt = directory.join("snapshot.json");
        // A snapshot made on an earlier tick of this build is reused as is.
        if let Ok(bytes) = std::fs::read(&receipt)
            && let Ok(made) = serde_json::from_slice::<Receipt>(&bytes)
            && std::fs::metadata(&file).is_ok_and(|m| m.len() == made.bytes)
        {
            return Ok(SourceSnapshot {
                commit: made.commit,
                sha256: made.sha256,
                bytes: made.bytes,
                file,
            });
        }
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory)
            .map_err(|_| SourceError::Retry("repository_staging_unavailable"))?;
        let header = if repository.visibility == "private" {
            let credentials = self
                .credentials
                .as_ref()
                .ok_or(SourceError::Skip("repository_access_unavailable"))?;
            let token = credentials
                .token(image_id, &repository.project_id)
                .await
                .map_err(|error| {
                    tracing::warn!(
                        code = error.code,
                        "User image repository credential refused"
                    );
                    if error.status.is_server_error() {
                        SourceError::Retry("repository_access_unavailable")
                    } else {
                        SourceError::Skip("repository_access_unavailable")
                    }
                })?;
            use base64::Engine as _;
            Some(Zeroizing::new(format!(
                "Authorization: Basic {}",
                base64::engine::general_purpose::STANDARD
                    .encode(format!("x-access-token:{}", token.as_str()))
            )))
        } else {
            None
        };
        let name = repository
            .repository_identity
            .strip_prefix("github.com/")
            .ok_or(SourceError::Skip("repository_invalid"))?;
        let url = format!("{}/{name}.git", self.origin);
        let repo = directory.join("repository.git");
        let repo_text = repo
            .to_str()
            .ok_or(SourceError::Retry("repository_staging_unavailable"))?;
        git(
            &directory,
            &["init", "--quiet", "--bare", repo_text],
            None,
            &self.origin,
            None,
        )
        .await?;
        let branch = &repository.default_branch;
        let refspec = format!("+refs/heads/{branch}:refs/heads/{branch}");
        let fetched = git(
            &repo,
            &[
                "fetch",
                "--quiet",
                "--depth=1",
                "--no-tags",
                "--",
                &url,
                &refspec,
            ],
            header.as_deref().map(|h| h.as_str()),
            &self.origin,
            Some(&repo),
        )
        .await;
        drop(header);
        fetched?;
        let commit = git(
            &repo,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("refs/heads/{branch}^{{commit}}"),
            ],
            None,
            &self.origin,
            None,
        )
        .await?;
        if !super::repositories::commit_valid(&commit) {
            return Err(SourceError::Retry("repository_fetch_failed"));
        }
        git(
            &repo,
            &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
            None,
            &self.origin,
            None,
        )
        .await?;
        git(&repo, &["pack-refs", "--all"], None, &self.origin, None).await?;
        let mut members = vec!["HEAD", "objects", "refs"];
        for optional in ["shallow", "packed-refs"] {
            if repo.join(optional).is_file() {
                members.push(optional);
            }
        }
        let status = tokio::process::Command::new("tar")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .arg("-C")
            .arg(&repo)
            .args([
                "--format=gnu",
                "--owner=0",
                "--group=0",
                "--numeric-owner",
                "-cf",
            ])
            .arg(&file)
            .args(&members)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|_| SourceError::Retry("repository_staging_unavailable"))?;
        let _ = std::fs::remove_dir_all(&repo);
        if !status.success() {
            return Err(SourceError::Retry("repository_staging_unavailable"));
        }
        let hashed = {
            let file = file.clone();
            tokio::task::spawn_blocking(move || -> std::io::Result<(String, u64)> {
                use std::io::Read;
                let mut handle = std::fs::File::open(file)?;
                let mut digest = Sha256::new();
                let mut buffer = vec![0; 1024 * 1024];
                let mut total = 0u64;
                loop {
                    let n = handle.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    total += n as u64;
                    digest.update(&buffer[..n]);
                }
                Ok((format!("{:x}", digest.finalize()), total))
            })
            .await
            .map_err(|_| SourceError::Retry("repository_staging_unavailable"))?
            .map_err(|_| SourceError::Retry("repository_staging_unavailable"))?
        };
        let (sha256, bytes) = hashed;
        if bytes > MAX_REPOSITORY_BYTES {
            return Err(SourceError::Skip("repository_too_large"));
        }
        let made = Receipt {
            commit: commit.to_ascii_lowercase(),
            sha256,
            bytes,
        };
        std::fs::write(
            &receipt,
            serde_json::to_vec(&made)
                .map_err(|_| SourceError::Retry("repository_staging_unavailable"))?,
        )
        .map_err(|_| SourceError::Retry("repository_staging_unavailable"))?;
        Ok(SourceSnapshot {
            commit: made.commit,
            sha256: made.sha256,
            bytes: made.bytes,
            file,
        })
    }
    fn discard(&self, image_id: &str) {
        if super::image_id_valid(image_id) {
            let _ = std::fs::remove_dir_all(self.root.join(image_id));
        }
    }
}
