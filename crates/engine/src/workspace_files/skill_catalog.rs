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

fn install(
    root: &Path,
    entry: CatalogEntry,
    files: &[(&SkillFile, Vec<u8>)],
) -> Result<(), WorkspaceFilesError> {
    let area = skill_area(root, true)?.ok_or_else(checkout_changed)?;
    let staging = tempfile::Builder::new()
        .prefix(".zuse-skill-")
        .tempdir_in(&area)
        .map_err(io_error)?;
    let staged = staging.path().join("package");
    fs::create_dir(&staged).map_err(io_error)?;
    for (file, bytes) in files {
        let relative = package_path(file)?;
        let destination = staged.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        // Resource files are data here. Installing a skill never runs them.
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if file.executable {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o755);
        }
        let mut output = options.open(destination).map_err(io_error)?;
        output.write_all(bytes).map_err(io_error)?;
    }
    let source = staged.strip_prefix(root).map_err(|_| checkout_changed())?;
    move_directory(root, source, Path::new(&entry.install_path())).map_err(|error| {
        if error.kind() == ErrorKind::AlreadyExists {
            existing_skill()
        } else {
            io_error(error)
        }
    })?;
    sync_parent_directory(&area);
    Ok(())
}

fn remove(root: &Path, entry: CatalogEntry) -> Result<(), WorkspaceFilesError> {
    let area = skill_area(root, false)?.ok_or_else(checkout_changed)?;
    let staging = tempfile::Builder::new()
        .prefix(".zuse-skill-")
        .tempdir_in(&area)
        .map_err(io_error)?;
    let staged = staging.path().join("package");
    let target = staged.strip_prefix(root).map_err(|_| checkout_changed())?;
    let relative = entry.install_path();
    move_directory(root, Path::new(&relative), target).map_err(io_error)?;
    // If files changed just before the move, keep the checkout's work.
    let matches = package_matches(&staged, entry);
    if !matches!(matches, Ok(true)) {
        if let Err(error) = move_directory(root, target, Path::new(&relative)) {
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
            claim_and_rename(&root.join(source), &root.join(destination))
        }
        result => result,
    }
}

#[cfg(unix)]
fn claim_and_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir(to)?;
    fs::rename(from, to).inspect_err(|_| {
        let _ = fs::remove_dir(to);
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
            let Some(file) = expected
                .get(&relative)
                .filter(|file| meta.is_file() && meta.len() == file.bytes as u64)
            else {
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
        let (from, to) = (root.path().join("staged"), root.path().join("skill"));
        fs::create_dir(&from).unwrap();
        fs::write(from.join("SKILL.md"), "new").unwrap();

        fs::create_dir(&to).unwrap();
        fs::write(to.join("SKILL.md"), "mine").unwrap();
        let error = claim_and_rename(&from, &to).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(to.join("SKILL.md")).unwrap(), "mine");

        fs::remove_dir_all(&to).unwrap();
        claim_and_rename(&from, &to).unwrap();
        assert_eq!(fs::read_to_string(to.join("SKILL.md")).unwrap(), "new");
        assert!(!from.exists());
    }
}
