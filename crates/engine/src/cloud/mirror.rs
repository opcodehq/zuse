//! Reference-compatible cleanup of cloud-owned local repository projections.
//! Catalog state admits cleanup; neither a timeout nor a local request implies archive.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

mod transfer;
pub use transfer::{SnapshotRequest, SnapshotResult, snapshot_stream, sync_once};

const MARKER: &str = ".zuse-sync.json";
const MANIFEST_LIMIT: u64 = 32 * 1024 * 1024;
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<u32>,
    workspace_id: String,
    #[serde(default)]
    files: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending: Vec<Entry>,
}
#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
struct Entry {
    path: String,
    hash: String,
    mode: u32,
    size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stamp: Option<String>,
}
pub use zuse_model::cloud::LocalMirrorCleanup as Cleanup;
fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains(['\\', '\0'])
        && path.split('/').all(|p| {
            !matches!(p, "" | "." | "..")
                && !p.eq_ignore_ascii_case(".git")
                && !p.eq_ignore_ascii_case(MARKER)
        })
}
impl Manifest {
    fn validate(&mut self, workspace: &str) -> Result<()> {
        ensure!(
            self.workspace_id == workspace,
            "This folder belongs to another cloud workspace."
        );
        ensure!(
            self.version.is_none() || self.version == Some(1),
            "Unsupported local sync manifest version."
        );
        // Older markers did not establish per-file ownership. Never infer it from the tree.
        if self.version.is_none() {
            self.files.clear();
            self.pending.clear();
        }
        for (index, list) in [&self.files, &self.pending].into_iter().enumerate() {
            let mut paths = HashSet::new();
            for f in list {
                ensure!(
                    valid_path(&f.path) && paths.insert(f.path.as_str()),
                    "Invalid local sync ownership path."
                );
                ensure!(
                    f.hash.len() == 64
                        && f.hash
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        && f.mode <= 0o777
                        && f.size <= 9_007_199_254_740_991,
                    "Invalid local sync file metadata."
                );
                ensure!(
                    f.link
                        .as_ref()
                        .is_none_or(|s| !s.contains('\0') && f.size == 0),
                    "Invalid local sync link."
                );
                ensure!(
                    f.stamp.as_ref().is_none_or(|s| !s.is_empty()
                        && s.len() <= 256
                        && s.bytes().all(|b| b.is_ascii_digit() || b == b':')),
                    "Invalid local sync source stamp."
                );
            }
            // A pending journal is the union of old and new paths, including
            // file/directory replacements. Cleanup refuses every pending journal.
            if index == 1 {
                continue;
            }
            for f in list {
                let mut parent = Path::new(&f.path).parent();
                while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
                    ensure!(
                        !paths.contains(p.to_str().unwrap()),
                        "Conflicting local sync ownership paths."
                    );
                    parent = p.parent();
                }
            }
        }
        Ok(())
    }
}

/// Discover only the reference's managed default tree, never arbitrary home folders.
/// Stop at a marker: checkout files are not another registry to traverse.
pub fn discover(base: &Path, workspace: &str) -> Result<Vec<PathBuf>> {
    let mut stack = vec![(base.to_owned(), 0)];
    let mut roots = Vec::new();
    let mut visited = 0;
    while let Some((path, depth)) = stack.pop() {
        let Some(dir) = unix::Dir::optional(&path)? else {
            continue;
        };
        visited += 1;
        ensure!(
            visited <= 50_000 && depth <= 64,
            "Local sync folder discovery exceeded its limit."
        );
        if let Some(file) = dir.optional_file(MARKER)? {
            let raw: serde_json::Value = serde_json::from_reader(file.take(MANIFEST_LIMIT + 1))
                .context("Invalid local sync ownership marker.")?;
            if raw["workspaceId"].as_str() == Some(workspace) {
                roots.push(path);
            }
            continue;
        }
        for entry in fs::read_dir(&path)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && !entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".zuse-sync-cache")
            {
                stack.push((entry.path(), depth + 1));
            }
        }
    }
    Ok(roots)
}

