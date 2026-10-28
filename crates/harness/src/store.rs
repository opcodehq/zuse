//! Atomic private checkpoints. A damaged journal is preserved, never reset.
use crate::{Checkpoint, Error};
use serde::Serialize;
use std::{
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};
pub struct FileStore {
    path: PathBuf,
}
impl FileStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn load(&self) -> Result<Option<Checkpoint>, Error> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Storage),
        };
        let state: Checkpoint =
            serde_json::from_slice(&bytes).map_err(|_| Error::InvalidCheckpoint)?;
        state.validate()?;
        Ok(Some(state))
    }
    pub fn save(&self, state: &Checkpoint) -> Result<(), Error> {
        state.validate()?;
        let directory = self.path.parent().ok_or(Error::Storage)?;
        std::fs::create_dir_all(directory).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| Error::Storage)?;
        }
        save_json(&self.path, state)
    }
}

/// One private, atomic commit path for checkpoints and process metadata.
/// The caller owns creation and permissions of its private parent directory.
pub(crate) fn save_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let directory = path.parent().ok_or(Error::Storage)?;
    let temp = directory.join(format!(".journal-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file =
            BufWriter::with_capacity(64 * 1024, options.open(&temp).map_err(|_| Error::Storage)?);
        serde_json::to_writer(&mut file, value).map_err(|_| Error::Storage)?;
        file.flush()
            .and_then(|_| file.get_ref().sync_all())
            .map_err(|_| Error::Storage)?;
        std::fs::rename(&temp, path).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        std::fs::File::open(directory)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Storage)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}
