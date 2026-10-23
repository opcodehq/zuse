//! Git-selected snapshot transport and verified publication for cloud mirrors.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::{StreamExt, stream::BoxStream};
use std::{
    collections::HashMap,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        io::AsRawFd,
    },
    sync::Arc,
};
use zuse_protocol::WorkspaceTarget;

const CHUNK: usize = 128 * 1024;
const MAX_FILES: usize = 100_000;
const MAX_BYTES: u64 = 100 * 1024 * 1024 * 1024;
#[derive(Deserialize)]
pub struct SnapshotRequest {
    #[serde(flatten)]
    pub target: WorkspaceTarget,
    #[serde(default)]
    pub have: Vec<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum Frame {
    Begin { checkout_id: String },
    Entry { entry: Entry },
    Chunk { path: String, data: String },
    End { path: String },
    Complete { files: usize },
    Error { message: String },
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotResult {
    pub checkout_id: String,
    pub files: usize,
    pub transferred_bytes: u64,
}
fn digest(file: &mut File) -> Result<String> {
    let mut hash = Sha256::new();
    let mut buf = vec![0; CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn stamp(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32) {
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
fn select(root: &Path, prefix: &str, paths: &mut Vec<String>) -> Result<()> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "--deduplicate",
            "-z",
        ])
        .output()?;
    ensure!(
        output.status.success(),
        "Could not enumerate the cloud repository."
    );
    ensure!(
        output.stdout.len() <= 64 * 1024 * 1024,
        "Repository file list exceeds the sync limit."
    );
    let dir = unix::Dir::optional(root)?.context("Cloud checkout is missing.")?;
    let mut names = HashSet::new();
    for raw in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let name = std::str::from_utf8(raw).context("Repository has a non-UTF-8 path.")?;
        ensure!(valid_path(name), "Unsupported repository path: {name}");
        if !names.insert(name) {
            continue;
        }
        let (parent, leaf) = match dir.parent(Path::new(name)) {
            Ok(value) => value,
            Err(error)
                if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
                    matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))
                }) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let file = match unix::openat(&parent.0, &leaf, libc::O_RDONLY | libc::O_NONBLOCK) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                paths.push(format!("{prefix}{name}"));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if file.metadata()?.is_dir() {
            ensure!(
                root.join(name).join(".git").exists(),
                "Uninitialized submodule: {name}"
            );
            select(&root.join(name), &format!("{prefix}{name}/"), paths)?;
        } else {
            paths.push(format!("{prefix}{name}"));
        }
        ensure!(
            paths.len() <= MAX_FILES,
            "Repository exceeds the sync file limit."
        );
    }
    Ok(())
}
fn link(parent: &unix::Dir, leaf: &Path) -> Result<String> {
    let mut buffer = vec![0; 16 * 1024];
    let name = unix::name(leaf)?;
    let n = unsafe {
        libc::readlinkat(
            parent.0.as_raw_fd(),
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    ensure!(
        n >= 0 && (n as usize) < buffer.len(),
        "Unsupported cloud symlink."
    );
    buffer.truncate(n as usize);
    Ok(String::from_utf8(buffer).context("Non-UTF-8 symlink target.")?)
}
pub fn snapshot_stream(
    root: PathBuf,
    checkout: String,
    have: Vec<String>,
) -> Result<BoxStream<'static, serde_json::Value>> {
    ensure!(
        have.len() <= MAX_FILES
            && have.iter().all(|h| h.len() == 64
                && h.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
        "Invalid snapshot baseline."
    );
    let have = have.into_iter().collect::<HashSet<_>>();
    let (tx, rx) = tokio::sync::mpsc::channel(2);
    tokio::task::spawn_blocking(move || {
        let send = |frame: Frame| -> Result<()> {
            tx.blocking_send(serde_json::to_value(frame)?)
                .map_err(|_| anyhow::anyhow!("Sync cancelled."))
        };
        let result = (|| {
            let dir = unix::Dir::optional(&root)?.context("Cloud checkout is missing.")?;
            let mut paths = Vec::new();
            select(&root, "", &mut paths)?;
            paths.sort();
            paths.dedup();
            ensure!(
                paths.len() <= MAX_FILES,
                "Repository exceeds the sync file limit."
            );
            send(Frame::Begin {
                checkout_id: checkout,
            })?;
            let mut count = 0;
            let mut total = 0u64;
            for path in paths {
                ensure!(!tx.is_closed(), "Sync cancelled.");
                let (parent, leaf) = dir.parent(Path::new(&path))?;
                let file = unix::openat(&parent.0, &leaf, libc::O_RDONLY | libc::O_NONBLOCK);
                let mut entry = Entry {
                    path: path.clone(),
                    hash: String::new(),
                    mode: 0,
                    size: 0,
                    link: None,
                    stamp: None,
                };
                match file {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) if error.raw_os_error() == Some(libc::ELOOP) => {
                        let target = link(&parent, &leaf)?;
                        entry.hash = format!("{:x}", Sha256::digest(target.as_bytes()));
                        entry.mode = 0o777;
                        entry.link = Some(target);
                        send(Frame::Entry { entry })?;
                    }
                    Err(error) => return Err(error.into()),
                    Ok(mut file) => {
                        let before = file.metadata()?;
                        ensure!(before.is_file(), "Unsupported cloud file type: {path}");
                        total = total
                            .checked_add(before.len())
                            .context("Snapshot is too large.")?;
                        ensure!(
                            total <= MAX_BYTES,
                            "Snapshot exceeds the 100 GiB sync limit."
                        );
                        entry.size = before.len();
                        entry.mode = before.mode() & 0o777;
                        entry.hash = digest(&mut file)?;
                        ensure!(
                            stamp(&before) == stamp(&file.metadata()?),
                            "Cloud file changed during capture: {path}"
                        );
                        let cached = have.contains(&entry.hash);
                        send(Frame::Entry { entry })?;
                        if !cached {
                            use std::io::{Seek, SeekFrom};
                            file.seek(SeekFrom::Start(0))?;
                            let mut buf = vec![0; CHUNK];
                            loop {
                                let n = file.read(&mut buf)?;
                                if n == 0 {
                                    break;
                                }
                                send(Frame::Chunk {
                                    path: path.clone(),
                                    data: STANDARD.encode(&buf[..n]),
                                })?;
                            }
                        }
                        ensure!(
                            stamp(&before) == stamp(&file.metadata()?),
                            "Cloud file changed during transfer: {path}"
                        );
                    }
                }
                send(Frame::End { path })?;
                count += 1;
            }
            send(Frame::Complete { files: count })
        })();
        if let Err(error) = result {
            let _ = send(Frame::Error {
                message: error.to_string(),
            });
        }
    });
    Ok(
        futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|v| (v, rx)) })
            .boxed(),
    )
}
fn ensure_dir(path: &Path) -> Result<unix::Dir> {
    ensure!(path.is_absolute(), "Choose an absolute local sync path.");
    let mut dir = unix::Dir::optional(Path::new("/"))?.unwrap();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                let name = Path::new(name);
                match unix::openat(&dir.0, name, libc::O_RDONLY | libc::O_DIRECTORY) {
                    Ok(file) => dir = unix::Dir(file),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        let raw = unix::name(name)?;
                        ensure!(
                            unsafe { libc::mkdirat(dir.0.as_raw_fd(), raw.as_ptr(), 0o700) } == 0,
                            "Could not create the local sync directory."
                        );
                        dir = unix::Dir(unix::openat(
                            &dir.0,
                            name,
                            libc::O_RDONLY | libc::O_DIRECTORY,
                        )?);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            _ => bail!("Invalid local sync path."),
        }
    }
    Ok(dir)
}
fn prepare(path: &Path, workspace: &str) -> Result<unix::Dir> {
    let dir = ensure_dir(path)?;
    if dir.optional_file(MARKER)?.is_none() {
        ensure!(
            fs::read_dir(path)?.next().is_none(),
            "Choose an empty folder or an existing mirror for this workspace."
        );
        dir.write_manifest(&Manifest {
            version: Some(1),
            workspace_id: workspace.into(),
            files: vec![],
            pending: vec![],
        })?;
    }
    read_manifest(&dir, workspace)?;
    Ok(dir)
}
fn parent_create(root: &unix::Dir, path: &Path) -> Result<(unix::Dir, PathBuf)> {
    let mut parent = unix::Dir(root.0.try_clone()?);
    let mut parts = path.components().peekable();
    while let Some(Component::Normal(name)) = parts.next() {
        if parts.peek().is_none() {
            return Ok((parent, name.into()));
        }
        let name = Path::new(name);
        match unix::openat(&parent.0, name, libc::O_RDONLY | libc::O_DIRECTORY) {
            Ok(file) => parent = unix::Dir(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let raw = unix::name(name)?;
                ensure!(
                    unsafe { libc::mkdirat(parent.0.as_raw_fd(), raw.as_ptr(), 0o700) } == 0,
                    "Could not create a mirror parent."
                );
                parent = unix::Dir(unix::openat(
                    &parent.0,
                    name,
                    libc::O_RDONLY | libc::O_DIRECTORY,
                )?);
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("Invalid sync path.")
}
fn install(root: &unix::Dir, entry: &Entry, objects: &unix::Dir) -> Result<()> {
    let (parent, leaf) = parent_create(root, Path::new(&entry.path))?;
    let temporary = PathBuf::from(format!(".zuse-sync-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        if let Some(target) = &entry.link {
            let target = std::ffi::CString::new(target.as_str())?;
            let name = unix::name(&temporary)?;
            ensure!(
                unsafe { libc::symlinkat(target.as_ptr(), parent.0.as_raw_fd(), name.as_ptr()) }
                    == 0,
                "Could not stage a mirror symlink."
            );
        } else {
            let mut source = objects
                .optional_file(&entry.hash)?
                .context("Missing verified snapshot object.")?;
            ensure!(
                source.metadata()?.len() == entry.size && digest(&mut source)? == entry.hash,
                "Corrupt snapshot object."
            );
            use std::io::{Seek, SeekFrom};
            source.seek(SeekFrom::Start(0))?;
            let mut output = unix::openat(
                &parent.0,
                &temporary,
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            )?;
            std::io::copy(&mut source, &mut output)?;
            output.set_permissions(fs::Permissions::from_mode(entry.mode))?;
            output.sync_all()?;
        }
        let from = unix::name(&temporary)?;
        let to = unix::name(&leaf)?;
        ensure!(
            unsafe {
                libc::renameat(
                    parent.0.as_raw_fd(),
                    from.as_ptr(),
                    parent.0.as_raw_fd(),
                    to.as_ptr(),
                )
            } == 0,
            "Could not publish cloud file: {}",
            entry.path
        );
        parent.0.sync_all()?;
        Ok(())
    })();
    let _ = parent.unlink(&temporary, false);
    result
}
fn matches(root: &unix::Dir, entry: &Entry) -> bool {
    let Ok((parent, leaf)) = root.parent(Path::new(&entry.path)) else {
        return false;
    };
    if let Some(target) = &entry.link {
        return link(&parent, &leaf).is_ok_and(|actual| actual == *target);
    }
    let Ok(mut file) = unix::openat(&parent.0, &leaf, libc::O_RDONLY | libc::O_NONBLOCK) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    metadata.is_file()
        && metadata.len() == entry.size
        && metadata.mode() & 0o777 == entry.mode
        && digest(&mut file).is_ok_and(|hash| hash == entry.hash)
}

fn publish(root: &Path, workspace: &str, files: Vec<Entry>, objects: &unix::Dir) -> Result<()> {
    let dir = prepare(root, workspace)?;
    let previous = read_manifest(&dir, workspace)?;
    let mut next = Manifest {
        version: Some(1),
        workspace_id: workspace.into(),
        files,
        pending: vec![],
    };
    next.validate(workspace)?;
    ensure!(
        serde_json::to_vec(&next)?.len() as u64 <= MANIFEST_LIMIT,
        "Snapshot ownership manifest exceeds the sync limit."
    );
    // Validate all staged bytes before claiming or changing any user file.
    for entry in &next.files {
        if entry.link.is_some() {
            continue;
        }
        let mut file = objects
            .optional_file(&entry.hash)?
            .context("Missing snapshot object.")?;
        ensure!(
            file.metadata()?.len() == entry.size && digest(&mut file)? == entry.hash,
            "Corrupt snapshot object."
        );
    }
    let changed = next
        .files
        .iter()
        .filter(|entry| !matches(&dir, entry))
        .map(|entry| entry.path.clone())
        .collect::<HashSet<_>>();
    if changed.is_empty() && previous.pending.is_empty() && previous.files == next.files {
        return Ok(());
    }
    let paths = next
        .files
        .iter()
        .map(|f| f.path.as_str())
        .collect::<HashSet<_>>();
    let mut owned = HashMap::new();
    for entry in previous.files.iter().chain(&previous.pending) {
        owned.insert(entry.path.clone(), entry.clone());
    }
    let mut removed = owned
        .keys()
        .filter(|p| !paths.contains(p.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    // A retry can observe a pending old file already replaced by a directory
    // containing the new snapshot's children. Keep that directory and its extras.
    let ordered = paths
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    removed.retain(|path| {
        let prefix = format!("{path}/");
        if ordered
            .range(prefix.as_str()..)
            .next()
            .is_some_and(|next| next.starts_with(&prefix))
        {
            if let Ok((parent, leaf)) = dir.parent(Path::new(path)) {
                if unix::openat(&parent.0, &leaf, libc::O_RDONLY | libc::O_DIRECTORY).is_ok() {
                    return false;
                }
            }
        }
        true
    });
    removed.sort_by_key(|p| std::cmp::Reverse(p.len()));
    // Directory replacement may only consume paths already owned by this mirror.
    fn check_directory(
        root: &Path,
        path: &str,
        removed: &HashSet<&str>,
        dirs: &mut Vec<String>,
    ) -> Result<()> {
        for item in fs::read_dir(root.join(path))? {
            let item = item?;
            let name = item
                .file_name()
                .into_string()
                .map_err(|_| anyhow::anyhow!("Non-UTF-8 local file."))?;
            let child = format!("{path}/{name}");
            if item.file_type()?.is_dir() {
                check_directory(root, &child, removed, dirs)?;
            } else {
                ensure!(
                    removed.contains(child.as_str()),
                    "Sync would replace an unrelated local file: {child}"
                );
            }
        }
        dirs.push(path.into());
        Ok(())
    }
    let removing = removed.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut dirs = vec![];
    for entry in &next.files {
        match dir.parent(Path::new(&entry.path)) {
            Ok((parent, leaf)) => {
                match unix::openat(&parent.0, &leaf, libc::O_RDONLY | libc::O_DIRECTORY) {
                    Ok(_) => check_directory(root, &entry.path, &removing, &mut dirs)?,
                    Err(error)
                        if matches!(
                            error.raw_os_error(),
                            Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP)
                        ) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Err(error)
                if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
                    matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))
                }) => {}
            Err(error) => return Err(error),
        }
    }
    let mut pending = owned;
    for entry in &next.files {
        pending.insert(entry.path.clone(), entry.clone());
    }
    let journal = Manifest {
        pending: pending.into_values().collect(),
        ..previous
    };
    ensure!(
        serde_json::to_vec(&journal)?.len() as u64 <= MANIFEST_LIMIT,
        "Snapshot ownership journal exceeds the sync limit."
    );
    dir.write_manifest(&journal)?;
    for path in &removed {
        match dir.unlink(Path::new(path), false) {
            Ok(()) => {}
            Err(error)
                if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
                    matches!(e.raw_os_error(), Some(libc::ENOENT | libc::ENOTDIR))
                }) => {}
            Err(error) => return Err(error),
        }
    }
    // Reclaim empty parents of deleted cloud-owned paths. Nonempty parents,
    // including directories containing unrelated local files, stay in place.
    for path in &removed {
        let mut parent = Path::new(path).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            dir.remove_empty(path)?;
            parent = path.parent();
        }
    }
    for path in dirs {
        dir.remove_empty(Path::new(&path))?;
    }
    for entry in &next.files {
        if changed.contains(&entry.path) {
            install(&dir, entry, objects)?;
        }
    }
    dir.write_manifest(&next)?;
    Ok(())
}
struct PartialFile {
    name: PathBuf,
    file: File,
    parent: Arc<unix::Dir>,
}
impl Drop for PartialFile {
    fn drop(&mut self) {
        let _ = self.parent.unlink(&self.name, false);
    }
}