pub fn cleanup_default(base: &Path, workspace: &str) -> Cleanup {
    cleanup_default_checked(base, workspace, &|| false)
}
fn cleanup_default_checked(base: &Path, workspace: &str, cancelled: &dyn Fn() -> bool) -> Cleanup {
    let mut status = Cleanup {
        state: "complete".into(),
        ..Cleanup::default()
    };
    let result = (|| {
        for root in discover(base, workspace)? {
            ensure!(!cancelled(), "Cleanup interrupted.");
            let (removed, preserved) = cleanup_checked(&root, workspace, cancelled)?;
            status.removed += removed;
            status.preserved += preserved;
        }
        Ok::<_, anyhow::Error>(())
    })();
    if let Err(error) = result {
        status.state = "error".into();
        status.error = Some(format!("Local mirror preserved: {error}"));
    }
    status
}

fn read_manifest(dir: &unix::Dir, workspace: &str) -> Result<Manifest> {
    let file = dir
        .optional_file(MARKER)?
        .context("Missing local sync ownership marker.")?;
    ensure!(
        file.metadata()?.len() <= MANIFEST_LIMIT,
        "Local sync ownership marker is too large."
    );
    let mut raw: serde_json::Value = serde_json::from_reader(file.take(MANIFEST_LIMIT + 1))?;
    ensure!(raw.is_object(), "Invalid local sync ownership marker.");
    if raw.get("version").is_none() {
        raw["files"] = serde_json::json!([]);
        raw["pending"] = serde_json::json!([]);
    }
    let mut manifest: Manifest = serde_json::from_value(raw)?;
    manifest.validate(workspace)?;
    Ok(manifest)
}

