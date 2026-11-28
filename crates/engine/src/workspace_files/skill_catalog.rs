use super::*;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read};
use zuse_protocol::skill_catalog::{
    self, CatalogAction, CatalogEntry, CatalogRequest, CatalogState, Installation, SkillFile,
};

impl WorkspaceFiles {
    pub(crate) async fn project_skill_catalog(
        &self,
        request: CatalogRequest,
    ) -> Result<CatalogState, WorkspaceFilesError> {
        self.require_active_target(&request.target)?;
        if let Some(chat_id) = &request.target.chat_id
            && self
                .inner
                .workspace
                .chat(chat_id)
                .map_err(|error| WorkspaceFilesError::Io(error.to_string()))?
                .is_some_and(|chat| chat.space_id.is_none())
        {
            return Err(bad_path("Select a project checkout to manage skills"));
        }
        let workspace = self.resolve_target(&request.target).await?;
        let mutation = match &request.action {
            CatalogAction::List => None,
            CatalogAction::Install { skill } | CatalogAction::Remove { skill } => {
                Some(skill_catalog::entry(skill).ok_or_else(|| bad_path("Unknown catalog skill"))?)
            }
        };
        if mutation.is_some()
            && request.expected_checkout_id.as_deref() != Some(workspace.checkout_id.as_str())
        {
            return Err(checkout_changed());
        }
        // Download before taking the checkout's filesystem gate.
        let mut files = None;
        if let Some(entry) = mutation
            && matches!(&request.action, CatalogAction::Install { .. })
        {
            let folder = workspace.root.clone();
            let current = tokio::task::spawn_blocking(move || status(&folder, entry))
                .await
                .map_err(worker_error)??;
            match current {
                Some(Installation::Installed) => {}
                Some(Installation::Existing) => return Err(existing_skill()),
                None => {
                    let downloaded = tokio::time::timeout(
                        Duration::from_secs(40),
                        crate::github_skills::download(entry),
                    )
                    .await
                    .map_err(|_| {
                        WorkspaceFilesError::Io(
                            "The GitHub download timed out. Try installing again.".into(),
                        )
                    })?
                    .map_err(WorkspaceFilesError::Io)?;
                    files = Some(downloaded);
                }
            }
        }
        let gate = self
            .mutation_gate(&workspace.checkout_id)
            .write_owned()
            .await;
        if mutation.is_some() {
            self.require_active_target(&request.target)?;
            if self.resolve_target(&request.target).await? != workspace {
                return Err(checkout_changed());
            }
        }

        tokio::task::spawn_blocking(move || {
            let _gate = gate;
            let root = &workspace.root;
            if let Some(entry) = mutation {
                match (request.action, status(root, entry)?) {
                    (_, Some(Installation::Existing)) => return Err(existing_skill()),
                    (CatalogAction::Install { .. }, None) => {
                        let files = files.ok_or_else(checkout_changed)?;
                        install(root, entry, &files)?;
                    }
                    (CatalogAction::Remove { .. }, Some(Installation::Installed)) => {
                        remove(root, entry)?;
                    }
                    _ => {}
                }
            }
            let mut installations = std::collections::BTreeMap::new();
            for entry in skill_catalog::entries() {
                if let Some(state) = status(root, entry)? {
                    installations.insert(entry.package.id.clone(), state);
                }
            }
            Ok(CatalogState {
                checkout_id: workspace.checkout_id,
                installations,
            })
        })
        .await
        .map_err(worker_error)?
    }
}

/// A private folder beside the installed skills, removed when dropped.
///
/// On Unix it keeps the folder's open handle, so files are written through
/// handles that were opened without following links rather than by path.
struct Staging {
    path: PathBuf,
    relative: PathBuf,
    #[cfg(unix)]
    dir: fs::File,
    kept: bool,
}

impl Staging {
    fn create(root: &Path, area: &Path) -> Result<Self, WorkspaceFilesError> {
        let area_relative = area
            .strip_prefix(root)
            .map_err(|_| checkout_changed())?
            .to_path_buf();
        #[cfg(unix)]
        let area_dir =
            mutations::open_parent(root, &area_relative.join("staging")).map_err(io_error)?;
        for _ in 0..8 {
            let name = format!(".zuse-skill-{}", uuid::Uuid::new_v4().simple());
            #[cfg(unix)]
            let created = at::mkdir(&area_dir, name.as_ref(), 0o700);
            #[cfg(not(unix))]
            let created = fs::create_dir(area.join(&name));
            match created {
                Ok(()) => {
                    return Ok(Self {
                        path: area.join(&name),
                        relative: area_relative.join(&name),
                        #[cfg(unix)]
                        dir: at::open_dir(&area_dir, name.as_ref()).map_err(io_error)?,
                        kept: false,
                    });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error(error)),
            }
        }
        Err(WorkspaceFilesError::Io(
            "Could not create a staging folder for the skill.".into(),
        ))
    }

