//! Checkout operations keep directory descriptors through reads and commits.
use crate::Error;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CStr, CString, OsString},
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
pub(super) const MAX_FILE: usize = 2 * 1024 * 1024;
fn io(error: std::io::Error) -> Error {
    Error::Tool(format!("Filesystem operation failed: {error}"))
}
fn message(value: &str) -> Error {
    Error::Tool(value.into())
}
fn create_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        message(
            "File already exists. Read it with read_file and use the returned hash as expected_hash to replace it.",
        )
    } else {
        io(error)
    }
}
pub(super) fn check(cancel: &CancellationToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(Error::Interrupted)
    } else {
        Ok(())
    }
}
pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn cstr(value: &std::ffi::OsStr) -> Result<CString, Error> {
    CString::new(value.as_bytes()).map_err(|_| message("Path contains a null byte"))
}
fn file(fd: i32) -> Result<File, Error> {
    if fd < 0 {
        Err(io(std::io::Error::last_os_error()))
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
pub(super) struct CheckoutFiles {
    root: PathBuf,
    handle: File,
    locks: PathBuf,
}
impl CheckoutFiles {
    pub fn open(root: &Path, locks: PathBuf) -> Result<Self, Error> {
        let root = std::fs::canonicalize(root).map_err(io)?;
        let handle = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&root)
            .map_err(io)?;
        std::fs::create_dir_all(&locks).map_err(io)?;
        std::fs::set_permissions(&locks, std::fs::Permissions::from_mode(0o700)).map_err(io)?;
        Ok(Self {
            root,
            handle,
            locks,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn current_root(&self) -> Result<(), Error> {
        let current = std::fs::metadata(&self.root).map_err(io)?;
        let held = self.handle.metadata().map_err(io)?;
        if !current.is_dir() || current.dev() != held.dev() || current.ino() != held.ino() {
            return Err(message(
                "The checkout changed. Reopen this conversation before continuing.",
            ));
        }
        Ok(())
    }
    pub fn resolve(&self, input: &str) -> Result<PathBuf, Error> {
        self.current_root()?;
        let path = Path::new(input);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        let mut normalized = PathBuf::new();
        for part in absolute.components() {
            match part {
                Component::CurDir => {}
                Component::ParentDir => {
                    normalized.pop();
                }
                other => normalized.push(other.as_os_str()),
            }
        }
        if !normalized.starts_with(&self.root) {
            return Err(message("Path is outside the checkout"));
        }
        let mut cursor = normalized.as_path();
        let mut missing: Vec<OsString> = vec![];
        loop {
            match std::fs::canonicalize(cursor) {
                Ok(mut canonical) => {
                    if !canonical.starts_with(&self.root) {
                        return Err(message("Path is outside the checkout"));
                    }
                    for name in missing.into_iter().rev() {
                        canonical.push(name);
                    }
                    return Ok(canonical);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(
                        cursor
                            .file_name()
                            .ok_or_else(|| message("Cannot resolve checkout path"))?
                            .into(),
                    );
                    cursor = cursor
                        .parent()
                        .ok_or_else(|| message("Cannot resolve checkout path"))?;
                }
                Err(error) => return Err(io(error)),
            }
        }
    }
    fn directory(&self, path: &Path, create: bool) -> Result<File, Error> {
        self.current_root()?;
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| message("Path is outside the checkout"))?;
        let mut current = self.handle.try_clone().map_err(io)?;
        for part in relative.components() {
            let Component::Normal(name) = part else {
                return Err(message("Invalid resolved checkout path"));
            };
            let name = cstr(name)?;
            let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
            let mut fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0
                && create
                && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
            {
                let result = unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o755) };
                if result < 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(io(std::io::Error::last_os_error()));
                }
                fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
            }
            current = file(fd)?;
        }
        Ok(current)
    }
    fn parent(&self, path: &Path, create: bool) -> Result<(File, CString), Error> {
        if path == self.root {
            return Err(message("Expected a file below the checkout root"));
        }
        let parent = self.directory(
            path.parent().ok_or_else(|| message("Invalid file path"))?,
            create,
        )?;
        Ok((
            parent,
            cstr(
                path.file_name()
                    .ok_or_else(|| message("Invalid file name"))?,
            )?,
        ))
    }
    fn read_at(parent: &File, name: &CStr, text: bool) -> Result<(Vec<u8>, u32), Error> {
        let file = file(unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        })?;
        let metadata = file.metadata().map_err(io)?;
        if !metadata.is_file() || metadata.len() > MAX_FILE as u64 {
            return Err(message("Expected a regular file no larger than 2 MiB"));
        }
        let mut data = Vec::with_capacity((metadata.len() as usize).min(MAX_FILE));
        file.take((MAX_FILE + 1) as u64)
            .read_to_end(&mut data)
            .map_err(io)?;
        if data.len() > MAX_FILE {
            return Err(message("File exceeds 2 MiB"));
        }
        if text && (data.contains(&0) || std::str::from_utf8(&data).is_err()) {
            return Err(message("Expected a UTF-8 text file"));
        }
        Ok((data, metadata.mode() & 0o777))
    }
    pub fn read(&self, path: &Path, text: bool) -> Result<Vec<u8>, Error> {
        let (parent, name) = self.parent(path, false)?;
        Self::read_at(&parent, &name, text).map(|(data, _)| data)
    }
    pub fn list(&self, path: &Path) -> Result<Value, Error> {
        let directory = self.directory(path, false)?;
        let raw = directory.into_raw_fd();
        let stream = unsafe { libc::fdopendir(raw) };
        if stream.is_null() {
            let error = std::io::Error::last_os_error();
            drop(unsafe { File::from_raw_fd(raw) });
            return Err(io(error));
        }
        struct Directory(*mut libc::DIR);
        impl Drop for Directory {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let stream = Directory(stream);
        let mut entries = vec![];
        loop {
            #[cfg(target_os = "linux")]
            unsafe {
                *libc::__errno_location() = 0;
            }
            #[cfg(target_os = "macos")]
            unsafe {
                *libc::__error() = 0;
            }
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(0) {
                    return Err(io(error));
                }
                break;
            }
            let entry = unsafe { &*entry };
            let name = unsafe { CStr::from_ptr(entry.d_name.as_ptr()) };
            if matches!(name.to_bytes(), b"." | b"..") {
                continue;
            }
            if entries.len() >= 50000 {
                return Err(message(
                    "Directory exceeds 50000 entries. Narrow the path or use glob.",
                ));
            }
            let mut kind = entry.d_type;
            if kind == libc::DT_UNKNOWN {
                let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
                if unsafe {
                    libc::fstatat(
                        libc::dirfd(stream.0),
                        name.as_ptr(),
                        metadata.as_mut_ptr(),
                        libc::AT_SYMLINK_NOFOLLOW,
                    )
                } < 0
                {
                    return Err(io(std::io::Error::last_os_error()));
                }
                kind = match unsafe { metadata.assume_init() }.st_mode & libc::S_IFMT {
                    libc::S_IFDIR => libc::DT_DIR,
                    libc::S_IFLNK => libc::DT_LNK,
                    _ => libc::DT_REG,
                };
            }
            entries.push(json!({"name":name.to_string_lossy(),"type":match kind {libc::DT_DIR=>"directory",libc::DT_LNK=>"symlink",_=>"file"}}));
        }
        entries.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        Ok(Value::Array(entries))
    }
    fn edit_lock(&self, path: &Path, cancel: &CancellationToken) -> Result<File, Error> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(
                self.locks
                    .join(format!("{}.lock", digest(path.as_os_str().as_bytes()))),
            )
            .map_err(io)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            check(cancel)?;
            match lock.try_lock() {
                Ok(()) => return Ok(lock),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(message(
                        "Another agent is editing this file. Retry with a fresh read.",
                    ));
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(io(error)),
            }
        }
    }
    pub fn change(
        &self,
        path: &Path,
        expected: Option<&str>,
        transform: impl FnOnce(&str) -> Result<String, Error>,
        cancel: &CancellationToken,
    ) -> Result<Value, Error> {
        check(cancel)?;
        let _lock = self.edit_lock(path, cancel)?;
        if self.resolve(
            path.to_str()
                .ok_or_else(|| message("Expected a UTF-8 path"))?,
        )? != path
        {
            return Err(message("Path changed before edit"));
        }
        let (parent, name) = self.parent(path, true)?;
        if expected.is_none() {
            // Reject a known create conflict before constructing and syncing a
            // replacement. linkat below still arbitrates concurrent creation.
            let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe {
                libc::fstatat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    metadata.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } == 0
            {
                return Err(create_error(std::io::ErrorKind::AlreadyExists.into()));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(io(error));
            }
        }
        let previous = expected
            .map(|_| Self::read_at(&parent, &name, true))
            .transpose()?;
        if let (Some(expected), Some((bytes, _))) = (expected, &previous)
            && digest(bytes) != expected
        {
            return Err(message("Stale file hash. Read the file again."));
        }
        let content = previous.as_ref().map_or(Ok(""), |(bytes, _)| {
            std::str::from_utf8(bytes).map_err(|_| message("Expected UTF-8 text"))
        })?;
        let next = transform(content)?;
        if next.len() > MAX_FILE {
            return Err(message("Edited file exceeds 2 MiB"));
        }
        check(cancel)?;
        let temporary = CString::new(format!(".zuse-{}.tmp", uuid::Uuid::new_v4())).unwrap();
        // C variadic arguments promote macOS's 16-bit mode_t to an int.
        let mut file = file(unsafe {
            libc::openat(
                parent.as_raw_fd(),
                temporary.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                previous.as_ref().map_or(0o644, |(_, mode)| *mode) as libc::c_uint,
            )
        })?;
        struct Temporary<'a> {
            parent: &'a File,
            name: &'a CStr,
        }
        impl Drop for Temporary<'_> {
            fn drop(&mut self) {
                unsafe {
                    libc::unlinkat(self.parent.as_raw_fd(), self.name.as_ptr(), 0);
                }
            }
        }
        let _temporary = Temporary {
            parent: &parent,
            name: &temporary,
        };
        file.write_all(next.as_bytes()).map_err(io)?;
        if let Some((_, mode)) = &previous {
            file.set_permissions(std::fs::Permissions::from_mode(*mode))
                .map_err(io)?;
        }
        file.sync_all().map_err(io)?;
        check(cancel)?;
        self.current_root()?;
        if let Some(expected) = expected {
            if digest(&Self::read_at(&parent, &name, true)?.0) != expected {
                return Err(message("File changed during edit. Read it again."));
            }
            if unsafe {
                libc::renameat(
                    parent.as_raw_fd(),
                    temporary.as_ptr(),
                    parent.as_raw_fd(),
                    name.as_ptr(),
                )
            } < 0
            {
                return Err(io(std::io::Error::last_os_error()));
            }
        } else if unsafe {
            libc::linkat(
                parent.as_raw_fd(),
                temporary.as_ptr(),
                parent.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        } < 0
        {
            return Err(create_error(std::io::Error::last_os_error()));
        }
        parent.sync_all().map_err(io)?;
        Ok(json!({"path":path,"hash":digest(next.as_bytes()),"bytes":next.len()}))
    }
}
pub(super) fn replace(content: &str, edit: &Value) -> Result<String, Error> {
    let old = edit["old_string"]
        .as_str()
        .ok_or_else(|| message("Missing edit text"))?;
    let new = edit["new_string"]
        .as_str()
        .ok_or_else(|| message("Missing replacement text"))?;
    let first = content
        .find(old)
        .ok_or_else(|| message("Edit text was not found"))?;
    if edit["replace_all"] == true {
        return Ok(content.replace(old, new));
    }
    if content.as_bytes()[first + 1..]
        .windows(old.len())
        .any(|part| part == old.as_bytes())
    {
        return Err(message("Ambiguous edit. Use replace_all or more context."));
    }
    let mut result = String::with_capacity(
        content
            .len()
            .saturating_sub(old.len())
            .saturating_add(new.len()),
    );
    result.push_str(&content[..first]);
    result.push_str(new);
    result.push_str(&content[first + old.len()..]);
    Ok(result)
}

#[cfg(test)]
mod creation_tests {
    use super::*;

    #[test]
    fn existing_file_rejects_creation_before_constructing_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let files = CheckoutFiles::open(directory.path(), directory.path().join("locks")).unwrap();
        let path = directory.path().join("existing.txt");
        std::fs::write(&path, "original").unwrap();
        let result = files.change(
            &path,
            None,
            |_| panic!("creation conflict must be detected before replacement work"),
            &CancellationToken::new(),
        );
        assert!(
            matches!(result, Err(Error::Tool(message)) if message.contains("read_file") && message.contains("expected_hash"))
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "original");
    }

    #[test]
    fn creation_race_preserves_the_other_writers_content() {
        let directory = tempfile::tempdir().unwrap();
        let files = CheckoutFiles::open(directory.path(), directory.path().join("locks")).unwrap();
        let path = directory.path().join("racing.txt");
        let result = files.change(
            &path,
            None,
            |_| {
                std::fs::write(&path, "other writer").unwrap();
                Ok("replacement".into())
            },
            &CancellationToken::new(),
        );
        assert!(matches!(result, Err(Error::Tool(message)) if message.contains("read_file")));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "other writer");
        assert!(!std::fs::read_dir(directory.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".zuse-")
        }));
    }
}
