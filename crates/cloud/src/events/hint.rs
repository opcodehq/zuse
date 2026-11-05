//! Same-host, same-environment wake hints for the durable outbox dispatcher.
//! Losing a datagram is harmless: bounded server reconciliation still drains it.
//! Hints contain no owner IDs, credentials, message bytes or provider commands.
#[cfg(unix)]
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[cfg(unix)]
pub struct Listener {
    socket: tokio::net::UnixDatagram,
    path: PathBuf,
    inode: u64,
    device: u64,
    _lock: std::fs::File,
}
#[cfg(unix)]
impl Listener {
    pub fn bind(directory: &Path) -> anyhow::Result<Self> {
        use anyhow::ensure;
        ensure!(
            directory.is_absolute(),
            "account_event_signal_directory_invalid"
        );
        std::fs::create_dir_all(directory)?;
        let metadata = std::fs::symlink_metadata(directory)?;
        ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() },
            "account_event_signal_directory_invalid"
        );
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        let lock_path = directory.join("dispatcher.lock");
        if let Ok(metadata) = std::fs::symlink_metadata(&lock_path) {
            ensure!(
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.uid() == unsafe { libc::geteuid() },
                "account_event_signal_lock_invalid"
            );
        }
        use std::os::unix::fs::OpenOptionsExt;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(lock_path)?;
        lock.try_lock()
            .map_err(|_| anyhow::anyhow!("account_event_dispatcher_already_listening"))?;
        let path = directory.join("wake.sock");
        if let Ok(metadata) = std::fs::symlink_metadata(&path) {
            ensure!(
                metadata.file_type().is_socket()
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.mode() & 0o077 == 0,
                "account_event_signal_socket_invalid"
            );
            std::fs::remove_file(&path)?;
        }
        let socket = tokio::net::UnixDatagram::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let metadata = std::fs::symlink_metadata(&path)?;
        Ok(Self {
            socket,
            path,
            inode: metadata.ino(),
            device: metadata.dev(),
            _lock: lock,
        })
    }
    /// Coalesce already queued best-effort hints, never drain without a bound.
    /// The caller still reconciles durable jobs if a concurrent hint is lost.
    pub fn drain_ready(&self) -> std::io::Result<usize> {
        let mut consumed = 0;
        let mut bytes = [0; 16];
        for _ in 0..64 {
            match self.socket.try_recv(&mut bytes) {
                Ok(size) => {
                    if size == 1 && bytes[0] == b'1' {
                        consumed += 1;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    return Ok(consumed);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(consumed)
    }
    pub async fn receive(&self) -> std::io::Result<()> {
        let mut bytes = [0; 16];
        loop {
            let size = self.socket.recv(&mut bytes).await?;
            if size == 1 && bytes[0] == b'1' {
                return Ok(());
            }
        }
    }
}
#[cfg(unix)]
impl Drop for Listener {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
            if metadata.file_type().is_socket()
                && metadata.ino() == self.inode
                && metadata.dev() == self.device
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}
pub fn send() {
    #[cfg(unix)]
    if let Some(directory) = std::env::var_os("ZUSE_CLOUD_EVENT_SIGNAL_DIRECTORY") {
        let result = (|| -> std::io::Result<()> {
            let socket = std::os::unix::net::UnixDatagram::unbound()?;
            socket.set_nonblocking(true)?;
            socket.send_to(b"1", Path::new(&directory).join("wake.sock"))?;
            Ok(())
        })();
        // A failed/lost hint does not roll back a committed user command. The
        // recovery timer observes the durable outbox rather than replaying it.
        let _ = result;
    }
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn signal_is_private_and_a_second_listener_cannot_unlink_the_first() {
        let directory = tempfile::tempdir().unwrap();
        let listener = Listener::bind(directory.path()).unwrap();
        assert!(Listener::bind(directory.path()).is_err());
        let path = directory.path().join("wake.sock");
        assert_eq!(
            std::fs::symlink_metadata(&path).unwrap().mode() & 0o777,
            0o600
        );
        let sender = tokio::net::UnixDatagram::unbound().unwrap();
        sender.send_to(b"1", &path).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_millis(500), listener.receive())
            .await
            .unwrap()
            .unwrap();
        drop(listener);
        assert!(!path.exists());
        let replacement = Listener::bind(directory.path()).unwrap();
        drop(replacement);
    }
}