    /// Leave the folder in place and return where it is.
    fn keep(mut self) -> PathBuf {
        self.kept = true;
        self.path.clone()
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.kept {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn install(
    root: &Path,
    entry: CatalogEntry,
    files: &[(&SkillFile, Vec<u8>)],
) -> Result<(), WorkspaceFilesError> {
    let area = skill_area(root, true)?.ok_or_else(checkout_changed)?;
    let staging = Staging::create(root, &area)?;
    write_package(&staging, files)?;
    move_directory(
        root,
        &staging.relative.join("package"),
        Path::new(&entry.install_path()),
    )
    .map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            existing_skill()
        } else {
            io_error(error)
        }
    })?;
    sync_parent_directory(&area);
    Ok(())
}

/// Writes the downloaded files into `staging/package`. Resource files are data
/// here: installing a skill never runs them.
#[cfg(unix)]
fn write_package(
    staging: &Staging,
    files: &[(&SkillFile, Vec<u8>)],
) -> Result<(), WorkspaceFilesError> {
    at::mkdir(&staging.dir, "package".as_ref(), 0o755).map_err(io_error)?;
    let package = at::open_dir(&staging.dir, "package".as_ref()).map_err(io_error)?;
    for (file, bytes) in files {
        let relative = package_path(file)?;
        let name = relative
            .file_name()
            .ok_or_else(|| bad_path("Invalid path in GitHub skill package"))?;
        // Every folder is opened from its parent's handle without following
        // links, so a folder swapped for a symlink fails instead of
        // redirecting the write.
        let mut directory = package.try_clone().map_err(io_error)?;
        for component in relative.parent().unwrap_or(Path::new("")).components() {
            let std::path::Component::Normal(part) = component else {
                return Err(bad_path("Invalid path in GitHub skill package"));
            };
            match at::mkdir(&directory, part, 0o755) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(io_error(error)),
            }
            directory = at::open_dir(&directory, part).map_err(io_error)?;
        }
        at::create_file(&directory, name, file.executable)
            .and_then(|mut output| output.write_all(bytes))
            .map_err(io_error)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn write_package(
    staging: &Staging,
    files: &[(&SkillFile, Vec<u8>)],
) -> Result<(), WorkspaceFilesError> {
    let staged = staging.path.join("package");
    fs::create_dir(&staged).map_err(io_error)?;
    for (file, bytes) in files {
        let destination = staged.join(package_path(file)?);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .and_then(|mut output| output.write_all(bytes))
            .map_err(io_error)?;
    }
    Ok(())
}

fn remove(root: &Path, entry: CatalogEntry) -> Result<(), WorkspaceFilesError> {
    let area = skill_area(root, false)?.ok_or_else(checkout_changed)?;
    let staging = Staging::create(root, &area)?;
    let target = staging.relative.join("package");
    let relative = entry.install_path();
    move_directory(root, Path::new(&relative), &target).map_err(io_error)?;
    // If files changed just before the move, keep the checkout's work.
    let matches = package_matches(&staging.path.join("package"), entry);
    if !matches!(matches, Ok(true)) {
        if let Err(error) = move_directory(root, &target, Path::new(&relative)) {
            let preserved = staging.keep();
            return Err(WorkspaceFilesError::Io(format!(
                "Could not restore the edited skill: {error}. Its files are in {}.",
                preserved.display()
            )));
        }
        return matches.and(Err(existing_skill()));
    }
    sync_parent_directory(&area);
    Ok(())
}

/// Moves a skill folder without ever replacing one that is already there.
///
/// The exclusive rename (`renameat2(RENAME_NOREPLACE)` on Linux,
/// `renameatx_np(RENAME_EXCL)` on macOS) is missing on some filesystems:
/// older overlayfs in containers, NFS, SMB and many FUSE mounts. There, claim
/// the name with `mkdir` and rename onto it: `rename(2)` only replaces an
/// empty directory, so files written there in between fail the move instead
/// of being lost.
fn move_directory(root: &Path, source: &Path, destination: &Path) -> std::io::Result<()> {
    match mutations::move_no_replace(root, source, destination) {
        #[cfg(unix)]
        Err(error) if exclusive_rename_unsupported(&error) => {
            claim_and_rename(root, source, destination)
        }
        result => result,
    }
}

/// The fallback move, anchored like the exclusive rename to parent handles
/// opened without following links, so no ancestor is resolved again by path.
#[cfg(unix)]
fn claim_and_rename(root: &Path, source: &Path, destination: &Path) -> std::io::Result<()> {
    let from = mutations::open_parent(root, source)?;
    let to = mutations::open_parent(root, destination)?;
    let source = source.file_name().ok_or(ErrorKind::InvalidInput)?;
    let destination = destination.file_name().ok_or(ErrorKind::InvalidInput)?;
    at::mkdir(&to, destination, 0o755)?;
    at::rename(&from, source, &to, destination).inspect_err(|_| {
        let _ = at::rmdir(&to, destination);
    })
}

#[cfg(unix)]
fn exclusive_rename_unsupported(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code) if code == libc::EINVAL
            || code == libc::ENOSYS
            || code == libc::ENOTSUP
            || code == libc::EOPNOTSUPP
    )
}

