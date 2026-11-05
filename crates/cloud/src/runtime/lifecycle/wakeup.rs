//! Private same-host scheduling hint. The original durable job remains authority.
//! A dropped hint or disabled listener falls back to bounded reconciliation.
use std::path::{Path, PathBuf};
pub struct Listener {
    #[cfg(unix)]
    inner: Option<crate::events::hint::Listener>,
}
fn directory() -> Option<PathBuf> {
    std::env::var_os("ZUSE_CLOUD_EVENT_SIGNAL_DIRECTORY")
        .map(|root| PathBuf::from(root).join("lifecycle"))
}
impl Listener {
    pub fn allocation_from_env() -> anyhow::Result<Self> {
        #[cfg(unix)]
        if let Some(root) = std::env::var_os("ZUSE_CLOUD_EVENT_SIGNAL_DIRECTORY") {
            return Ok(Self { inner: Some(crate::events::hint::Listener::bind(&PathBuf::from(root).join("allocation"))?) });
        }
        Ok(Self::disabled())
    }
    pub fn disabled() -> Self {
        Self {
            #[cfg(unix)]
            inner: None,
        }
    }
    pub fn from_env() -> anyhow::Result<Self> {
        #[cfg(unix)]
        if let Some(path) = directory() {
            return Ok(Self {
                inner: Some(crate::events::hint::Listener::bind(&path)?),
            });
        }
        Ok(Self::disabled())
    }
    pub fn drain_ready(&self) -> std::io::Result<usize> {
        #[cfg(unix)]
        if let Some(inner) = &self.inner {
            return inner.drain_ready();
        }
        Ok(0)
    }
    pub async fn receive(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(inner) = &self.inner {
            return inner.receive().await;
        }
        std::future::pending().await
    }
}
pub(crate) fn send() {
    #[cfg(unix)]
    if let Some(path) = directory() {
        send_to(&path);
        if let Some(root) = path.parent() { send_to(&root.join("allocation")); }
    }
}
#[cfg(unix)]
fn send_to(directory: &Path) {
    let _ = (|| -> std::io::Result<()> {
        let socket = std::os::unix::net::UnixDatagram::unbound()?;
        socket.set_nonblocking(true)?;
        socket.send_to(b"1", directory.join("wake.sock"))?;
        Ok(())
    })();
}
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn lifecycle_hints_are_isolated_private_and_optional() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("lifecycle");
        send_to(&path); // No listener: never affects admission or panics.
        let listener = crate::events::hint::Listener::bind(&path).unwrap();
        assert!(
            !root.path().join("wake.sock").exists(),
            "account feed socket is separate"
        );
        assert_eq!(
            std::fs::metadata(path.join("wake.sock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        send_to(&path);
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.receive())
            .await
            .unwrap()
            .unwrap();
        let disabled = Listener::disabled();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), disabled.receive())
                .await
                .is_err(),
            "disabled listener cannot busy-loop"
        );
        send_to(&path);
        send_to(&path);
        send_to(&path);
        listener.receive().await.unwrap();
        assert_eq!(
            listener.drain_ready().unwrap(),
            2,
            "queued hints coalesce before the durable read"
        );
        send_to(&path);
        tokio::time::timeout(std::time::Duration::from_millis(100), listener.receive())
            .await
            .unwrap()
            .unwrap();
        drop(listener);
        send_to(&path); // Original durable timer remains recovery.
    }
}
