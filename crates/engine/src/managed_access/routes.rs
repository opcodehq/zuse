//! Per-account remembered endpoints. A saved route never grants authorization.
use super::{Error, Result, valid_id};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Route {
    pub origin: String,
    pub last_verified_at: i64,
    pub wire_protocol_version: u32,
    pub protocol: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Computer {
    pub environment_id: String,
    pub label: String,
    pub routes: Vec<Route>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved {
    version: u32,
    subject: String,
    computers: Vec<Computer>,
}
pub(super) struct Routes {
    root: PathBuf,
    gate: Mutex<()>,
}
fn valid_origin(origin: &str) -> bool {
    zuse_remote::normalized_origin(origin).is_ok_and(|normalized| normalized == origin)
}
impl Routes {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.join("managed-access/routes"),
            gate: Mutex::new(()),
        }
    }
    fn path(&self, subject: &str) -> PathBuf {
        self.root
            .join(format!("{:x}.json", Sha256::digest(subject.as_bytes())))
    }
    fn read(&self, subject: &str) -> Result<Saved> {
        let path = self.path(subject);
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Saved {
                    version: 1,
                    subject: subject.into(),
                    computers: vec![],
                });
            }
            Err(_) => return Err(Error::Storage),
        };
        if !meta.is_file() || meta.len() > 262144 {
            return Err(Error::Invalid);
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| Error::Storage)?
            .take(262145)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        if bytes.len() > 262144 {
            return Err(Error::Invalid);
        }
        let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
        if saved.version != 1
            || saved.subject != subject
            || saved.computers.len() > 256
            || saved.computers.iter().any(|c| {
                !valid_id(&c.environment_id)
                    || c.label.is_empty()
                    || c.label.len() > 128
                    || c.routes.is_empty()
                    || c.routes.len() > 8
                    || c.routes
                        .iter()
                        .any(|r| !valid_origin(&r.origin) || r.last_verified_at <= 0)
            })
        {
            return Err(Error::Invalid);
        }
        Ok(saved)
    }
    fn write(&self, saved: &Saved) -> Result<()> {
        std::fs::create_dir_all(&self.root).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| Error::Storage)?;
        }
        let bytes = serde_json::to_vec(saved).map_err(|_| Error::Storage)?;
        if bytes.len() > 262144 {
            return Err(Error::Invalid);
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.root).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| Error::Storage)?;
        }
        file.write_all(&bytes)
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| Error::Storage)?;
        file.persist(self.path(&saved.subject))
            .map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        std::fs::File::open(&self.root)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Storage)?;
        Ok(())
    }
    pub fn list(&self, subject: &str) -> Result<Vec<Computer>> {
        let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        Ok(self.read(subject)?.computers)
    }
    pub fn remember(
        &self,
        subject: &str,
        environment: &str,
        label: &str,
        origin: &str,
    ) -> Result<Vec<Computer>> {
        let label = label.trim();
        let origin = zuse_remote::normalized_origin(origin).map_err(|_| Error::Invalid)?;
        if !valid_id(environment)
            || label.is_empty()
            || label.len() > 128
            || label.chars().any(char::is_control)
        {
            return Err(Error::Invalid);
        }
        let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        let mut saved = self.read(subject)?;
        let mut computer = saved
            .computers
            .iter()
            .position(|c| c.environment_id == environment)
            .map(|i| saved.computers.remove(i))
            .unwrap_or(Computer {
                environment_id: environment.into(),
                label: label.into(),
                routes: vec![],
            });
        computer.label = label.into();
        computer.routes.retain(|route| route.origin != origin);
        computer.routes.insert(
            0,
            Route {
                origin,
                last_verified_at: chrono::Utc::now().timestamp_millis(),
                wire_protocol_version: zuse_protocol::managed_connection::WIRE_VERSION,
                protocol: zuse_protocol::managed_connection::PROTOCOL.into(),
            },
        );
        computer.routes.truncate(8);
        saved.computers.insert(0, computer);
        saved.computers.truncate(256);
        self.write(&saved)?;
        Ok(saved.computers)
    }
    pub fn forget(&self, subject: &str, environment: &str) -> Result<Vec<Computer>> {
        if !valid_id(environment) {
            return Err(Error::Invalid);
        }
        let _gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
        let mut saved = self.read(subject)?;
        saved.computers.retain(|c| c.environment_id != environment);
        self.write(&saved)?;
        Ok(saved.computers)
    }
}
impl Computer {
    pub fn record(&self) -> serde_json::Value {
        serde_json::json!({"environmentId":self.environment_id,"label":self.label,"routes":self.routes,"saved":true,"serviceState":"saved","wireProtocolVersion":self.routes[0].wire_protocol_version,"capabilities":{"version":1,"features":[self.routes[0].protocol]}})
    }
}