/// Single-component filesystem calls relative to an open directory handle.
#[cfg(unix)]
mod at {
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    fn name(value: &OsStr) -> io::Result<CString> {
        Ok(CString::new(value.as_bytes())?)
    }

    fn check(result: libc::c_int) -> io::Result<libc::c_int> {
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result)
        }
    }

    pub(super) fn mkdir(parent: &File, child: &OsStr, mode: libc::mode_t) -> io::Result<()> {
        let child = name(child)?;
        // SAFETY: a live directory handle and a NUL-terminated single component.
        check(unsafe { libc::mkdirat(parent.as_raw_fd(), child.as_ptr(), mode) }).map(drop)
    }

    pub(super) fn open_dir(parent: &File, child: &OsStr) -> io::Result<File> {
        let child = name(child)?;
        // SAFETY: as above; O_NOFOLLOW refuses a symlink in place of the folder.
        let fd = check(unsafe {
            libc::openat(
                parent.as_raw_fd(),
                child.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?;
        // SAFETY: openat returned a newly owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    pub(super) fn create_file(parent: &File, child: &OsStr, executable: bool) -> io::Result<File> {
        let child = name(child)?;
        let mode: libc::c_uint = if executable { 0o755 } else { 0o644 };
        // SAFETY: as above; O_CREAT | O_EXCL | O_NOFOLLOW creates a new file only.
        let fd = check(unsafe {
            libc::openat(
                parent.as_raw_fd(),
                child.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                mode,
            )
        })?;
        // SAFETY: openat returned a newly owned descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    pub(super) fn rename(from: &File, source: &OsStr, to: &File, target: &OsStr) -> io::Result<()> {
        let (source, target) = (name(source)?, name(target)?);
        // SAFETY: live directory handles and NUL-terminated single components.
        check(unsafe {
            libc::renameat(
                from.as_raw_fd(),
                source.as_ptr(),
                to.as_raw_fd(),
                target.as_ptr(),
            )
        })
        .map(drop)
    }

    pub(super) fn rmdir(parent: &File, child: &OsStr) -> io::Result<()> {
        let child = name(child)?;
        // SAFETY: as above; removes only an empty directory.
        check(unsafe { libc::unlinkat(parent.as_raw_fd(), child.as_ptr(), libc::AT_REMOVEDIR) })
            .map(drop)
    }
}

fn status(root: &Path, entry: CatalogEntry) -> Result<Option<Installation>, WorkspaceFilesError> {
    let Some(area) = skill_area(root, false)? else {
        return Ok(None);
    };
    let directory = area.join(&entry.package.name);
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.is_dir() && !mutations::is_link(&metadata) => {}
        Ok(_) => return Ok(Some(Installation::Existing)),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error)),
    }
    Ok(Some(if package_matches(&directory, entry)? {
        Installation::Installed
    } else {
        Installation::Existing
    }))
}