async fn receive(
    root: PathBuf,
    workspace: String,
    mut stream: BoxStream<'static, serde_json::Value>,
    objects: Arc<unix::Dir>,
) -> Result<SnapshotResult> {
    let mut checkout = None;
    let mut files = vec![];
    let mut seen = HashSet::new();
    let mut current: Option<(Entry, Option<PartialFile>, u64)> = None;
    let mut bytes = 0u64;
    let mut declared = 0u64;
    let mut complete = false;
    while let Some(value) = stream.next().await {
        let frame: Frame =
            serde_json::from_value(value).context("Invalid cloud snapshot frame.")?;
        ensure!(!complete, "Trailing cloud snapshot frame.");
        match frame {
            Frame::Begin { checkout_id } => {
                ensure!(
                    checkout.is_none() && !checkout_id.is_empty(),
                    "Invalid snapshot identity."
                );
                checkout = Some(checkout_id);
            }
            Frame::Entry { entry } => {
                ensure!(
                    checkout.is_some()
                        && current.is_none()
                        && seen.insert(entry.path.clone())
                        && files.len() < MAX_FILES,
                    "Invalid snapshot entry sequence."
                );
                let mut check = Manifest {
                    version: Some(1),
                    workspace_id: workspace.clone(),
                    files: vec![entry.clone()],
                    pending: vec![],
                };
                check.validate(&workspace)?;
                declared = declared
                    .checked_add(entry.size)
                    .context("Snapshot is too large.")?;
                ensure!(
                    declared <= MAX_BYTES,
                    "Snapshot exceeds the sync size limit."
                );
                if let Some(target) = &entry.link {
                    ensure!(
                        entry.hash == format!("{:x}", Sha256::digest(target.as_bytes())),
                        "Invalid symlink digest."
                    );
                }
                current = Some((entry, None, 0));
            }
            Frame::Chunk { path, data } => {
                let (entry, output, size) =
                    current.as_mut().context("Snapshot data has no file.")?;
                ensure!(
                    entry.path == path && entry.link.is_none() && data.len() <= CHUNK * 2,
                    "Invalid snapshot data."
                );
                let raw = STANDARD.decode(data)?;
                *size += raw.len() as u64;
                bytes += raw.len() as u64;
                ensure!(
                    *size <= entry.size && bytes <= MAX_BYTES,
                    "Snapshot data exceeds its declared size."
                );
                if output.is_none() {
                    let name = PathBuf::from(format!(".receive-{}", uuid::Uuid::new_v4()));
                    let file = unix::openat(
                        &objects.0,
                        &name,
                        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                    )?;
                    *output = Some(PartialFile {
                        name,
                        file,
                        parent: objects.clone(),
                    });
                }
                output.as_mut().unwrap().file.write_all(&raw)?;
            }
            Frame::End { path } => {
                let (entry, output, size) =
                    current.take().context("Snapshot has no active file.")?;
                ensure!(entry.path == path, "Snapshot path changed.");
                if entry.link.is_none() {
                    if let Some(output) = output {
                        ensure!(size == entry.size, "Truncated cloud file.");
                        output.file.sync_all()?;
                        let mut file = objects
                            .optional_file(output.name.to_str().unwrap())?
                            .unwrap();
                        ensure!(
                            digest(&mut file)? == entry.hash,
                            "Cloud file digest mismatch."
                        );
                        let from = unix::name(&output.name)?;
                        let to = unix::name(Path::new(&entry.hash))?;
                        ensure!(
                            unsafe {
                                libc::renameat(
                                    objects.0.as_raw_fd(),
                                    from.as_ptr(),
                                    objects.0.as_raw_fd(),
                                    to.as_ptr(),
                                )
                            } == 0,
                            "Could not retain verified snapshot object."
                        );
                    } else if entry.size == 0 && objects.optional_file(&entry.hash)?.is_none() {
                        ensure!(
                            entry.hash == format!("{:x}", Sha256::digest([])),
                            "Invalid empty file digest."
                        );
                        let file = unix::openat(
                            &objects.0,
                            Path::new(&entry.hash),
                            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
                        )?;
                        file.sync_all()?;
                    }
                    let mut file = objects
                        .optional_file(&entry.hash)?
                        .context("Missing cloud file bytes.")?;
                    ensure!(
                        file.metadata()?.len() == entry.size && digest(&mut file)? == entry.hash,
                        "Unverified cached snapshot object."
                    );
                }
                files.push(entry);
            }
            Frame::Complete { files: count } => {
                ensure!(
                    checkout.is_some() && current.is_none() && count == files.len(),
                    "Incomplete cloud snapshot."
                );
                complete = true;
            }
            Frame::Error { message } => bail!("Cloud snapshot failed: {message}"),
        }
    }
    ensure!(
        complete,
        "Cloud snapshot ended before verification completed."
    );
    let result = SnapshotResult {
        checkout_id: checkout.unwrap(),
        files: files.len(),
        transferred_bytes: bytes,
    };
    tokio::task::spawn_blocking(move || publish(&root, &workspace, files, &objects)).await??;
    Ok(result)
}
/// One complete transfer on an already connected execution client. This never
/// acquires a gateway or resumes compute. The caller serializes sync and cleanup.
pub async fn sync_once(
    client: &zuse_rpc::RpcClient,
    target: WorkspaceTarget,
    root: PathBuf,
    workspace: String,
) -> Result<SnapshotResult> {
    let root = root.components().collect::<PathBuf>();
    prepare(&root, &workspace)?;
    let mut cache_name = root.as_os_str().to_owned();
    cache_name.push(".zuse-sync-cache");
    let cache = PathBuf::from(cache_name);
    let cache_dir = prepare(&cache, &workspace)?;
    let objects = Arc::new(ensure_dir(&cache.join("objects"))?);
    let mut have = vec![];
    for item in fs::read_dir(cache.join("objects"))? {
        let name = item?.file_name().to_string_lossy().into_owned();
        if name.len() != 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            continue;
        }
        let mut file = objects
            .optional_file(&name)?
            .context("Invalid snapshot cache object.")?;
        if digest(&mut file)? == name {
            have.push(name);
        }
    }
    ensure!(
        have.len() <= MAX_FILES,
        "Snapshot cache exceeds the file limit."
    );
    let mut params = serde_json::to_value(target)?;
    params["have"] = serde_json::to_value(have)?;
    let stream = client
        .subscribe(zuse_protocol::methods::WORKSPACE_SYNC_SNAPSHOT, params)
        .await?;
    let stream = futures::stream::unfold(stream, |mut stream| async move {
        stream.recv().await.map(|v| (v, stream))
    })
    .boxed();
    let result = receive(root.clone(), workspace.clone(), stream, objects.clone()).await;
    if result.is_ok() {
        let dir = unix::Dir::optional(&root)?.context("Local mirror disappeared.")?;
        let manifest = read_manifest(&dir, &workspace)?;
        let keep = manifest
            .files
            .iter()
            .filter(|entry| entry.link.is_none())
            .map(|entry| entry.hash.as_str())
            .collect::<HashSet<_>>();
        for item in fs::read_dir(cache.join("objects"))? {
            let name = item?.file_name().to_string_lossy().into_owned();
            if name.len() == 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                && !keep.contains(name.as_str())
            {
                objects.unlink(Path::new(&name), false)?;
            }
        }
    }
    drop(cache_dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) {
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        );
    }
    fn source(root: &Path) {
        fs::create_dir(root).unwrap();
        git(root, &["init", "--initial-branch=main"]);
        git(root, &["config", "user.email", "sync@example.invalid"]);
        git(root, &["config", "user.name", "Sync fixture"]);
        fs::create_dir(root.join("nested")).unwrap();
        fs::write(root.join("nested/tracked"), "original").unwrap();
        fs::write(root.join(".gitignore"), "ignored\n.env\n").unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-m", "fixture"]);
    }
    async fn transfer(source: &Path, mirror: &Path, have: Vec<String>) -> Result<SnapshotResult> {
        let cache = PathBuf::from(format!("{}.zuse-sync-cache", mirror.display()));
        prepare(&cache, "workspace")?;
        let objects = Arc::new(ensure_dir(&cache.join("objects"))?);
        receive(
            mirror.into(),
            "workspace".into(),
            snapshot_stream(source.into(), "original-checkout".into(), have)?,
            objects,
        )
        .await
    }
    #[tokio::test]
    async fn git_snapshot_roundtrip_incremental_delete_and_archive_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote");
        let local = temp.path().join("local");
        source(&remote);
        fs::write(remote.join("nested/tracked"), "changed bytes").unwrap();
        fs::write(remote.join("untracked"), vec![42; CHUNK * 3 + 19]).unwrap();
        fs::write(remote.join("empty"), []).unwrap();
        fs::write(remote.join("ignored"), "disposable").unwrap();
        fs::write(remote.join(".env"), "private").unwrap();
        std::os::unix::fs::symlink("/unrelated/external", remote.join("link")).unwrap();
        let result = transfer(&remote, &local, vec![]).await.unwrap();
        assert_eq!(result.checkout_id, "original-checkout");
        assert_eq!(
            fs::read(local.join("untracked")).unwrap(),
            vec![42; CHUNK * 3 + 19]
        );
        assert!(!local.join("ignored").exists());
        assert!(!local.join(".env").exists());
        assert!(!local.join(".git").exists());
        assert_eq!(
            fs::read_link(local.join("link")).unwrap(),
            Path::new("/unrelated/external")
        );
        let marker =
            read_manifest(&unix::Dir::optional(&local).unwrap().unwrap(), "workspace").unwrap();
        let cached = marker
            .files
            .iter()
            .filter(|f| f.link.is_none())
            .map(|f| f.hash.clone())
            .collect();
        let result = transfer(&remote, &local, cached).await.unwrap();
        assert_eq!(result.transferred_bytes, 0);
        fs::write(local.join("local-only"), "keep me").unwrap();
        fs::write(
            local.join("nested/tracked"),
            "local edit is replaced while sync is enabled",
        )
        .unwrap();
        fs::remove_dir_all(remote.join("nested")).unwrap();
        fs::write(remote.join("untracked"), "next").unwrap();
        transfer(&remote, &local, vec![]).await.unwrap();
        assert!(!local.join("nested/tracked").exists());
        assert_eq!(fs::read_to_string(local.join("untracked")).unwrap(), "next");
        assert_eq!(
            fs::read_to_string(local.join("local-only")).unwrap(),
            "keep me"
        );
        fs::write(local.join("untracked"), "new local work after sync").unwrap();
        let (removed, preserved) = cleanup(&local, "workspace").unwrap();
        assert!(removed > 0);
        assert_eq!(preserved, 1);
        assert_eq!(
            fs::read_to_string(local.join("untracked")).unwrap(),
            "new local work after sync"
        );
        assert!(local.join("local-only").exists());
    }
    #[tokio::test]
    async fn incomplete_and_corrupt_snapshots_never_publish_files() {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote");
        let local = temp.path().join("local");
        source(&remote);
        transfer(&remote, &local, vec![]).await.unwrap();
        let marker = fs::read(local.join(MARKER)).unwrap();
        let objects = Arc::new(ensure_dir(&temp.path().join("objects")).unwrap());
        let entry = Entry {
            path: "nested/tracked".into(),
            hash: format!("{:x}", Sha256::digest(b"new")),
            mode: 0o644,
            size: 3,
            link: None,
            stamp: None,
        };
        for bad in [false, true] {
            let mut frames = vec![
                Frame::Begin {
                    checkout_id: "original-checkout".into(),
                },
                Frame::Entry {
                    entry: entry.clone(),
                },
                Frame::Chunk {
                    path: entry.path.clone(),
                    data: STANDARD.encode(if bad { b"bad" } else { b"new" }),
                },
            ];
            if bad {
                frames.push(Frame::End {
                    path: entry.path.clone(),
                });
                frames.push(Frame::Complete { files: 1 });
            }
            let stream =
                futures::stream::iter(frames.into_iter().map(|f| serde_json::to_value(f).unwrap()))
                    .boxed();
            assert!(
                receive(local.clone(), "workspace".into(), stream, objects.clone())
                    .await
                    .is_err()
            );
            assert_eq!(fs::read(local.join(MARKER)).unwrap(), marker);
            assert_eq!(
                fs::read_to_string(local.join("nested/tracked")).unwrap(),
                "original"
            );
            assert!(
                fs::read_dir(temp.path().join("objects"))
                    .unwrap()
                    .next()
                    .is_none(),
                "Failed transfer must remove its temporary chunks"
            );
        }
    }
    #[tokio::test]
    async fn file_directory_transitions_and_interrupted_journal_are_retryable() {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote");
        let local = temp.path().join("local");
        source(&remote);
        transfer(&remote, &local, vec![]).await.unwrap();
        fs::remove_dir_all(remote.join("nested")).unwrap();
        fs::write(remote.join("nested"), "now a file").unwrap();
        transfer(&remote, &local, vec![]).await.unwrap();
        assert_eq!(
            fs::read_to_string(local.join("nested")).unwrap(),
            "now a file"
        );
        fs::remove_file(remote.join("nested")).unwrap();
        fs::create_dir(remote.join("nested")).unwrap();
        fs::write(remote.join("nested/next"), "back to directory").unwrap();
        transfer(&remote, &local, vec![]).await.unwrap();
        assert_eq!(
            fs::read_to_string(local.join("nested/next")).unwrap(),
            "back to directory"
        );
        let dir = unix::Dir::optional(&local).unwrap().unwrap();
        let mut manifest = read_manifest(&dir, "workspace").unwrap();
        manifest.pending = manifest.files.clone();
        manifest.pending.push(Entry {
            path: "nested".into(),
            hash: format!("{:x}", Sha256::digest(b"past partial")),
            mode: 0o644,
            size: 12,
            link: None,
            stamp: None,
        });
        dir.write_manifest(&manifest).unwrap();
        assert!(cleanup(&local, "workspace").is_err());
        transfer(&remote, &local, vec![]).await.unwrap();
        assert!(read_manifest(&dir, "workspace").unwrap().pending.is_empty());
    }
    #[tokio::test]
    async fn local_collisions_and_symlink_ancestors_preserve_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote");
        let local = temp.path().join("local");
        source(&remote);
        transfer(&remote, &local, vec![]).await.unwrap();
        fs::write(local.join("nested/local-only"), "unrelated").unwrap();
        fs::remove_dir_all(remote.join("nested")).unwrap();
        fs::write(remote.join("nested"), "replacement").unwrap();
        assert!(
            transfer(&remote, &local, vec![])
                .await
                .unwrap_err()
                .to_string()
                .contains("unrelated")
        );
        assert_eq!(
            fs::read_to_string(local.join("nested/local-only")).unwrap(),
            "unrelated"
        );
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), "untouched").unwrap();
        let alias = temp.path().join("alias");
        std::os::unix::fs::symlink(&outside, &alias).unwrap();
        assert!(transfer(&remote, &alias, vec![]).await.is_err());
        assert_eq!(
            fs::read_to_string(outside.join("sentinel")).unwrap(),
            "untouched"
        );
    }
}