/// Caller must stop/join the workspace sync operation before taking its cleanup gate.
/// Descriptor-relative operations refuse symlink ancestors, including the root.
pub fn cleanup(root: &Path, workspace: &str) -> Result<(usize, usize)> {
    cleanup_checked(root, workspace, &|| false)
}
fn cleanup_checked(
    root: &Path,
    workspace: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<(usize, usize)> {
    let dir = unix::Dir::optional(root)?;
    let mut manifest = if let Some(dir) = &dir {
        Some(read_manifest(dir, workspace)?)
    } else {
        None
    };
    ensure!(
        manifest.as_ref().is_none_or(|m| m.pending.is_empty()),
        "Snapshot publication was interrupted. Sync the workspace before cleanup."
    );
    // Preflight cache ownership before changing either tree, including a missing checkout.
    let mut cache_path = root.as_os_str().to_os_string();
    cache_path.push(".zuse-sync-cache");
    let cache = PathBuf::from(cache_path);
    let cache_dir = unix::Dir::optional(&cache)?;
    if let Some(cache_dir) = &cache_dir {
        read_manifest(cache_dir, workspace)?;
    }
    let Some(dir) = dir else {
        ensure!(!cancelled(), "Cleanup interrupted.");
        if let Some(cache_dir) = cache_dir {
            purge_cache(&cache_dir, &cache, workspace)?;
        }
        return Ok((0, 0));
    };
    let mut manifest = manifest.take().unwrap();
    let mut removed = HashSet::new();
    let mut directories = HashSet::new();
    for file in &manifest.files {
        ensure!(!cancelled(), "Cleanup interrupted.");
        if unix::remove_unchanged(&dir, file, cancelled)? {
            removed.insert(file.path.clone());
            let mut parent = Path::new(&file.path).parent();
            while let Some(p) = parent.filter(|p| !p.as_os_str().is_empty()) {
                directories.insert(p.to_owned());
                parent = p.parent();
            }
        }
    }
    let mut directories: Vec<_> = directories.into_iter().collect();
    directories.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for parent in directories {
        dir.remove_empty(&parent)?;
    }
    let count = removed.len();
    manifest.files.retain(|f| !removed.contains(&f.path));
    let preserved = manifest.files.len();
    ensure!(!cancelled(), "Cleanup interrupted.");
    if count > 0 {
        dir.write_manifest(&manifest)?;
    }
    if let Some(cache_dir) = cache_dir {
        purge_cache(&cache_dir, &cache, workspace)?;
    }
    if fs::read_dir(root)?.all(|e| e.is_ok_and(|e| e.file_name() == MARKER)) {
        dir.unlink(Path::new(MARKER), false)?;
        unix::remove_root(root)?;
    }
    Ok((count, preserved))
}

// Keep proof of cache ownership until all payload removal succeeds. A failed
// unlink/rmdir must remain retryable after process restart.
fn purge_cache(dir: &unix::Dir, path: &Path, workspace: &str) -> Result<()> {
    purge_cache_with(dir, path, workspace, unix::remove_root)
}
fn purge_cache_with(
    dir: &unix::Dir,
    path: &Path,
    workspace: &str,
    retire: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    let manifest = read_manifest(dir, workspace)?;
    dir.remove_contents(path, true)?;
    dir.unlink(Path::new(MARKER), false)?;
    if let Err(error) = retire(path) {
        let _ = dir.write_manifest(&manifest);
        return Err(error);
    }
    Ok(())
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
    };
    pub struct Dir(pub(super) File);
    pub(super) fn name(path: &Path) -> Result<CString> {
        Ok(CString::new(path.as_os_str().as_bytes())?)
    }
    pub(super) fn openat(parent: &File, path: &Path, flags: i32) -> std::io::Result<File> {
        let path = name(path).map_err(std::io::Error::other)?;
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                path.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    impl Dir {
        pub fn optional(path: &Path) -> Result<Option<Self>> {
            ensure!(path.is_absolute(), "Local sync paths must be absolute.");
            let mut dir = Self(File::open("/")?);
            for component in path.components() {
                match component {
                    Component::RootDir => {}
                    Component::Normal(p) => {
                        match openat(&dir.0, Path::new(p), libc::O_RDONLY | libc::O_DIRECTORY) {
                            Ok(file) => dir = Self(file),
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                            Err(_) => bail!(
                                "Local sync cleanup refused a non-directory or symlink ancestor."
                            ),
                        }
                    }
                    _ => bail!("Invalid local sync directory."),
                }
            }
            Ok(Some(dir))
        }
        pub(super) fn parent(&self, path: &Path) -> Result<(Self, PathBuf)> {
            let mut dir = Self(self.0.try_clone()?);
            let mut parts = path.components().peekable();
            while let Some(Component::Normal(p)) = parts.next() {
                if parts.peek().is_none() {
                    return Ok((dir, PathBuf::from(p)));
                }
                dir = Self(openat(
                    &dir.0,
                    Path::new(p),
                    libc::O_RDONLY | libc::O_DIRECTORY,
                )?);
            }
            bail!("Invalid local sync file path.")
        }
        pub fn optional_file(&self, path: &str) -> Result<Option<File>> {
            match openat(&self.0, Path::new(path), libc::O_RDONLY | libc::O_NONBLOCK) {
                Ok(f) => {
                    ensure!(
                        f.metadata()?.is_file(),
                        "Local sync marker is not a regular file."
                    );
                    Ok(Some(f))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e.into()),
            }
        }
        pub fn unlink(&self, path: &Path, directory: bool) -> Result<()> {
            let (parent, leaf) = self.parent(path)?;
            let leaf = name(&leaf)?;
            if unsafe {
                libc::unlinkat(
                    parent.0.as_raw_fd(),
                    leaf.as_ptr(),
                    if directory { libc::AT_REMOVEDIR } else { 0 },
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(())
        }
        pub fn remove_empty(&self, path: &Path) -> Result<()> {
            match self.unlink(path, true) {
                Ok(()) => Ok(()),
                Err(e)
                    if e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                        matches!(
                            e.raw_os_error(),
                            Some(libc::ENOENT | libc::ENOTEMPTY | libc::EEXIST | libc::ENOTDIR)
                        )
                    }) =>
                {
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        pub fn write_manifest(&self, manifest: &Manifest) -> Result<()> {
            let temporary = PathBuf::from(format!(".zuse-sync-{}.tmp", uuid::Uuid::new_v4()));
            let mut file = openat(
                &self.0,
                &temporary,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            )?;
            let result = (|| {
                serde_json::to_writer(&mut file, manifest)?;
                file.flush()?;
                file.sync_all()?;
                let from = name(&temporary)?;
                let to = name(Path::new(MARKER))?;
                ensure!(
                    unsafe {
                        libc::renameat(
                            self.0.as_raw_fd(),
                            from.as_ptr(),
                            self.0.as_raw_fd(),
                            to.as_ptr(),
                        )
                    } == 0,
                    "Could not commit local sync ownership marker."
                );
                self.0.sync_all()?;
                Ok(())
            })();
            let _ = self.unlink(&temporary, false);
            result
        }
        pub fn remove_contents(&self, path: &Path, keep_marker: bool) -> Result<()> {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let leaf = PathBuf::from(entry.file_name());
                if keep_marker && leaf == Path::new(MARKER) {
                    continue;
                }
                match openat(&self.0, &leaf, libc::O_RDONLY | libc::O_DIRECTORY) {
                    Ok(child) => {
                        Self(child).remove_contents(&entry.path(), false)?;
                        self.unlink(&leaf, true)?;
                    }
                    Err(e) if matches!(e.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) => {
                        self.unlink(&leaf, false)?
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(())
        }
    }
    fn signature(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32) {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
            m.mode(),
        )
    }
    pub fn remove_unchanged(
        root: &Dir,
        entry: &Entry,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool> {
        let (parent, leaf) = match root.parent(Path::new(&entry.path)) {
            Ok(p) => p,
            Err(_) => return Ok(false), // A changed/removed parent is local work, not deletion permission.
        };
        let leaf_name = name(&leaf)?;
        let mut before: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                parent.0.as_raw_fd(),
                leaf_name.as_ptr(),
                &mut before,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::NotFound {
                return Ok(false);
            }
            return Err(e.into());
        }
        if let Some(link) = &entry.link {
            if before.st_mode & libc::S_IFMT != libc::S_IFLNK {
                return Ok(false);
            }
            let mut target = vec![0; link.len() + 1];
            let n = unsafe {
                libc::readlinkat(
                    parent.0.as_raw_fd(),
                    leaf_name.as_ptr(),
                    target.as_mut_ptr().cast(),
                    target.len(),
                )
            };
            if n < 0 || &target[..n as usize] != link.as_bytes() {
                return Ok(false);
            }
        } else {
            if before.st_mode & libc::S_IFMT != libc::S_IFREG {
                return Ok(false);
            }
            let mut file = openat(&parent.0, &leaf, libc::O_RDONLY | libc::O_NONBLOCK)?;
            let metadata = file.metadata()?;
            if metadata.mode() & 0o777 != entry.mode || metadata.len() != entry.size {
                return Ok(false);
            }
            let mut hash = Sha256::new();
            let mut bytes = [0; 65536];
            let mut read = 0;
            loop {
                ensure!(!cancelled(), "Cleanup interrupted.");
                let n = file.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                read += n as u64;
                if read > entry.size {
                    return Ok(false);
                }
                hash.update(&bytes[..n]);
            }
            if read != entry.size
                || format!("{:x}", hash.finalize()) != entry.hash
                || signature(&metadata) != signature(&file.metadata()?)
            {
                return Ok(false);
            }
        }
        let mut after: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                parent.0.as_raw_fd(),
                leaf_name.as_ptr(),
                &mut after,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            return Ok(false);
        }
        // Identity and nanosecond mutation times protect against replacement during hashing.
        if before.st_dev != after.st_dev
            || before.st_ino != after.st_ino
            || before.st_size != after.st_size
            || before.st_mode != after.st_mode
            || before.st_mtime != after.st_mtime
            || before.st_ctime != after.st_ctime
            || before.st_mtime_nsec != after.st_mtime_nsec
            || before.st_ctime_nsec != after.st_ctime_nsec
        {
            return Ok(false);
        }
        // A second descriptor signature check above protects in-place edits; unlink never follows a link.
        ensure!(!cancelled(), "Cleanup interrupted.");
        parent.unlink(&leaf, false)?;
        Ok(true)
    }
    pub fn remove_root(root: &Path) -> Result<()> {
        let parent = Dir::optional(root.parent().context("Invalid local sync root.")?)?
            .context("Missing local sync parent.")?;
        parent.unlink(
            Path::new(root.file_name().context("Invalid local sync root.")?),
            true,
        )
    }
}

/// One cleanup operation per authenticated workspace. Catalog reads never wait
/// for file hashing, and repeated reads do not restart failed/completed work.
#[derive(Clone)]
pub struct Manager {
    base: PathBuf,
    records: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<(String, String), Record>>>,
    gate: std::sync::Arc<std::sync::Mutex<()>>,
}
struct Record {
    status: Cleanup,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Manager {
    pub fn new(base: PathBuf) -> Self {
        Self {
            base,
            records: Default::default(),
            gate: Default::default(),
        }
    }
    pub fn observe(
        &self,
        account: &str,
        workspace: &str,
        admitted: bool,
        retry: bool,
    ) -> Option<Cleanup> {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        let key = (account.to_owned(), workspace.to_owned());
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !admitted {
            if let Some(record) = records.remove(&key) {
                record.cancelled.store(true, Ordering::SeqCst);
            }
            return None;
        }
        if let Some(record) = records.get(&key) {
            if !retry || record.status.state == "pending" {
                return Some(record.status.clone());
            }
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        records.insert(
            key.clone(),
            Record {
                status: Cleanup::pending(),
                cancelled: cancelled.clone(),
            },
        );
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let _gate = this
                .gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let result =
                cleanup_default_checked(&this.base, &key.1, &|| cancelled.load(Ordering::SeqCst));
            let mut records = this
                .records
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(record) = records.get_mut(&key) {
                if Arc::ptr_eq(&record.cancelled, &cancelled) && !cancelled.load(Ordering::SeqCst) {
                    record.status = result;
                }
            }
        });
        Some(Cleanup::pending())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn entry(root: &Path, path: &str, bytes: &[u8]) -> Entry {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&target, bytes).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        Entry {
            path: path.into(),
            hash: format!("{:x}", Sha256::digest(bytes)),
            mode: 0o644,
            size: bytes.len() as u64,
            link: None,
            stamp: None,
        }
    }
    fn marker(root: &Path, workspace: &str, files: Vec<Entry>) {
        fs::create_dir_all(root).unwrap();
        fs::write(
            root.join(MARKER),
            serde_json::to_vec(&Manifest {
                version: Some(1),
                workspace_id: workspace.into(),
                files,
                pending: vec![],
            })
            .unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn archive_removes_unchanged_owned_files_and_cache_preserving_edits_and_extras() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("repo");
        fs::create_dir(&root).unwrap();
        let files = vec![
            entry(&root, "src/clean", "saved".as_bytes()),
            entry(&root, "src/edited", b"saved"),
            entry(&root, "mode", b"saved"),
        ];
        marker(&root, "ws", files);
        fs::write(root.join("src/edited"), b"local work").unwrap();
        fs::set_permissions(root.join("mode"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join("extra"), b"local only").unwrap();
        let cache = tmp
            .path()
            .canonicalize()
            .unwrap()
            .join("repo.zuse-sync-cache");
        marker(&cache, "ws", vec![]);
        fs::create_dir(cache.join("objects")).unwrap();
        fs::write(cache.join("objects/ciphertext"), b"download").unwrap();
        let outside = tmp.path().canonicalize().unwrap().join("outside");
        fs::write(&outside, b"unrelated").unwrap();
        symlink(&outside, cache.join("external-link")).unwrap();
        assert_eq!(cleanup(&root, "ws").unwrap(), (1, 2));
        assert!(!root.join("src/clean").exists());
        assert_eq!(fs::read(root.join("src/edited")).unwrap(), b"local work");
        assert!(root.join("extra").exists());
        assert!(!cache.exists());
        assert_eq!(fs::read(outside).unwrap(), b"unrelated");
        assert_eq!(
            read_manifest(&unix::Dir::optional(&root).unwrap().unwrap(), "ws")
                .unwrap()
                .files
                .len(),
            2
        );
        assert_eq!(cleanup(&root, "ws").unwrap(), (0, 2));
    }
    #[test]
    fn empty_mirror_is_removed_and_missing_root_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("repo");
        fs::create_dir(&root).unwrap();
        let file = entry(&root, "deep/file", b"unchanged");
        marker(&root, "ws", vec![file]);
        assert_eq!(cleanup(&root, "ws").unwrap(), (1, 0));
        assert!(!root.exists());
        assert_eq!(cleanup(&root, "ws").unwrap(), (0, 0));
    }
    #[test]
    fn foreign_pending_and_invalid_manifests_preserve_every_byte() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("repo");
        fs::create_dir(&root).unwrap();
        let f = entry(&root, "file", b"saved");
        marker(&root, "other", vec![f.clone()]);
        assert!(cleanup(&root, "ws").is_err());
        marker(&root, "ws", vec![f.clone()]);
        let mut m = read_manifest(&unix::Dir::optional(&root).unwrap().unwrap(), "ws").unwrap();
        m.pending = vec![f.clone()];
        fs::write(root.join(MARKER), serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(cleanup(&root, "ws").is_err());
        m.pending.clear();
        m.files[0].path = "../outside".into();
        fs::write(root.join(MARKER), serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(cleanup(&root, "ws").is_err());
        assert_eq!(fs::read(root.join("file")).unwrap(), b"saved");
    }
    #[test]
    fn symlink_roots_and_parents_never_authorize_external_deletion() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().canonicalize().unwrap().join("outside");
        fs::create_dir(&outside).unwrap();
        let f = entry(&outside, "file", b"outside");
        marker(&outside, "ws", vec![f.clone()]);
        let alias = tmp.path().canonicalize().unwrap().join("alias");
        symlink(&outside, &alias).unwrap();
        assert!(cleanup(&alias, "ws").is_err());
        let root = tmp.path().canonicalize().unwrap().join("repo");
        marker(
            &root,
            "ws",
            vec![Entry {
                path: "linked/file".into(),
                ..f
            }],
        );
        symlink(&outside, root.join("linked")).unwrap();
        assert_eq!(cleanup(&root, "ws").unwrap(), (0, 1));
        assert_eq!(fs::read(outside.join("file")).unwrap(), b"outside");
    }
    #[test]
    fn legacy_ownership_is_empty_and_foreign_cache_blocks_cleanup_before_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("repo");
        fs::create_dir(&root).unwrap();
        let f = entry(&root, "file", b"saved");
        fs::write(root.join(MARKER), br#"{"workspaceId":"ws"}"#).unwrap();
        assert_eq!(cleanup(&root, "ws").unwrap(), (0, 0));
        assert!(root.join("file").exists());
        marker(&root, "ws", vec![f]);
        marker(
            &tmp.path()
                .canonicalize()
                .unwrap()
                .join("repo.zuse-sync-cache"),
            "other",
            vec![],
        );
        assert!(cleanup(&root, "ws").is_err());
        assert!(root.join("file").exists());
    }
    #[test]
    fn discovery_matches_workspace_without_entering_checkout_or_cache_or_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap().join("cloud");
        let a = base.join("repo/feature/branch");
        marker(&a, "ws", vec![]);
        marker(&a.join("nested"), "ws", vec![]);
        marker(&base.join("another"), "other", vec![]);
        marker(&base.join("cache.zuse-sync-cache"), "ws", vec![]);
        symlink(&a, base.join("alias")).unwrap();
        assert_eq!(discover(&base, "ws").unwrap(), vec![a]);
    }
    #[test]
    fn unchanged_links_are_unlinked_and_missing_checkout_cache_is_purged() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("repo");
        let target = tmp.path().canonicalize().unwrap().join("outside");
        fs::write(&target, b"outside").unwrap();
        marker(
            &root,
            "ws",
            vec![Entry {
                path: "link".into(),
                hash: "0".repeat(64),
                mode: 0o777,
                size: 0,
                link: Some(target.to_str().unwrap().into()),
                stamp: None,
            }],
        );
        symlink(&target, root.join("link")).unwrap();
        assert_eq!(cleanup(&root, "ws").unwrap(), (1, 0));
        assert!(!root.exists());
        assert_eq!(fs::read(&target).unwrap(), b"outside");
        let cache = tmp
            .path()
            .canonicalize()
            .unwrap()
            .join("repo.zuse-sync-cache");
        marker(&cache, "ws", vec![]);
        fs::write(cache.join("object"), b"private").unwrap();
        assert_eq!(cleanup(&root, "ws").unwrap(), (0, 0));
        assert!(!cache.exists());
    }
    #[tokio::test]
    async fn manager_requires_catalog_admission_and_reconciles_a_new_archive_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap().join("repo");
        marker(&root, "ws", vec![]);
        let file = entry(&root, "file", b"saved");
        marker(&root, "ws", vec![file.clone()]);
        let manager = Manager::new(tmp.path().canonicalize().unwrap().to_owned());
        assert!(manager.observe("account", "ws", false, false).is_none());
        assert!(root.join("file").exists());
        for _ in 0..2 {
            assert_eq!(
                manager.observe("account", "ws", true, false).unwrap().state,
                "pending"
            );
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    if manager.observe("account", "ws", true, false).unwrap().state == "complete" {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert!(!root.exists());
            marker(&root, "ws", vec![]);
            entry(&root, "file", b"saved");
            marker(&root, "ws", vec![file.clone()]);
            assert_eq!(
                manager.observe("account", "ws", true, false).unwrap().state,
                "complete"
            );
            assert!(root.join("file").exists());
            manager.observe("account", "ws", false, false);
        }
    }
    #[test]
    fn failed_cache_removal_keeps_ownership_proof_for_retry() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("repo");
        let cache = base.join("repo.zuse-sync-cache");
        marker(&root, "ws", vec![]);
        marker(&cache, "ws", vec![]);
        fs::create_dir(cache.join("objects")).unwrap();
        fs::write(cache.join("objects/payload"), b"private download").unwrap();
        let dir = unix::Dir::optional(&cache).unwrap().unwrap();
        // A restart after payload cleanup still has a durable ownership marker.
        dir.remove_contents(&cache, true).unwrap();
        assert!(cache.join(MARKER).is_file());
        assert!(!cache.join("objects").exists());
        // Inject a failed final rmdir: this sandbox bypasses Unix directory mode
        // checks, so chmod cannot reliably exercise the failure path here.
        let result = purge_cache_with(&dir, &cache, "ws", |_| {
            bail!("injected cache retirement failure")
        });
        assert!(result.is_err());
        assert!(cache.join(MARKER).is_file());
        assert_eq!(read_manifest(&dir, "ws").unwrap().workspace_id, "ws");
        assert_eq!(cleanup(&root, "ws").unwrap(), (0, 0));
        assert!(!cache.exists());
        assert!(!root.exists());
    }
}