fn package_matches(root: &Path, entry: CatalogEntry) -> Result<bool, WorkspaceFilesError> {
    if !fs::symlink_metadata(root).is_ok_and(|meta| meta.is_dir() && !mutations::is_link(&meta)) {
        return Ok(false);
    }
    let expected = entry
        .package
        .files
        .iter()
        .map(|file| Ok((package_path(file)?.to_path_buf(), file)))
        .collect::<Result<HashMap<_, _>, WorkspaceFilesError>>()?;
    let mut directories = vec![PathBuf::new()];
    let mut found = 0;
    while let Some(directory) = directories.pop() {
        for item in fs::read_dir(root.join(&directory)).map_err(io_error)? {
            let item = item.map_err(io_error)?;
            let relative = directory.join(item.file_name());
            let meta = fs::symlink_metadata(item.path()).map_err(io_error)?;
            if mutations::is_link(&meta) {
                return Ok(false);
            }
            if meta.is_dir() && expected.keys().any(|file| file.starts_with(&relative)) {
                directories.push(relative);
                continue;
            }
            let Some(file) = expected.get(&relative).filter(|file| {
                meta.is_file() && meta.len() == file.bytes as u64 && executable_matches(&meta, file)
            }) else {
                return Ok(false);
            };
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY);
            }
            let mut bytes = Vec::new();
            options
                .open(item.path())
                .and_then(|handle| handle.take(file.bytes as u64 + 1).read_to_end(&mut bytes))
                .map_err(io_error)?;
            if format!("{:x}", Sha256::digest(&bytes)) != file.sha256 {
                return Ok(false);
            }
            found += 1;
        }
    }
    Ok(found == expected.len())
}

/// A permission change counts as an edit: the package is kept, not removed.
#[cfg(unix)]
fn executable_matches(meta: &fs::Metadata, file: &SkillFile) -> bool {
    use std::os::unix::fs::PermissionsExt;
    (meta.permissions().mode() & 0o111 != 0) == file.executable
}

#[cfg(not(unix))]
fn executable_matches(_: &fs::Metadata, _: &SkillFile) -> bool {
    true
}

fn package_path(file: &SkillFile) -> Result<&Path, WorkspaceFilesError> {
    let relative = WorkspaceRelativePath::file(&file.path)?;
    if relative.wire_path() != file.path || contains_git_component(relative.as_path()) {
        return Err(bad_path("Invalid path in GitHub skill package"));
    }
    Ok(Path::new(&file.path))
}

fn skill_area(root: &Path, create: bool) -> Result<Option<PathBuf>, WorkspaceFilesError> {
    let mut directory = root.to_path_buf();
    for component in [".agents", "skills"] {
        directory.push(component);
        if create {
            match fs::create_dir(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(io_error(error)),
            }
        }
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if metadata.is_dir() && !mutations::is_link(&metadata) => {}
            Ok(_) => {
                return Err(WorkspaceFilesError::Unsupported(format!(
                    "The skill path component {component} must be a real directory."
                )));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(Some(directory))
}

fn existing_skill() -> WorkspaceFilesError {
    WorkspaceFilesError::Authorization(
        "This skill folder has existing or edited files. Zuse kept them intact.".into(),
    )
}

fn checkout_changed() -> WorkspaceFilesError {
    WorkspaceFilesError::Authorization(
        "Refresh Skills to confirm the current checkout and try again.".into(),
    )
}

fn io_error(error: std::io::Error) -> WorkspaceFilesError {
    WorkspaceFilesError::Io(error.to_string())
}

fn worker_error(error: tokio::task::JoinError) -> WorkspaceFilesError {
    WorkspaceFilesError::Io(format!("Skill catalog worker failed: {error}"))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn fallback_move_never_replaces_an_existing_skill_folder() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (Path::new("staged"), Path::new("skill"));
        fs::create_dir(root.path().join(from)).unwrap();
        fs::write(root.path().join("staged/SKILL.md"), "new").unwrap();

        fs::create_dir(root.path().join(to)).unwrap();
        fs::write(root.path().join("skill/SKILL.md"), "mine").unwrap();
        let error = claim_and_rename(root.path(), from, to).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(
            fs::read_to_string(root.path().join("skill/SKILL.md")).unwrap(),
            "mine"
        );

        fs::remove_dir_all(root.path().join(to)).unwrap();
        claim_and_rename(root.path(), from, to).unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("skill/SKILL.md")).unwrap(),
            "new"
        );
        assert!(!root.path().join(from).exists());
    }

    #[test]
    fn staged_writes_refuse_a_folder_swapped_for_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let area = root.path().join(".agents/skills");
        fs::create_dir_all(&area).unwrap();
        let staging = Staging::create(root.path(), &area).unwrap();
        fs::create_dir(staging.path.join("package")).unwrap();
        std::os::unix::fs::symlink(outside.path(), staging.path.join("package/scripts")).unwrap();
        let package = at::open_dir(&staging.dir, "package".as_ref()).unwrap();
        assert!(at::open_dir(&package, "scripts".as_ref()).is_err());
        assert!(fs::read_dir(outside.path()).unwrap().next().is_none());
        let path = staging.path.clone();
        drop(staging);
        assert!(!path.exists());
    }
}
