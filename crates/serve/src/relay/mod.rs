//! Opaque rendezvous for outbound hosts. Pairing keys never reach the broker.
mod broker;
mod host;
pub use broker::RelayBroker;
pub use host::serve_host;
use std::{io::Read, path::Path, sync::Arc};
use subtle::ConstantTimeEq;
pub use zuse_remote::normalized_origin as validate_origin;
/// Operator-issued routing credential, independent of every Noise pairing key.
#[derive(Clone)]
pub struct HostToken(Arc<String>);
impl HostToken {
    pub fn parse(value: String) -> anyhow::Result<Self> {
        anyhow::ensure!(
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Relay host token must contain 64 hexadecimal characters"
        );
        Ok(Self(Arc::new(value)))
    }
    pub fn read_file(path: &Path) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path)?;
        anyhow::ensure!(
            file.metadata()?.is_file() && file.metadata()?.len() <= 66,
            "Relay token must be a regular file"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        let mut value = String::new();
        file.take(66).read_to_string(&mut value)?;
        Self::parse(value.trim().to_owned())
    }
    fn matches(&self, candidate: &str) -> bool {
        bool::from(self.0.as_bytes().ct_eq(candidate.as_bytes()))
    }
    fn header(&self) -> String {
        format!("Bearer {}", self.0)
    }
}
