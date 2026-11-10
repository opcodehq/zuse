//! Durable checkout checkpoints. Conversation documents and branches are retained.
use crate::EngineError;
mod review;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

#[derive(Clone)]
pub struct Archives {
    root: PathBuf,
    managed: PathBuf,
    changes: std::sync::Arc<tokio::sync::watch::Sender<u64>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Prepared,
    Removed,
    Restoring,
    Restored,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutArchive {
    pub version: u32,
    pub checkout: PathBuf,
    pub repo: PathBuf,
    pub common_dir: PathBuf,
    pub head: String,
    pub branch: Option<String>,
    pub checkpoint: String,
    pub reference: String,
    pub phase: Phase,
    pub changed_paths: Vec<String>,
    generation: String,
    retained: Vec<PathBuf>,
}
#[derive(Serialize, Deserialize)]
struct ManagedCheckout {
    checkout: PathBuf,
    repo: PathBuf,
    common_dir: PathBuf,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveOutcome {
    pub storage_reclaimed: bool,
    pub checkpoint: Option<String>,
    pub changed_paths: Vec<String>,
    pub notice: Option<String>,
}
impl ArchiveOutcome {
    pub(crate) fn retained(notice: &str) -> Self {
        Self {
            storage_reclaimed: false,
            checkpoint: None,
            changed_paths: vec![],
            notice: Some(notice.into()),
        }
    }
    fn checkpoint(record: &CheckoutArchive, notice: Option<String>) -> Self {
        Self {
            storage_reclaimed: record.phase == Phase::Removed,
            checkpoint: Some(record.checkpoint.clone()),
            changed_paths: record.changed_paths.clone(),
            notice,
        }
    }
}
fn error(message: impl Into<String>) -> EngineError {
    EngineError::Other(message.into())
}
fn key(path: &Path) -> String {
    format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()))
}
fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}
fn cache_path(path: &Path) -> bool {
    path.components().any(|part| {
        matches!(
            part.as_os_str().to_str(),
            Some(
                "node_modules"
                    | "target"
                    | "dist"
                    | "build"
                    | ".next"
                    | ".nuxt"
                    | ".cache"
                    | ".turbo"
                    | ".venv"
                    | "venv"
                    | "__pycache__"
                    | "coverage"
                    | ".gradle"
            )
        )
    })
}
async fn git(cwd: &Path, args: &[&OsStr], index: Option<&Path>) -> Result<Vec<u8>, EngineError> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.current_dir(cwd)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    let output = tokio::time::timeout(Duration::from_secs(120), cmd.output())
        .await
        .map_err(|_| {
            error("Archive Git operation timed out. The saved checkpoint is retained.")
        })??;
    if !output.status.success() {
        return Err(error(format!(
            "Archive Git operation: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}
async fn text(cwd: &Path, args: &[&str]) -> Result<String, EngineError> {
    Ok(
        String::from_utf8(git(cwd, &args.iter().map(OsStr::new).collect::<Vec<_>>(), None).await?)
            .map_err(|_| error("Git returned an invalid archive identity."))?
            .trim()
            .to_owned(),
    )
}
fn paths(bytes: &[u8]) -> Result<Vec<PathBuf>, EngineError> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|bytes| {
            #[cfg(unix)]
            let path = {
                use std::os::unix::ffi::OsStringExt;
                PathBuf::from(OsString::from_vec(bytes.to_vec()))
            };
            #[cfg(not(unix))]
            let path = PathBuf::from(
                String::from_utf8(bytes.to_vec())
                    .map_err(|_| error("A file name cannot be represented on this computer."))?,
            );
            if !safe_relative(&path) {
                return Err(error("Git returned a path outside the archived checkout."));
            }
            Ok(path)
        })
        .collect()
}
fn durable_json(path: &Path, value: &impl Serialize) -> Result<(), EngineError> {
    let parent = path
        .parent()
        .ok_or_else(|| error("Archive storage has no parent."))?;
    private_dir(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|err| error(err.to_string()))?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path)
        .map_err(|err| EngineError::Io(err.error))?;
    sync_dir(parent)?;
    Ok(())
}
fn private_dir(path: &Path) -> Result<(), EngineError> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn sync_dir(path: &Path) -> Result<(), EngineError> {
    #[cfg(unix)]
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}
fn digest(path: &Path) -> Result<Vec<u8>, EngineError> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 65536];
    loop {
        let size = file.read(&mut chunk)?;
        if size == 0 {
            break;
        }
        hash.update(&chunk[..size]);
    }
    Ok(hash.finalize().to_vec())
}
/// Merge only identical existing entries; never overwrite newer work during restore.
fn copy_tree(source: &Path, target: &Path) -> Result<(), EngineError> {
    let metadata = std::fs::symlink_metadata(source)?;
    if let Ok(existing) = std::fs::symlink_metadata(target) {
        if metadata.is_dir() && existing.is_dir() {
            for entry in std::fs::read_dir(source)? {
                let entry = entry?;
                copy_tree(&entry.path(), &target.join(entry.file_name()))?;
            }
            sync_dir(target)?;
            return Ok(());
        }
        let same = if metadata.file_type().is_symlink() && existing.file_type().is_symlink() {
            std::fs::read_link(source)? == std::fs::read_link(target)?
        } else if metadata.is_file() && existing.is_file() {
            metadata.len() == existing.len() && digest(source)? == digest(target)?
        } else {
            false
        };
        return if same {
            Ok(())
        } else {
            Err(error(format!(
                "Restore would overwrite {}. The archive is retained.",
                target.display()
            )))
        };
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if metadata.file_type().is_symlink() {
        let link = std::fs::read_link(source)?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(link, target)?;
        #[cfg(windows)]
        {
            if source.is_dir() {
                std::os::windows::fs::symlink_dir(link, target)?;
            } else {
                std::os::windows::fs::symlink_file(link, target)?;
            }
        }
    } else if metadata.is_dir() {
        std::fs::create_dir(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        std::fs::set_permissions(target, metadata.permissions())?;
        sync_dir(target)?;
    } else if metadata.is_file() {
        std::fs::copy(source, target)?;
        std::fs::File::open(target)?.sync_all()?;
    } else {
        return Err(error(format!(
            "Cannot preserve special file {}. Nothing was removed.",
            source.display()
        )));
    }
    Ok(())
}
fn verify_tree(source: &Path, saved: &Path) -> Result<(), EngineError> {
    let current = std::fs::symlink_metadata(source)?;
    let prior = std::fs::symlink_metadata(saved)?;
    let same = if current.file_type().is_symlink() && prior.file_type().is_symlink() {
        std::fs::read_link(source)? == std::fs::read_link(saved)?
    } else if current.is_file() && prior.is_file() {
        current.len() == prior.len() && digest(source)? == digest(saved)?
    } else if current.is_dir() && prior.is_dir() {
        let mut a = std::fs::read_dir(source)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut b = std::fs::read_dir(saved)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<Vec<_>, _>>()?;
        a.sort();
        b.sort();
        if a != b {
            false
        } else {
            for name in a {
                verify_tree(&source.join(&name), &saved.join(name))?;
            }
            true
        }
    } else {
        false
    };
    if !same {
        return Err(error(format!(
            "{} changed during archive. Nothing was removed; retry when writes stop.",
            source.display()
        )));
    }
    Ok(())
}
impl Archives {
    pub fn new(data_dir: &Path) -> Self {
        let data_dir = std::fs::canonicalize(data_dir).unwrap_or_else(|_| {
            if data_dir.is_absolute() {
                data_dir.to_path_buf()
            } else {
                std::env::current_dir()
                    .map(|cwd| cwd.join(data_dir))
                    .unwrap_or_else(|_| data_dir.to_path_buf())
            }
        });
        Self {
            root: data_dir.join("archives"),
            managed: data_dir.join("managed-worktrees"),
            changes: std::sync::Arc::new(tokio::sync::watch::channel(0).0),
        }
    }
    fn record_path(&self, checkout: &Path) -> PathBuf {
        self.root.join(key(checkout)).join("record.json")
    }
    pub(crate) fn retained_root(&self, record: &CheckoutArchive) -> PathBuf {
        self.root
            .join(key(&record.checkout))
            .join(&record.generation)
            .join("retained")
    }
    pub fn record(&self, checkout: &Path) -> Result<Option<CheckoutArchive>, EngineError> {
        let alias = self
            .root
            .join("aliases")
            .join(format!("{}.json", key(checkout)));
        let physical: PathBuf = match std::fs::read(alias) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| error("Archive checkout mapping is damaged."))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                std::fs::canonicalize(checkout).unwrap_or_else(|_| checkout.to_path_buf())
            }
            Err(err) => return Err(err.into()),
        };
        let checkout = physical.as_path();
        let path = self.record_path(checkout);
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let record: CheckoutArchive = serde_json::from_slice(&bytes).map_err(|_| {
            error("Archive metadata is damaged. The checkout and checkpoints were left untouched.")
        })?;
        if record.version != 1
            || record.checkout != checkout
            || !safe_relative(Path::new(&record.generation))
            || record.generation.contains(std::path::MAIN_SEPARATOR)
            || record.retained.iter().any(|path| !safe_relative(path))
        {
            return Err(error("Archive metadata does not match this checkout."));
        }
        Ok(Some(record))
    }
    /// Forget recovery material only after the checkout has been removed.
    pub(crate) async fn discard(&self, repo: &Path, checkout: &Path) -> Result<(), EngineError> {
        if checkout.exists() {
            return Err(error("Remove the worktree before discarding its archive."));
        }
        let record = self.record(checkout)?;
        let physical = record
            .as_ref()
            .map(|record| record.checkout.as_path())
            .unwrap_or(checkout);
        if let Some(record) = &record {
            if std::fs::canonicalize(repo)? != std::fs::canonicalize(&record.repo)? {
                return Err(error("The archive belongs to another project."));
            }
        }
        let prefix = format!("refs/zuse/archive/{}/", key(physical));
        let refs = text(repo, &["for-each-ref", "--format=%(refname)", &prefix]).await?;
        for reference in refs.lines() {
            if !reference.starts_with(&prefix) {
                return Err(error("Unexpected archive checkpoint reference."));
            }
            text(repo, &["update-ref", "-d", reference]).await?;
        }
        let root = self.root.clone();
        let managed = self.managed.join(format!("{}.json", key(physical)));
        let physical = physical.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<(), EngineError> {
            let aliases = root.join("aliases");
            match std::fs::read_dir(&aliases) {
                Ok(entries) => {
                    for entry in entries {
                        let path = entry?.path();
                        if path.extension() == Some(OsStr::new("json"))
                            && std::fs::read(&path)
                                .ok()
                                .and_then(|bytes| serde_json::from_slice::<PathBuf>(&bytes).ok())
                                .as_ref()
                                == Some(&physical)
                        {
                            std::fs::remove_file(path)?;
                        }
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            }
            for result in [
                std::fs::remove_dir_all(root.join(key(&physical))),
                std::fs::remove_file(managed),
            ] {
                if let Err(err) = result {
                    if err.kind() != std::io::ErrorKind::NotFound {
                        return Err(err.into());
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|err| error(err.to_string()))??;
        self.notify_change();
        Ok(())
    }
    async fn save(&self, record: &CheckoutArchive) -> Result<(), EngineError> {
        let record = record.clone();
        let path = self.record_path(&record.checkout);
        tokio::task::spawn_blocking(move || durable_json(&path, &record))
            .await
            .map_err(|err| error(err.to_string()))??;
        self.notify_change();
        Ok(())
    }
    pub(crate) fn notify_change(&self) {
        self.changes
            .send_modify(|revision| *revision = revision.wrapping_add(1));
    }
    pub(crate) fn watch_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changes.subscribe()
    }
    pub(crate) async fn remember_path(
        &self,
        path: &Path,
        physical: &Path,
    ) -> Result<(), EngineError> {
        if path == physical {
            return Ok(());
        }
        let file = self
            .root
            .join("aliases")
            .join(format!("{}.json", key(path)));
        let physical = physical.to_path_buf();
        tokio::task::spawn_blocking(move || durable_json(&file, &physical))
            .await
            .map_err(|err| error(err.to_string()))?
    }

    pub(crate) async fn register_managed(
        &self,
        repo: &Path,
        checkout: &Path,
    ) -> Result<(), EngineError> {
        let checkout = std::fs::canonicalize(checkout)?;
        let repo = std::fs::canonicalize(repo)?;
        let common_dir = PathBuf::from(
            text(
                &checkout,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?,
        );
        let marker = ManagedCheckout {
            checkout: checkout.clone(),
            repo,
            common_dir,
        };
        let path = self.managed.join(format!("{}.json", key(&checkout)));
        tokio::task::spawn_blocking(move || durable_json(&path, &marker))
            .await
            .map_err(|err| error(err.to_string()))?
    }
    pub(crate) async fn managed_checkout(
        &self,
        repo: &Path,
        checkout: &Path,
        configured_root: &Path,
    ) -> Result<bool, EngineError> {
        let checkout = std::fs::canonicalize(checkout)?;
        let repo = std::fs::canonicalize(repo)?;
        if checkout == repo
            || !std::fs::symlink_metadata(checkout.join(".git"))
                .is_ok_and(|metadata| metadata.is_file())
        {
            return Ok(false);
        }
        let common = PathBuf::from(
            text(
                &checkout,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?,
        );
        let expected = PathBuf::from(
            text(
                &repo,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?,
        );
        let git_dir = PathBuf::from(text(&checkout, &["rev-parse", "--absolute-git-dir"]).await?);
        if std::fs::canonicalize(&common)? != std::fs::canonicalize(expected)?
            || std::fs::canonicalize(&git_dir)? == std::fs::canonicalize(&common)?
        {
            return Ok(false);
        }
        let marker = self.managed.join(format!("{}.json", key(&checkout)));
        if let Ok(bytes) = std::fs::read(marker) {
            let saved: ManagedCheckout = serde_json::from_slice(&bytes)
                .map_err(|_| error("Managed worktree metadata is damaged."))?;
            return Ok(saved.checkout == checkout
                && saved.repo == repo
                && saved.common_dir == common);
        }
        if std::fs::canonicalize(configured_root).is_ok_and(|root| checkout.starts_with(root)) {
            self.register_managed(&repo, &checkout).await?;
            return Ok(true);
        }
        Ok(false)
    }
    async fn snapshot_tree(&self, checkout: &Path) -> Result<String, EngineError> {
        private_dir(&self.root)?;
        let temporary = tempfile::NamedTempFile::new_in(&self.root)?.into_temp_path();
        std::fs::remove_file(&temporary)?;
        let index = Some(temporary.as_ref());
        git(
            checkout,
            &[OsStr::new("read-tree"), OsStr::new("HEAD")],
            index,
        )
        .await?;
        git(
            checkout,
            &[
                OsStr::new("add"),
                OsStr::new("-A"),
                OsStr::new("--"),
                OsStr::new("."),
            ],
            index,
        )
        .await?;
        let tree = git(checkout, &[OsStr::new("write-tree")], index).await?;
        Ok(String::from_utf8(tree)
            .map_err(|_| error("Invalid checkpoint tree."))?
            .trim()
            .into())
    }
    async fn retained_paths(&self, checkout: &Path) -> Result<Vec<PathBuf>, EngineError> {
        let bytes = git(
            checkout,
            &[
                OsStr::new("ls-files"),
                OsStr::new("--others"),
                OsStr::new("--ignored"),
                OsStr::new("--exclude-standard"),
                OsStr::new("--directory"),
                OsStr::new("-z"),
            ],
            None,
        )
        .await?;
        let mut found = paths(&bytes)?
            .into_iter()
            .filter(|path| !cache_path(path))
            .collect::<Vec<_>>();
        if std::fs::symlink_metadata(checkout.join(".context")).is_ok() {
            found.push(PathBuf::from(".context"));
        }
        found.sort();
        found.dedup();
        let mut roots: Vec<PathBuf> = vec![];
        for path in found {
            if !roots.iter().any(|root| path.starts_with(root)) {
                roots.push(path);
            }
        }
        Ok(roots)
    }
    pub(crate) async fn prepare(
        &self,
        repo: &Path,
        checkout: &Path,
    ) -> Result<CheckoutArchive, EngineError> {
        let checkout = std::fs::canonicalize(checkout)?;
        let repo = std::fs::canonicalize(repo)?;
        if self.root.starts_with(&checkout) || checkout.starts_with(&self.root) {
            return Err(error(
                "Archive storage must be outside the checkout being removed.",
            ));
        }
        if !text(&checkout, &["diff", "--name-only", "--diff-filter=U"])
            .await?
            .is_empty()
        {
            return Err(error(
                "Resolve merge conflicts before archiving this worktree.",
            ));
        }
        let head = text(&checkout, &["rev-parse", "HEAD"]).await?;
        let branch = text(&checkout, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .await
            .ok();
        let tree = self.snapshot_tree(&checkout).await?;
        let staged = text(&checkout, &["ls-tree", "-r", &tree]).await?;
        if staged.lines().any(|line| line.starts_with("160000 ")) {
            return Err(error(
                "Archive the nested repositories or submodules separately before removing this checkout.",
            ));
        }
        let generation = crate::new_id();
        let reference = format!("refs/zuse/archive/{}/{}", key(&checkout), generation);
        let message = format!(
            "Zuse archive checkpoint\n\nZuse-Archive-Checkout: {}",
            key(&checkout)
        );
        let checkpoint = text(
            &checkout,
            &[
                "-c",
                "user.name=Zuse",
                "-c",
                "user.email=zuse@localhost",
                "commit-tree",
                &tree,
                "-p",
                &head,
                "-m",
                &message,
            ],
        )
        .await?;
        text(&repo, &["update-ref", &reference, &checkpoint]).await?;
        let changed = git(
            &repo,
            &[
                OsStr::new("diff"),
                OsStr::new("--name-only"),
                OsStr::new("-z"),
                OsStr::new(&head),
                OsStr::new(&checkpoint),
            ],
            None,
        )
        .await?;
        let common_dir = PathBuf::from(
            text(
                &checkout,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?,
        );
        let retained = self.retained_paths(&checkout).await?;
        let record = CheckoutArchive {
            version: 1,
            checkout,
            repo,
            common_dir,
            head,
            branch,
            checkpoint,
            reference,
            phase: Phase::Prepared,
            changed_paths: paths(&changed)?
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            generation,
            retained,
        };
        let saved = self.retained_root(&record);
        let source = record.checkout.clone();
        let entries = record.retained.clone();
        tokio::task::spawn_blocking(move || {
            private_dir(&saved)?;
            for path in entries {
                copy_tree(&source.join(&path), &saved.join(path))?;
            }
            sync_dir(&saved)
        })
        .await
        .map_err(|err| error(err.to_string()))??;
        self.save(&record).await?;
        Ok(record)
    }
    pub(crate) async fn remove(
        &self,
        record: &mut CheckoutArchive,
    ) -> Result<ArchiveOutcome, EngineError> {
        if record.phase != Phase::Prepared {
            return Err(error("The checkout is not ready for archive cleanup."));
        }
        if text(&record.checkout, &["rev-parse", "HEAD"]).await? != record.head
            || self.snapshot_tree(&record.checkout).await?
                != text(
                    &record.repo,
                    &["rev-parse", &format!("{}^{{tree}}", record.checkpoint)],
                )
                .await?
        {
            return Err(error(
                "The checkout changed during archive. Its files were kept; retry after writes stop.",
            ));
        }
        if self.retained_paths(&record.checkout).await? != record.retained {
            return Err(error(
                "Ignored files changed during archive. Its files were kept.",
            ));
        }
        let source = record.checkout.clone();
        let saved = self.retained_root(record);
        let entries = record.retained.clone();
        tokio::task::spawn_blocking(move || {
            for path in entries {
                verify_tree(&source.join(&path), &saved.join(path))?;
            }
            Ok::<_, EngineError>(())
        })
        .await
        .map_err(|err| error(err.to_string()))??;
        git(
            &record.repo,
            &[
                OsStr::new("worktree"),
                OsStr::new("remove"),
                OsStr::new("--force"),
                OsStr::new("--"),
                record.checkout.as_os_str(),
            ],
            None,
        )
        .await?;
        record.phase = Phase::Removed;
        self.save(record).await?;
        Ok(ArchiveOutcome::checkpoint(record, None))
    }
    async fn verify_checkout(&self, record: &CheckoutArchive) -> Result<(), EngineError> {
        if std::fs::canonicalize(&record.checkout)? != record.checkout {
            return Err(error("The restore path now points somewhere else."));
        }
        let common = PathBuf::from(
            text(
                &record.checkout,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .await?,
        );
        if std::fs::canonicalize(common)? != std::fs::canonicalize(&record.common_dir)? {
            return Err(error("The restore path belongs to another repository."));
        }
        Ok(())
    }
    pub(crate) async fn restore(
        &self,
        record: &mut CheckoutArchive,
    ) -> Result<ArchiveOutcome, EngineError> {
        if std::fs::symlink_metadata(&record.checkout).is_ok() {
            match record.phase {
                Phase::Prepared | Phase::Restored => {
                    self.verify_checkout(record).await?;
                    record.phase = Phase::Restored;
                    self.save(record).await?;
                    return Ok(ArchiveOutcome::checkpoint(record, None));
                }
                Phase::Removed => {
                    return Err(error(
                        "The restore path is occupied. Move that folder first; the archive has been kept.",
                    ));
                }
                Phase::Restoring => {
                    self.verify_checkout(record).await?;
                }
            }
        } else {
            let found = text(&record.repo, &["rev-parse", "--verify", &record.reference]).await?;
            if found != record.checkpoint {
                return Err(error(
                    "The archive checkpoint reference changed. Nothing was restored.",
                ));
            }
            let mut use_branch = if let Some(branch) = &record.branch {
                text(
                    &record.repo,
                    &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
                )
                .await
                .is_ok_and(|head| head == record.head)
            } else {
                false
            };
            if use_branch {
                let occupied = format!("branch refs/heads/{}", record.branch.as_deref().unwrap());
                use_branch = !text(&record.repo, &["worktree", "list", "--porcelain"])
                    .await?
                    .lines()
                    .any(|line| line == occupied);
            }
            record.phase = Phase::Restoring;
            self.save(record).await?;
            let mut args = vec![OsStr::new("worktree"), OsStr::new("add")];
            if !use_branch {
                args.push(OsStr::new("--detach"));
            }
            args.push(OsStr::new("--"));
            args.push(record.checkout.as_os_str());
            args.push(OsStr::new(if use_branch {
                record.branch.as_deref().unwrap()
            } else {
                &record.head
            }));
            git(&record.repo, &args, None).await?;
        }
        self.verify_checkout(record).await?;
        let head = text(&record.checkout, &["rev-parse", "HEAD"]).await?;
        if head != record.head {
            return Err(error(
                "The restored checkout advanced. The checkpoint is retained; no files were overwritten.",
            ));
        }
        let working = self.snapshot_tree(&record.checkout).await?;
        let base = text(
            &record.repo,
            &["rev-parse", &format!("{}^{{tree}}", record.head)],
        )
        .await?;
        let saved_tree = text(
            &record.repo,
            &["rev-parse", &format!("{}^{{tree}}", record.checkpoint)],
        )
        .await?;
        if working != base && working != saved_tree {
            return Err(error(
                "The incomplete restore contains newer edits. Keep them before retrying; the archive is retained.",
            ));
        }
        if working != saved_tree {
            text(
                &record.checkout,
                &["read-tree", "-m", "-u", "HEAD", &record.checkpoint],
            )
            .await?;
        }
        text(&record.checkout, &["reset", "--mixed", "HEAD"]).await?;
        let source = self.retained_root(record);
        let target = record.checkout.clone();
        let entries = record.retained.clone();
        tokio::task::spawn_blocking(move || {
            for path in entries {
                copy_tree(&source.join(&path), &target.join(path))?;
            }
            Ok::<_, EngineError>(())
        })
        .await
        .map_err(|err| error(err.to_string()))??;
        record.phase = Phase::Restored;
        self.save(record).await?;
        let actual_branch = text(
            &record.checkout,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
        )
        .await
        .ok();
        let notice=(actual_branch!=record.branch).then(||"The saved branch changed or is in use. Your files were restored in a detached checkout at the saved commit.".into());
        Ok(ArchiveOutcome::checkpoint(record, notice))
    }
}

impl Archives {
    /// Resolve original attachment paths without widening the live workspace jail.
    pub(crate) async fn attachment(
        &self,
        path: &Path,
    ) -> Result<Option<(PathBuf, PathBuf)>, EngineError> {
        for checkout in path.ancestors().skip(1) {
            let Some(record) = self.record(checkout)? else {
                continue;
            };
            if record.phase == Phase::Restored {
                return Ok(None);
            }
            let relative = path
                .strip_prefix(checkout)
                .map_err(|_| error("Invalid archived attachment path."))?;
            if !safe_relative(relative) {
                return Err(error("Invalid archived attachment path."));
            }
            let retained_root = self.retained_root(&record);
            let retained = retained_root.join(relative);
            if retained.is_file() {
                let root = std::fs::canonicalize(&retained_root)?;
                let resolved = std::fs::canonicalize(&retained)?;
                if !resolved.starts_with(&root) {
                    return Err(error(
                        "Archived attachment resolves outside its saved checkout.",
                    ));
                }
                return Ok(Some((resolved, root)));
            }
            let cache_root = self
                .root
                .join(key(checkout))
                .join(&record.generation)
                .join("materialized");
            private_dir(&cache_root)?;
            let destination = cache_root.join(relative);
            if !destination.exists() {
                if let Some(parent) = destination.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let file = tempfile::NamedTempFile::new_in(destination.parent().unwrap())?;
                let mut object = OsString::from(format!("{}:", record.checkpoint));
                #[cfg(unix)]
                object.push(relative.as_os_str());
                #[cfg(not(unix))]
                object.push(relative.to_string_lossy().replace('\\', "/"));
                let mut command = tokio::process::Command::new("git");
                command
                    .current_dir(&record.repo)
                    .arg("show")
                    .arg(object)
                    .stdin(Stdio::null())
                    .stdout(Stdio::from(file.as_file().try_clone()?))
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
                let output = tokio::time::timeout(
                    Duration::from_secs(120),
                    command.spawn()?.wait_with_output(),
                )
                .await
                .map_err(|_| error("Archived attachment read timed out."))??;
                if !output.status.success() {
                    return Err(error("This file is not in the saved checkout."));
                }
                file.as_file().sync_all()?;
                if let Err(err) = file.persist_noclobber(&destination) {
                    if err.error.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(err.error.into());
                    }
                }
            }
            let root = std::fs::canonicalize(cache_root)?;
            let resolved = std::fs::canonicalize(destination)?;
            if !resolved.starts_with(&root) {
                return Err(error("Archived attachment cache path is invalid."));
            }
            return Ok(Some((resolved, root)));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        _temp: tempfile::TempDir,
        repo: PathBuf,
        checkout: PathBuf,
        data: PathBuf,
        archives: Archives,
        branch: String,
    }
    impl Fixture {
        async fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let repo = temp.path().join("project");
            let data = temp.path().join("data");
            let root = temp.path().join("worktrees");
            std::fs::create_dir_all(&repo).unwrap();
            text(&repo, &["init", "-b", "main"]).await.unwrap();
            std::fs::write(repo.join("tracked.txt"), "original\n").unwrap();
            std::fs::write(repo.join("deleted.txt"), "keep in HEAD\n").unwrap();
            std::fs::write(repo.join(".gitignore"), "node_modules/\n.context/\n.env\n").unwrap();
            text(&repo, &["add", "."]).await.unwrap();
            text(
                &repo,
                &[
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@localhost",
                    "commit",
                    "-m",
                    "Initial files",
                ],
            )
            .await
            .unwrap();
            let repos = crate::Repos::with_worktrees_root(&data, "host", root);
            let worktree = repos.create_worktree(&repo, "HEAD").await.unwrap();
            let checkout = PathBuf::from(&worktree.path);
            Self {
                _temp: temp,
                repo,
                checkout,
                data: data.clone(),
                archives: Archives::new(&data),
                branch: worktree.branch,
            }
        }
        fn edit(&self) {
            std::fs::write(self.checkout.join("tracked.txt"), "changed\n").unwrap();
            std::fs::remove_file(self.checkout.join("deleted.txt")).unwrap();
            std::fs::write(self.checkout.join("new.bin"), [0, 1, 255, 7]).unwrap();
            std::fs::create_dir_all(self.checkout.join(".context")).unwrap();
            std::fs::write(self.checkout.join(".context/image.png"), [137, 80, 78, 71]).unwrap();
            std::fs::write(self.checkout.join(".env"), "LOCAL_VALUE=preserved\n").unwrap();
            std::fs::create_dir_all(self.checkout.join("node_modules/cache")).unwrap();
            std::fs::write(
                self.checkout.join("node_modules/cache/rebuildable"),
                vec![42; 65536],
            )
            .unwrap();
        }
        async fn remove(&self) -> CheckoutArchive {
            let mut record = self
                .archives
                .prepare(&self.repo, &self.checkout)
                .await
                .unwrap();
            self.archives.remove(&mut record).await.unwrap();
            record
        }
    }
    #[tokio::test]
    async fn archive_roundtrip_preserves_dirty_files_context_and_index_without_moving_branch() {
        let f = Fixture::new().await;
        f.edit();
        let index = PathBuf::from(
            text(
                &f.checkout,
                &["rev-parse", "--path-format=absolute", "--git-path", "index"],
            )
            .await
            .unwrap(),
        );
        let original_index = std::fs::read(&index).unwrap();
        let head = text(&f.checkout, &["rev-parse", "HEAD"]).await.unwrap();
        let mut record = f.archives.prepare(&f.repo, &f.checkout).await.unwrap();
        assert_eq!(std::fs::read(index).unwrap(), original_index);
        assert!(record.changed_paths.contains(&"tracked.txt".into()));
        assert!(record.changed_paths.contains(&"deleted.txt".into()));
        assert!(record.changed_paths.contains(&"new.bin".into()));
        f.archives.remove(&mut record).await.unwrap();
        assert!(!f.checkout.exists());
        assert_eq!(
            text(&f.repo, &["rev-parse", &format!("refs/heads/{}", f.branch)])
                .await
                .unwrap(),
            head
        );
        let reopened = Archives::new(&f.data);
        let mut record = reopened.record(&f.checkout).unwrap().unwrap();
        assert_eq!(record.phase, Phase::Removed);
        let (image, root) = reopened
            .attachment(&f.checkout.join(".context/image.png"))
            .await
            .unwrap()
            .unwrap();
        assert!(image.starts_with(root));
        assert_eq!(std::fs::read(image).unwrap(), [137, 80, 78, 71]);
        let (binary, _) = reopened
            .attachment(&f.checkout.join("new.bin"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(binary).unwrap(), [0, 1, 255, 7]);
        reopened.restore(&mut record).await.unwrap();
        assert_eq!(
            text(&f.checkout, &["rev-parse", "HEAD"]).await.unwrap(),
            head
        );
        assert_eq!(
            std::fs::read_to_string(f.checkout.join("tracked.txt")).unwrap(),
            "changed\n"
        );
        assert!(!f.checkout.join("deleted.txt").exists());
        assert_eq!(
            std::fs::read(f.checkout.join("new.bin")).unwrap(),
            [0, 1, 255, 7]
        );
        assert_eq!(
            std::fs::read_to_string(f.checkout.join(".env")).unwrap(),
            "LOCAL_VALUE=preserved\n"
        );
        assert_eq!(
            std::fs::read(f.checkout.join(".context/image.png")).unwrap(),
            [137, 80, 78, 71]
        );
        assert!(!f.checkout.join("node_modules").exists());
        text(&f.checkout, &["diff", "--cached", "--quiet"])
            .await
            .unwrap();
        assert_eq!(
            text(&f.repo, &["rev-parse", &record.reference])
                .await
                .unwrap(),
            record.checkpoint
        );
    }
    #[tokio::test]
    async fn archive_restore_refuses_occupied_path_and_retains_checkpoint() {
        let f = Fixture::new().await;
        f.edit();
        let mut record = f.remove().await;
        std::fs::create_dir(&f.checkout).unwrap();
        std::fs::write(f.checkout.join("new-owner.txt"), "do not replace").unwrap();
        assert!(
            f.archives
                .restore(&mut record)
                .await
                .unwrap_err()
                .to_string()
                .contains("occupied")
        );
        assert_eq!(
            std::fs::read_to_string(f.checkout.join("new-owner.txt")).unwrap(),
            "do not replace"
        );
        assert_eq!(
            text(&f.repo, &["rev-parse", &record.reference])
                .await
                .unwrap(),
            record.checkpoint
        );
    }
    #[tokio::test]
    async fn archive_restore_detaches_when_saved_branch_advanced_without_losing_dirty_files() {
        let f = Fixture::new().await;
        f.edit();
        let mut record = f.remove().await;
        std::fs::write(f.repo.join("other.txt"), "new branch work").unwrap();
        text(&f.repo, &["add", "."]).await.unwrap();
        text(
            &f.repo,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@localhost",
                "commit",
                "-m",
                "Advance repository",
            ],
        )
        .await
        .unwrap();
        let advanced = text(&f.repo, &["rev-parse", "HEAD"]).await.unwrap();
        text(
            &f.repo,
            &["update-ref", &format!("refs/heads/{}", f.branch), &advanced],
        )
        .await
        .unwrap();
        let restored = f.archives.restore(&mut record).await.unwrap();
        assert!(restored.notice.unwrap().contains("detached"));
        assert_eq!(
            text(&f.repo, &["rev-parse", &format!("refs/heads/{}", f.branch)])
                .await
                .unwrap(),
            advanced
        );
        assert_eq!(
            text(&f.checkout, &["rev-parse", "HEAD"]).await.unwrap(),
            record.head
        );
        assert_eq!(
            std::fs::read_to_string(f.checkout.join("tracked.txt")).unwrap(),
            "changed\n"
        );
    }
    #[tokio::test]
    async fn archive_journal_recovers_removal_before_phase_save_and_rejects_changed_snapshot() {
        let f = Fixture::new().await;
        f.edit();
        let mut record = f.archives.prepare(&f.repo, &f.checkout).await.unwrap();
        std::fs::write(f.checkout.join("tracked.txt"), "newer edits").unwrap();
        assert!(f.archives.remove(&mut record).await.is_err());
        assert!(f.checkout.exists());
        let mut record = f.archives.prepare(&f.repo, &f.checkout).await.unwrap();
        let prepared = record.clone();
        f.archives.remove(&mut record).await.unwrap();
        f.archives.save(&prepared).await.unwrap();
        let mut recovered = Archives::new(&f.data).record(&f.checkout).unwrap().unwrap();
        f.archives.restore(&mut recovered).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(f.checkout.join("tracked.txt")).unwrap(),
            "newer edits"
        );
    }
    #[tokio::test]
    async fn force_delete_discards_prepared_archive_and_simulator_state() {
        use serde_json::json;
        use zuse_rpc::{RpcService, methods};
        let f = Fixture::new().await;
        f.edit();
        let mut record = f.archives.prepare(&f.repo, &f.checkout).await.unwrap();
        let alias = f._temp.path().join("archive-alias");
        f.archives.remember_path(&alias, &f.checkout).await.unwrap();
        std::fs::write(f.checkout.join("tracked.txt"), "changed after preparation").unwrap();
        assert!(f.archives.remove(&mut record).await.is_err());
        assert!(f.archives.retained_root(&record).exists());
        assert!(f.archives.discard(&f.repo, &f.checkout).await.is_err());
        let mut core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(&f.data).unwrap(),
            std::sync::Arc::new(crate::HarnessRegistry::new()),
            zuse_protocol::HarnessId::Mock,
            None,
        )
        .unwrap();
        core.workspace
            .create_space(
                "project",
                &core.device_id,
                &f.repo.to_string_lossy(),
                None,
                true,
            )
            .unwrap();
        core.simulators = zuse_simulator::Simulators::default();
        let rpc = core.rpc_service();
        rpc.handle(methods::MUTATE, json!({"op":"createChat","chatId":"prepared","spaceId":"project","cwd":f.checkout,"config":{"harness":"mock","sandbox":"workspace-write"}})).await.unwrap();
        core.simulators.configure("prepared", true, false).await;
        assert_eq!(core.simulators.preferences("prepared"), (true, false));
        rpc.handle(
            methods::MUTATE,
            json!({"op":"forceDeleteChat","chatId":"prepared"}),
        )
        .await
        .unwrap();
        assert!(!f.checkout.exists());
        assert!(f.archives.record(&f.checkout).unwrap().is_none());
        assert!(!f.archives.root.join(key(&f.checkout)).exists());
        assert!(
            !f.archives
                .root
                .join("aliases")
                .join(format!("{}.json", key(&alias)))
                .exists()
        );
        assert!(
            text(&f.repo, &["show-ref", "--verify", &record.reference])
                .await
                .is_err()
        );
        assert_eq!(core.simulators.preferences("prepared"), (false, false));
        // A missing, already archived checkout keeps its checkpoint until the
        // last sharing conversation is permanently deleted.
        let repos = crate::Repos::with_worktrees_root(
            &f.data,
            "host",
            f._temp.path().join("saved-worktrees"),
        );
        let saved = PathBuf::from(repos.create_worktree(&f.repo, "HEAD").await.unwrap().path);
        let mut archived = f.archives.prepare(&f.repo, &saved).await.unwrap();
        f.archives.remove(&mut archived).await.unwrap();
        for id in ["saved", "saved-shared"] {
            rpc.handle(methods::MUTATE, json!({"op":"createChat","chatId":id,"spaceId":"project","cwd":saved,"config":{"harness":"mock","sandbox":"workspace-write"}})).await.unwrap();
        }
        rpc.handle(
            methods::MUTATE,
            json!({"op":"forceDeleteChat","chatId":"saved"}),
        )
        .await
        .unwrap();
        assert!(f.archives.record(&saved).unwrap().is_some());
        rpc.handle(
            methods::MUTATE,
            json!({"op":"forceDeleteChat","chatId":"saved-shared"}),
        )
        .await
        .unwrap();
        assert!(f.archives.record(&saved).unwrap().is_none());
        core.sessions.shutdown().await;
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn force_delete_rpc_removes_socket_worktree_but_preserves_shared_and_main_folders() {
        use serde_json::json;
        use zuse_rpc::{RpcService, methods};
        let f = Fixture::new().await;
        f.edit();
        let _socket = std::os::unix::net::UnixListener::bind(
            f.checkout.join(".context/appshot-activation.sock"),
        )
        .unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(&f.data).unwrap(),
            std::sync::Arc::new(crate::HarnessRegistry::new()),
            zuse_protocol::HarnessId::Mock,
            None,
        )
        .unwrap();
        core.workspace
            .create_space(
                "project",
                &core.device_id,
                &f.repo.to_string_lossy(),
                None,
                true,
            )
            .unwrap();
        let rpc = core.rpc_service();
        for (id, cwd) in [
            ("first", &f.checkout),
            ("shared", &f.checkout),
            ("main", &f.repo),
        ] {
            rpc.handle(methods::MUTATE, json!({"op":"createChat","chatId":id,"spaceId":"project","cwd":cwd,"config":{"harness":"mock","sandbox":"workspace-write"}})).await.unwrap();
        }
        assert!(
            rpc.handle(
                methods::MUTATE,
                json!({"op":"forceDeleteChat","chatId":"first","targetDeviceId":"wrong-host"})
            )
            .await
            .is_err()
        );
        rpc.handle(
            methods::MUTATE,
            json!({"op":"forceDeleteChat","chatId":"shared"}),
        )
        .await
        .unwrap();
        assert!(f.checkout.exists());
        assert!(core.workspace.chat("first").unwrap().is_some());
        assert!(core.workspace.chat("shared").unwrap().is_none());
        let error = rpc
            .handle(
                methods::MUTATE,
                json!({"op":"setChatArchived","chatId":"first","archived":true}),
            )
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("Cannot preserve special file"));
        // Deleting a side chat keeps its parent's worktree; deleting the parent
        // also removes any remaining child conversations.
        for id in ["side", "agent"] {
            rpc.handle(methods::MUTATE, json!({"op":"createChat","chatId":id,"spaceId":"project","cwd":f.checkout,"parentChatId":"first","config":{"harness":"mock","sandbox":"workspace-write"}})).await.unwrap();
        }
        rpc.handle(
            methods::MUTATE,
            json!({"op":"forceDeleteChat","chatId":"side"}),
        )
        .await
        .unwrap();
        assert!(f.checkout.exists());
        assert!(core.workspace.chat("side").unwrap().is_none());
        rpc.handle(
            methods::MUTATE,
            json!({"op":"forceDeleteChat","chatId":"first"}),
        )
        .await
        .unwrap();
        assert!(!f.checkout.exists());
        assert!(core.workspace.chat("first").unwrap().is_none());
        assert!(core.workspace.chat("agent").unwrap().is_none());
        assert!(f.archives.record(&f.checkout).unwrap().is_none());
        rpc.handle(
            methods::MUTATE,
            json!({"op":"forceDeleteChat","chatId":"main"}),
        )
        .await
        .unwrap();
        assert!(f.repo.join("tracked.txt").exists());
        assert!(core.workspace.chat("main").unwrap().is_none());
        core.sessions.shutdown().await;
    }

    #[tokio::test]
    async fn archive_rpc_reclaims_only_after_last_shared_thread_and_restores_before_activation() {
        use serde_json::json;
        use zuse_rpc::{RpcReply, RpcService, methods};
        let f = Fixture::new().await;
        f.edit();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(&f.data).unwrap(),
            std::sync::Arc::new(crate::HarnessRegistry::new()),
            zuse_protocol::HarnessId::Mock,
            None,
        )
        .unwrap();
        core.workspace
            .create_space(
                "project",
                &core.device_id,
                &f.repo.to_string_lossy(),
                None,
                true,
            )
            .unwrap();
        let rpc = core.rpc_service();
        let first_checkout = {
            #[cfg(unix)]
            {
                let alias = f._temp.path().join("checkout-alias");
                std::os::unix::fs::symlink(&f.checkout, &alias).unwrap();
                alias
            }
            #[cfg(not(unix))]
            {
                f.checkout.clone()
            }
        };
        for (id, cwd) in [("first", &first_checkout), ("second", &f.checkout)] {
            rpc.handle(methods::MUTATE,json!({"op":"createChat","chatId":id,"spaceId":"project","cwd":cwd,"config":{"harness":"mock","sandbox":"workspace-write"}})).await.unwrap();
        }
        let handle = core.doc_host.open("first").unwrap();
        handle
            .doc()
            .push_message(&zuse_doc::SessionMessageEntry {
                id: "message".into(),
                role: zuse_doc::MessageRole::Assistant,
                parts: vec![zuse_doc::MessagePart::Text {
                    id: "text".into(),
                    text: "Keep this conversation".into(),
                }],
                created_at: 0,
                device_id: core.device_id.clone(),
                status: Some(zuse_doc::MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        assert!(rpc.handle(methods::MUTATE,json!({"op":"setChatArchived","chatId":"first","archived":true,"targetDeviceId":"wrong-host"})).await.is_err());
        assert!(!core.workspace.chat("first").unwrap().unwrap().archived);
        let RpcReply::Value(first) = rpc
            .handle(
                methods::MUTATE,
                json!({"op":"setChatArchived","chatId":"first","archived":true}),
            )
            .await
            .unwrap()
        else {
            panic!("archive must return receipt")
        };
        assert_eq!(first["archive"]["storageReclaimed"], false);
        assert!(f.checkout.exists());
        use futures::StreamExt;
        let RpcReply::Stream(mut review_watch) = rpc
            .handle(methods::WATCH_CHECKOUT_DIFFS, json!({"chatId":"first"}))
            .await
            .unwrap()
        else {
            panic!("expected review stream")
        };
        review_watch.next().await.unwrap();
        let RpcReply::Value(second) = rpc
            .handle(
                methods::MUTATE,
                json!({"op":"setChatArchived","chatId":"second","archived":true}),
            )
            .await
            .unwrap()
        else {
            panic!("archive must return receipt")
        };
        assert_eq!(second["archive"]["storageReclaimed"], true);
        assert!(!f.checkout.exists());
        let RpcReply::Value(status) = rpc
            .handle(methods::GET_CHAT_ARCHIVE_STATUS, json!({"chatId":"first"}))
            .await
            .unwrap()
        else {
            panic!("archive status must be unary")
        };
        assert_eq!(status["phase"], "removed");
        assert_eq!(status["storageReclaimed"], true);
        assert!(
            status["changedPaths"]
                .as_array()
                .unwrap()
                .contains(&json!("tracked.txt"))
        );
        let RpcReply::Value(review) = rpc
            .handle(
                methods::GET_CHECKOUT_DIFF,
                json!({"chatId":"first","cwd":first_checkout,"mode":"workingTree"}),
            )
            .await
            .unwrap()
        else {
            panic!("expected saved diff")
        };
        assert!(review["patch"].as_str().unwrap().contains("+changed"));
        assert!(
            review["files"]
                .as_array()
                .unwrap()
                .iter()
                .any(|file| file["path"] == "new.bin")
        );
        let file_request = json!({"chatId":"first","cwd":review["cwd"],"checkoutId":review["checkoutId"],"path":"tracked.txt","mode":"workingTree","diffChecksum":review["checksum"]});
        let RpcReply::Value(source) = rpc
            .handle(methods::GET_CHECKOUT_FILE_DIFF_TEXT, file_request.clone())
            .await
            .unwrap()
        else {
            panic!("expected saved source")
        };
        assert_eq!(source["oldText"], "original\n");
        assert_eq!(source["newText"], "changed\n");
        let mut stale = file_request.clone();
        stale["diffChecksum"] = json!("old");
        let RpcReply::Value(source) = rpc
            .handle(methods::GET_CHECKOUT_FILE_DIFF_TEXT, stale)
            .await
            .unwrap()
        else {
            panic!("expected stale source")
        };
        assert_eq!(source["stale"], true);
        let mut outside = file_request;
        outside["path"] = json!("../outside");
        assert!(
            rpc.handle(methods::GET_CHECKOUT_FILE_DIFF_TEXT, outside)
                .await
                .is_err()
        );
        let mut mismatch = json!({"chatId":"first","cwd":f.repo,"mode":"workingTree"});
        assert!(
            rpc.handle(methods::GET_CHECKOUT_DIFF, mismatch.clone())
                .await
                .is_err()
        );
        mismatch["cwd"] = json!(first_checkout);
        mismatch["mode"] = json!("branch");
        mismatch["baseRef"] = json!("main");
        let RpcReply::Value(branch) = rpc
            .handle(methods::GET_CHECKOUT_DIFF, mismatch)
            .await
            .unwrap()
        else {
            panic!("expected branch diff")
        };
        assert!(branch["patch"].as_str().unwrap().contains("+changed"));
        let RpcReply::Value(branches) = rpc
            .handle(
                methods::LIST_BRANCHES,
                json!({"chatId":"first","repoPath":first_checkout}),
            )
            .await
            .unwrap()
        else {
            panic!("expected branches")
        };
        assert!(branches.as_array().unwrap().contains(&json!("main")));
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let frame = review_watch.next().await.unwrap();
                assert!(frame.get("error").is_none(), "{frame}");
                if frame
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|diff| diff["checksum"] == review["checksum"])
                {
                    break;
                }
            }
        })
        .await
        .expect("archive completion must wake an existing review viewer");
        assert!(
            !f.checkout.exists(),
            "review must not recreate the checkout"
        );
        // Reusing the physical path must not replace the saved source of truth.
        std::fs::create_dir(&f.checkout).unwrap();
        std::fs::write(f.checkout.join("tracked.txt"), "unrelated newer folder").unwrap();
        let RpcReply::Value(occupied) = rpc
            .handle(
                methods::GET_CHECKOUT_DIFF,
                json!({"chatId":"first","cwd":first_checkout,"mode":"workingTree"}),
            )
            .await
            .unwrap()
        else {
            panic!("expected saved diff")
        };
        assert_eq!(occupied["checksum"], review["checksum"]);
        std::fs::remove_file(f.checkout.join("tracked.txt")).unwrap();
        std::fs::remove_dir(&f.checkout).unwrap();
        // Another checkout's current HEAD must not change this saved diff.
        std::fs::write(f.repo.join("main-only.txt"), "new main work").unwrap();
        text(&f.repo, &["add", "."]).await.unwrap();
        text(
            &f.repo,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@localhost",
                "commit",
                "-m",
                "Advance main checkout",
            ],
        )
        .await
        .unwrap();
        let RpcReply::Value(stable) = rpc
            .handle(
                methods::GET_CHECKOUT_DIFF,
                json!({"chatId":"first","cwd":first_checkout,"mode":"workingTree"}),
            )
            .await
            .unwrap()
        else {
            panic!("expected stable saved diff")
        };
        assert_eq!(stable["checksum"], review["checksum"]);
        assert!(core.workspace.chat("first").unwrap().unwrap().archived);
        assert!(
            core.doc_host
                .queue_message("first", "do not revive", vec![])
                .is_err()
        );
        assert!(
            core.sessions
                .steer("first", "do not revive", None)
                .await
                .is_err()
        );
        assert_eq!(handle.doc().read_entries().unwrap().len(), 1);
        rpc.handle(
            methods::MUTATE,
            json!({"op":"setChatArchived","chatId":"first","archived":false}),
        )
        .await
        .unwrap();
        assert!(!core.workspace.chat("first").unwrap().unwrap().archived);
        assert!(core.workspace.chat("second").unwrap().unwrap().archived);
        assert_eq!(
            std::fs::read_to_string(f.checkout.join("tracked.txt")).unwrap(),
            "changed\n"
        );
        assert_eq!(handle.doc().read_entries().unwrap().len(), 1);
        core.shutdown().await;
    }

    #[tokio::test]
    async fn archived_workspace_browses_read_only_files_images_and_pinned_history() {
        use serde_json::{Value, json};
        use zuse_rpc::{RpcReply, RpcService, methods};
        async fn value(rpc: &dyn RpcService, method: &str, params: Value) -> Value {
            let RpcReply::Value(value) = rpc.handle(method, params).await.unwrap() else {
                panic!("Expected unary response")
            };
            value
        }
        let f = Fixture::new().await;
        f.edit();
        std::fs::create_dir(f.checkout.join("folder")).unwrap();
        std::fs::write(f.checkout.join("folder/nested.txt"), "nested saved text").unwrap();
        std::fs::write(
            f.checkout.join("unicode.txt"),
            "    café — needle ñ\nİSTANBUL needle\n",
        )
        .unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", f.checkout.join("outside-link")).unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(&f.data).unwrap(),
            std::sync::Arc::new(crate::HarnessRegistry::new()),
            zuse_protocol::HarnessId::Mock,
            None,
        )
        .unwrap();
        core.workspace
            .create_space(
                "project",
                &core.device_id,
                &f.repo.to_string_lossy(),
                None,
                true,
            )
            .unwrap();
        let rpc = core.rpc_service();
        value(rpc.as_ref(), methods::MUTATE, json!({"op":"createChat","chatId":"files","spaceId":"project","cwd":f.checkout,"config":{"harness":"mock","sandbox":"workspace-write"}})).await;
        value(
            rpc.as_ref(),
            methods::MUTATE,
            json!({"op":"setChatArchived","chatId":"files","archived":true}),
        )
        .await;
        let root = value(
            rpc.as_ref(),
            methods::LIST_WORKSPACE_DIRECTORY,
            json!({"chatId":"files"}),
        )
        .await;
        assert_eq!(root["mutationCapabilities"]["deleteEntry"], false);
        assert!(
            root["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == "tracked.txt" && entry["readOnly"] == true)
        );
        assert!(
            !root["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == ".env")
        );
        let ignored = value(
            rpc.as_ref(),
            methods::LIST_WORKSPACE_DIRECTORY,
            json!({"chatId":"files","includeIgnored":true}),
        )
        .await;
        assert!(
            ignored["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == ".env")
        );
        assert!(
            !ignored["entries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == "node_modules")
        );
        let nested = value(
            rpc.as_ref(),
            methods::LIST_WORKSPACE_DIRECTORY,
            json!({"chatId":"files","directory":"folder"}),
        )
        .await;
        assert_eq!(nested["entries"][0]["path"], "folder/nested.txt");
        for (path, expected) in [
            ("tracked.txt", "changed\n"),
            ("folder/nested.txt", "nested saved text"),
            (".env", "LOCAL_VALUE=preserved\n"),
        ] {
            let file = value(
                rpc.as_ref(),
                methods::READ_WORKSPACE_FILE,
                json!({"chatId":"files","path":path}),
            )
            .await;
            assert_eq!(file["text"], expected);
            assert_eq!(file["readOnlyReason"], "archived");
            assert_eq!(file["checkoutId"], root["checkoutId"]);
        }
        let absolute = value(
            rpc.as_ref(),
            methods::READ_WORKSPACE_FILE,
            json!({"chatId":"files","path":f.checkout.join("tracked.txt")}),
        )
        .await;
        assert_eq!(absolute["path"], "tracked.txt");
        for path in ["../outside", ".git/config", "/etc/passwd", "deleted.txt"] {
            assert!(
                rpc.handle(
                    methods::READ_WORKSPACE_FILE,
                    json!({"chatId":"files","path":path})
                )
                .await
                .is_err()
            );
        }
        #[cfg(unix)]
        {
            let link = value(
                rpc.as_ref(),
                methods::READ_WORKSPACE_FILE,
                json!({"chatId":"files","path":"outside-link"}),
            )
            .await;
            assert_eq!(link["readOnlyReason"], "symlink");
            assert!(link["text"].is_null());
        }
        let image = value(rpc.as_ref(), methods::READ_WORKSPACE_IMAGE, json!({"chatId":"files","path":".context/image.png","expectedCheckoutId":root["checkoutId"],"offset":0})).await;
        assert_eq!(image["mimeType"], "image/png");
        assert_eq!(image["data"], "iVBORw==");
        let found = value(
            rpc.as_ref(),
            methods::SEARCH_WORKSPACE_FILES,
            json!({"chatId":"files","query":"nst"}),
        )
        .await;
        assert!(
            found
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["path"] == "folder/nested.txt")
        );
        let content = value(
            rpc.as_ref(),
            methods::SEARCH_WORKSPACE_CONTENT,
            json!({"chatId":"files","query":"needle"}),
        )
        .await;
        assert_eq!(content["matches"].as_array().unwrap().len(), 2);
        assert_eq!(content["matches"][0]["preview"], "café — needle ñ");
        assert_eq!(content["matches"][0]["column"], 14);
        assert_eq!(content["matches"][0]["ranges"], json!([[10, 16]]));
        assert_eq!(content["matches"][1]["column"], 10);
        let warm = value(
            rpc.as_ref(),
            methods::WARM_WORKSPACE_SEARCH,
            json!({"chatId":"files"}),
        )
        .await;
        assert_eq!(warm["state"], "ready");
        // Saved HEAD is independent of the main checkout and future branch tips.
        let head = text(&f.repo, &["rev-parse", "HEAD"]).await.unwrap();
        std::fs::write(f.repo.join("later.txt"), "future main work").unwrap();
        text(&f.repo, &["add", "."]).await.unwrap();
        text(
            &f.repo,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@localhost",
                "commit",
                "-m",
                "Future main work",
            ],
        )
        .await
        .unwrap();
        let history = value(
            rpc.as_ref(),
            methods::LIST_GIT_HISTORY,
            json!({"chatId":"files","cwd":f.checkout}),
        )
        .await;
        assert_eq!(history["headSha"], head);
        assert_eq!(history["commits"].as_array().unwrap().len(), 1);
        assert_eq!(history["commits"][0]["subject"], "Initial files");
        let search = value(
            rpc.as_ref(),
            methods::SEARCH_GIT_HISTORY,
            json!({"chatId":"files","cwd":f.checkout,"query":"Future"}),
        )
        .await;
        assert!(search["commits"].as_array().unwrap().is_empty());
        assert!(!f.checkout.exists(), "browsing must not restore a checkout");
        assert!(rpc.handle(methods::WRITE_WORKSPACE_FILE, json!({"chatId":"files","path":"tracked.txt","text":"overwrite","expectedCheckoutId":root["checkoutId"],"expectedContentHash":absolute["contentHash"],"encoding":"utf8","lineEnding":"lf"})).await.is_err());
        let direct_write: zuse_protocol::WriteWorkspaceFileRequest = serde_json::from_value(json!({"chatId":"files","path":"tracked.txt","text":"overwrite","expectedCheckoutId":root["checkoutId"],"expectedContentHash":absolute["contentHash"],"encoding":"utf8","lineEnding":"lf"})).unwrap();
        assert!(core.workspace_files.write_file(direct_write).await.is_err());
        use futures::StreamExt;
        let RpcReply::Stream(mut file_watch) = rpc
            .handle(methods::WATCH_WORKSPACE_FILES, json!({"chatId":"files"}))
            .await
            .unwrap()
        else {
            panic!("expected file watch")
        };
        assert_eq!(file_watch.next().await.unwrap()["resyncRequired"], true);
        value(
            rpc.as_ref(),
            methods::MUTATE,
            json!({"op":"setChatArchived","chatId":"files","archived":false}),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_secs(2), file_watch.next())
                .await
                .unwrap()
                .is_none(),
            "restoration must retire the static watch so the client can attach live"
        );
        let active = value(
            rpc.as_ref(),
            methods::READ_WORKSPACE_FILE,
            json!({"chatId":"files","path":"tracked.txt"}),
        )
        .await;
        assert!(active["readOnlyReason"].is_null());
        assert_eq!(active["text"], "changed\n");
        // Main/shared folders remain live on disk, but an archived thread
        // still cannot mutate them and receives an explicit read-only preview.
        value(rpc.as_ref(), methods::MUTATE, json!({"op":"createChat","chatId":"mainfiles","spaceId":"project","cwd":f.repo,"config":{"harness":"mock","sandbox":"workspace-write"}})).await;
        let RpcReply::Stream(mut retained_watch) = rpc
            .handle(
                methods::WATCH_WORKSPACE_FILES,
                json!({"chatId":"mainfiles"}),
            )
            .await
            .unwrap()
        else {
            panic!("expected main file watch")
        };
        retained_watch.next().await.unwrap();
        value(
            rpc.as_ref(),
            methods::MUTATE,
            json!({"op":"setChatArchived","chatId":"mainfiles","archived":true}),
        )
        .await;
        assert!(
            tokio::time::timeout(Duration::from_secs(2), retained_watch.next())
                .await
                .unwrap()
                .is_none(),
            "flag-only archive must refresh file editability too"
        );
        let retained = value(
            rpc.as_ref(),
            methods::READ_WORKSPACE_FILE,
            json!({"chatId":"mainfiles","path":"tracked.txt"}),
        )
        .await;
        assert_eq!(retained["readOnlyReason"], "archived");
        assert!(f.repo.exists());
        core.shutdown().await;
    }

    #[tokio::test]
    async fn archive_managed_scope_excludes_main_and_external_worktrees() {
        let f = Fixture::new().await;
        let root = f._temp.path().join("worktrees");
        assert!(
            !f.archives
                .managed_checkout(&f.repo, &f.repo, &root)
                .await
                .unwrap()
        );
        assert!(
            f.archives
                .managed_checkout(&f.repo, &f.checkout, &root)
                .await
                .unwrap()
        );
        let outside = f._temp.path().join("manual");
        git(
            &f.repo,
            &[
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--detach"),
                outside.as_os_str(),
                OsStr::new("HEAD"),
            ],
            None,
        )
        .await
        .unwrap();
        assert!(
            !f.archives
                .managed_checkout(&f.repo, &outside, &root)
                .await
                .unwrap()
        );
    }
}
