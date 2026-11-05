//! Reuse the launcher lock and bind the canonical Rust database before execution.
use crate::identity::atomic_write;
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    path::{Component, Path, PathBuf},
};
use zuse_protocol::cloud::{
    CloudRuntimeBootstrap, CloudRuntimeStorage, RUST_RUNTIME_STORAGE_FORMAT,
};

mod boot;

pub struct DataLock {
    _file: File,
}
impl DataLock {
    pub fn acquire(directory: &Path, inherited: Option<i32>) -> Result<Self> {
        let fresh = !directory.exists();
        std::fs::create_dir_all(directory)?;
        #[cfg(unix)]
        if fresh {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(not(unix))]
        let _ = fresh;
        let path = directory.join(".workspace-runtime.lock");
        #[cfg(target_os = "linux")]
        if let Some(fd) = inherited {
            use std::os::{fd::FromRawFd, unix::fs::MetadataExt};
            ensure!(fd >= 3, "runtime_data_lock_invalid");
            let expected = std::fs::metadata(&path)?;
            let actual = std::fs::metadata(format!("/proc/self/fd/{fd}"))?;
            let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{fd}"))?;
            ensure!(
                expected.dev() == actual.dev()
                    && expected.ino() == actual.ino()
                    && info.lines().any(|s| s.starts_with("lock:")
                        && s.contains("FLOCK")
                        && s.contains("ADVISORY")
                        && s.contains("WRITE")),
                "runtime_data_lock_invalid"
            );
            let owned = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
            ensure!(owned >= 0, "runtime_data_lock_invalid");
            return Ok(Self {
                _file: unsafe { File::from_raw_fd(owned) },
            });
        }
        #[cfg(not(target_os = "linux"))]
        ensure!(inherited.is_none(), "runtime_inherited_lock_unsupported");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        file.try_lock()
            .map_err(|_| anyhow::anyhow!("runtime_data_lock_held"))?;
        Ok(Self { _file: file })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Marker {
    version: u8,
    enrollment_id: String,
    workspace_id: String,
    account_id: String,
    storage: CloudRuntimeStorage,
    database: String,
    ready: bool,
}
pub struct Storage {
    marker: PathBuf,
    binding: Marker,
    database: PathBuf,
}
impl Storage {
    pub fn enrollment(directory: &Path) -> Result<Option<String>> {
        match std::fs::read(directory.join("rust-runtime-storage.json")) {
            Ok(bytes) => {
                let marker: Marker = serde_json::from_slice(&bytes)
                    .map_err(|_| anyhow::anyhow!("runtime_storage_marker_invalid"))?;
                ensure!(
                    !marker.enrollment_id.is_empty(),
                    "runtime_storage_marker_invalid"
                );
                Ok(Some(marker.enrollment_id))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => bail!("runtime_storage_marker_invalid"),
        }
    }
    pub fn database_path(&self) -> &Path {
        &self.database
    }
    pub fn initialized(&self) -> bool {
        self.binding.ready
    }
    /// A control-plane storage grant alone never authorizes replacing existing bytes.
    pub fn prepare(
        directory: &Path,
        database: &Path,
        bootstrap: &CloudRuntimeBootstrap,
        incarnation: &str,
        enrollment: &str,
    ) -> Result<Self> {
        ensure!(
            !enrollment.is_empty() && enrollment.len() <= 128,
            "runtime_storage_enrollment_required"
        );
        let storage = bootstrap
            .runtime_storage
            .clone()
            .ok_or_else(|| anyhow::anyhow!("runtime_storage_enrollment_required"))?;
        ensure!(
            storage.format == RUST_RUNTIME_STORAGE_FORMAT
                && storage.storage_incarnation_id == incarnation,
            "runtime_storage_binding_mismatch"
        );
        let relative = database
            .strip_prefix(directory)
            .map_err(|_| anyhow::anyhow!("runtime_storage_path_invalid"))?;
        ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "runtime_storage_path_invalid"
        );
        let marker = directory.join("rust-runtime-storage.json");
        let expected = Marker {
            version: 1,
            enrollment_id: enrollment.into(),
            workspace_id: bootstrap.workspace_id.clone(),
            account_id: bootstrap.zuse_account_id.clone(),
            storage,
            database: relative
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("runtime_storage_path_invalid"))?
                .into(),
            ready: false,
        };
        let prior = match std::fs::read(&marker) {
            Ok(bytes) => Some(
                serde_json::from_slice::<Marker>(&bytes)
                    .map_err(|_| anyhow::anyhow!("runtime_storage_marker_invalid"))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => bail!("runtime_storage_marker_invalid"),
        };
        let binding = if let Some(prior) = prior {
            ensure!(
                prior.version == 1
                    && prior.enrollment_id == enrollment
                    && prior.workspace_id == expected.workspace_id
                    && prior.account_id == expected.account_id
                    && prior.storage == expected.storage
                    && prior.database == expected.database,
                "runtime_storage_binding_mismatch"
            );
            prior
        } else if let Some(grant) = bootstrap.extra.get("rustRecoveryRestore") {
            #[cfg(feature = "engine")]
            {
                let grant: crate::recovery::RestoreEnrollment =
                    serde_json::from_value(grant.clone())?;
                let receipt = &grant.receipt;
                let restored = receipt
                    .binding
                    .rust_storage
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("runtime_storage_migration_required"))?;
                ensure!(
                    grant.generation == bootstrap.runtime_generation
                        && Some(grant.machine_id.as_str())
                            == bootstrap
                                .extra
                                .get("providerSandboxId")
                                .and_then(|v| v.as_str())
                        && receipt.binding.workspace_id == bootstrap.workspace_id
                        && receipt.binding.chat_id == bootstrap.chat_id
                        && receipt.binding.session_id == bootstrap.initial_session_id
                        && restored.account_id == bootstrap.zuse_account_id
                        && restored.storage_incarnation_id == incarnation
                        && restored.database == relative,
                    "runtime_restore_grant_mismatch"
                );
                crate::recovery::validate_adoption(directory, receipt)?;
                let mut restored = expected;
                restored.ready = true;
                atomic_write(&marker, &serde_json::to_vec(&restored)?)?;
                restored
            }
            #[cfg(not(feature = "engine"))]
            {
                let _ = grant;
                bail!("runtime_storage_migration_required")
            }
        } else {
            // Legacy databases, snapshots and any unrecognized data require a verified
            // import. Keep all of those bytes; do not create a second empty profile.
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                ensure!(
                    !entry.file_type()?.is_symlink(),
                    "runtime_storage_migration_required"
                );
                ensure!(
                    matches!(
                        name.as_ref(),
                        ".workspace-runtime.lock" | "engine.lock" | crate::identity::FILE
                    ) || (name.starts_with(".runtime-write-") && name.ends_with(".next"))
                        || boot::recognized(directory, &name, bootstrap, incarnation)?,
                    "runtime_storage_migration_required"
                );
            }
            atomic_write(&marker, &serde_json::to_vec(&expected)?)?;
            expected
        };
        let store = Self {
            marker,
            binding,
            database: database.to_path_buf(),
        };
        if store.binding.ready {
            store.verify()?;
        }
        Ok(store)
    }
    fn verify(&self) -> Result<()> {
        ensure!(self.database.is_file(), "runtime_storage_missing");
        let connection = rusqlite::Connection::open_with_flags(
            &self.database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let binding:(String,String,String)=connection.query_row("SELECT workspace_id,account_id,storage_incarnation_id FROM zuse_cloud_storage_identity WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|anyhow::anyhow!("runtime_storage_identity_missing"))?;
        ensure!(
            binding
                == (
                    self.binding.workspace_id.clone(),
                    self.binding.account_id.clone(),
                    self.binding.storage.storage_incarnation_id.clone()
                ),
            "runtime_storage_binding_mismatch"
        );
        Ok(())
    }
    /// Call after the existing engine opens its schema and before any command runs.
    pub fn commit(&mut self) -> Result<()> {
        if !self.binding.ready {
            ensure!(self.database.is_file(), "runtime_storage_missing");
            let mut connection = rusqlite::Connection::open_with_flags(
                &self.database,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
            )?;
            // Full integrity checks belong to initial installation and verified
            // archive restore, not ordinary reopen of a large history database.
            let integrity: String = connection.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
            ensure!(integrity == "ok", "runtime_storage_corrupt");
            let tx = connection.transaction()?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS zuse_cloud_storage_identity (singleton INTEGER PRIMARY KEY CHECK(singleton=1),workspace_id TEXT NOT NULL,account_id TEXT NOT NULL,storage_incarnation_id TEXT NOT NULL)")?;
            tx.execute(
                "INSERT OR IGNORE INTO zuse_cloud_storage_identity VALUES (1,?1,?2,?3)",
                rusqlite::params![
                    self.binding.workspace_id,
                    self.binding.account_id,
                    self.binding.storage.storage_incarnation_id
                ],
            )?;
            tx.commit()?;
            self.verify()?;
            self.binding.ready = true;
            atomic_write(&self.marker, &serde_json::to_vec(&self.binding)?)?;
        }
        self.verify()
    }
}
