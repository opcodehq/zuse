//! Local wake delivery. A parked guest waits for an explicit lifecycle dispatch,
//! not API polling or keepalive traffic. Authority still validates the operation.
use crate::DataLock;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};
use tokio_util::sync::CancellationToken;
const LIMIT: u64 = 2048;
const DIRECTORY: &str = ".runtime-memory";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Wake {
    pub workspace_id: String,
    pub generation: i64,
    pub storage_incarnation_id: String,
    pub operation_id: String,
}
impl Wake {
    fn valid(&self) -> bool {
        self.generation > 0
            && [
                &self.workspace_id,
                &self.storage_incarnation_id,
                &self.operation_id,
            ]
            .iter()
            .all(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
    }
}
pub struct WakeListener {
    listener: UnixListener,
    path: PathBuf,
    device: u64,
    inode: u64,
    workspace: String,
    generation: i64,
    incarnation: String,
    _lock: DataLock,
}
fn path(data: &Path) -> PathBuf {
    data.join(DIRECTORY).join("wake.sock")
}
fn private_socket(path: &Path) -> Result<std::fs::Metadata> {
    let m = std::fs::symlink_metadata(path)?;
    ensure!(
        m.file_type().is_socket() && m.mode() & 0o077 == 0 && m.uid() == unsafe { libc::geteuid() },
        "runtime_wake_socket_invalid"
    );
    Ok(m)
}
impl WakeListener {
    pub fn bind(data: &Path, workspace: &str, generation: i64, incarnation: &str) -> Result<Self> {
        ensure!(
            Wake {
                workspace_id: workspace.into(),
                generation,
                storage_incarnation_id: incarnation.into(),
                operation_id: "validation".into()
            }
            .valid(),
            "runtime_wake_binding_invalid"
        );
        let directory = data.join(DIRECTORY);
        std::fs::create_dir_all(&directory)?;
        let metadata = std::fs::symlink_metadata(&directory)?;
        ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() },
            "runtime_wake_directory_invalid"
        );
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
        // Independent lock forbids unlinking another live listener, even within
        // the same guest process. A crashed process releases its kernel lock.
        let lock = DataLock::acquire(&directory, None)?;
        let path = path(data);
        if path.symlink_metadata().is_ok() {
            private_socket(&path)?;
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let m = private_socket(&path)?;
        Ok(Self {
            listener,
            path,
            device: m.dev(),
            inode: m.ino(),
            workspace: workspace.into(),
            generation,
            incarnation: incarnation.into(),
            _lock: lock,
        })
    }
    pub async fn wait(&self, cancel: &CancellationToken) -> Result<Option<Wake>> {
        loop {
            let (mut stream, _) = tokio::select! {_=cancel.cancelled()=>return Ok(None),accepted=self.listener.accept()=>accepted?};
            let read = async {
                let mut bytes = Vec::new();
                (&mut stream)
                    .take(LIMIT + 1)
                    .read_to_end(&mut bytes)
                    .await?;
                ensure!(bytes.len() <= LIMIT as usize, "runtime_wake_too_large");
                let wake: Wake = serde_json::from_slice(&bytes)?;
                ensure!(
                    wake.valid()
                        && wake.workspace_id == self.workspace
                        && wake.generation == self.generation
                        && wake.storage_incarnation_id == self.incarnation,
                    "runtime_wake_binding_mismatch"
                );
                stream.write_all(b"1").await?;
                Ok::<_, anyhow::Error>(wake)
            };
            let result = tokio::select! {_=cancel.cancelled()=>return Ok(None),result=tokio::time::timeout(std::time::Duration::from_secs(1),read)=>result};
            // Ignore malformed local clients without logging their frame. The
            // authority, not this notification, authorizes admission reopening.
            if let Ok(Ok(wake)) = result {
                return Ok(Some(wake));
            }
        }
    }
}
impl Drop for WakeListener {
    fn drop(&mut self) {
        if let Ok(m) = std::fs::symlink_metadata(&self.path) {
            if m.file_type().is_socket() && m.dev() == self.device && m.ino() == self.inode {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}
/// False means no parked listener exists: the caller may start the retained guest
/// normally. A delivered notification is not a ready/executed-command receipt.
pub async fn notify(data: &Path, wake: &Wake) -> Result<bool> {
    ensure!(wake.valid(), "runtime_wake_binding_invalid");
    let path = path(data);
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
        Ok(_) => {
            private_socket(&path)?;
        }
    }
    let dispatch = async {
        let mut stream = match UnixStream::connect(&path).await {
            Ok(stream) => stream,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        let bytes = serde_json::to_vec(wake)?;
        ensure!(bytes.len() <= LIMIT as usize, "runtime_wake_too_large");
        stream.write_all(&bytes).await?;
        stream.shutdown().await?;
        let mut ack = [0];
        stream.read_exact(&mut ack).await?;
        ensure!(ack == *b"1", "runtime_wake_not_acknowledged");
        Ok(true)
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), dispatch)
        .await
        .map_err(|_| anyhow::anyhow!("runtime_wake_delivery_timed_out"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wake() -> Wake {
        Wake {
            workspace_id: "task".into(),
            generation: 2,
            storage_incarnation_id: "disk".into(),
            operation_id: "resume-operation".into(),
        }
    }
    #[tokio::test]
    async fn wake_is_explicit_scoped_and_exclusive_without_polling() {
        let root = tempfile::tempdir().unwrap();
        assert!(!notify(root.path(), &wake()).await.unwrap());
        let listener = WakeListener::bind(root.path(), "task", 2, "disk").unwrap();
        assert!(WakeListener::bind(root.path(), "task", 2, "disk").is_err());
        let cancel = CancellationToken::new();
        let request = wake();
        let send = notify(root.path(), &request);
        let (received, sent) = tokio::join!(listener.wait(&cancel), send);
        assert!(sent.unwrap());
        assert_eq!(received.unwrap(), Some(wake()));
        drop(listener);
        assert!(!path(root.path()).exists());
    }
    #[tokio::test]
    async fn stale_generation_and_oversized_frames_cannot_wake_the_current_runtime() {
        let root = tempfile::tempdir().unwrap();
        let listener = WakeListener::bind(root.path(), "task", 2, "disk").unwrap();
        let cancel = CancellationToken::new();
        let send = async {
            let mut wrong = wake();
            wrong.generation = 1;
            assert!(notify(root.path(), &wrong).await.is_err());
            let mut socket = UnixStream::connect(path(root.path())).await.unwrap();
            socket
                .write_all(&vec![b'a'; LIMIT as usize + 1])
                .await
                .unwrap();
            socket.shutdown().await.unwrap();
            notify(root.path(), &wake()).await.unwrap()
        };
        let (received, sent) = tokio::join!(listener.wait(&cancel), send);
        assert!(sent);
        assert_eq!(received.unwrap(), Some(wake()));
    }
    #[tokio::test]
    async fn regular_files_and_symlinks_are_never_replaced_and_cancellation_is_idle() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(DIRECTORY)).unwrap();
        std::fs::write(path(root.path()), b"retained user data").unwrap();
        assert!(WakeListener::bind(root.path(), "task", 2, "disk").is_err());
        assert_eq!(
            std::fs::read(path(root.path())).unwrap(),
            b"retained user data"
        );
        std::fs::remove_file(path(root.path())).unwrap();
        // A process crash releases the kernel lock but leaves its socket node.
        let stale = std::os::unix::net::UnixListener::bind(path(root.path())).unwrap();
        std::fs::set_permissions(path(root.path()), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        drop(stale);
        let listener = WakeListener::bind(root.path(), "task", 2, "disk").unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(listener.wait(&cancel).await.unwrap().is_none());
        drop(listener);
        std::os::unix::fs::symlink(root.path().join("other"), path(root.path())).unwrap();
        assert!(WakeListener::bind(root.path(), "task", 2, "disk").is_err());
        assert!(
            notify(root.path(), &wake()).await.is_err(),
            "a dangling symlink is not a cold-start allowance"
        );
    }
}
