//! One local/shared-volume guard across provider effects, verification and permanent cleanup.
use super::*;
use std::path::PathBuf;
pub(super) struct Guard {
    _lock: zuse_cloud_runtime::DataLock,
    data: PathBuf,
}
impl Guard {
    pub(super) fn acquire(workspace: &str) -> Result<Self, ApiError> {
        let root = std::env::temp_dir().join("zuse-cloud-effects");
        let name = hash(workspace);
        let locks = root.join("locks").join(&name);
        for path in [&root, &root.join("locks"), &locks] {
            std::fs::create_dir_all(path).map_err(|_| failed())?;
            if std::fs::symlink_metadata(path)
                .map_err(|_| failed())?
                .file_type()
                .is_symlink()
            {
                return Err(failed());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| failed())?;
            }
        }
        let lock = zuse_cloud_runtime::DataLock::acquire(&locks, None)
            .map_err(|_| ApiError::conflict("cloud_workspace_effects_busy"))?;
        Ok(Self {
            _lock: lock,
            data: root.join("verification").join(name),
        })
    }
    pub(super) fn temp(&self) -> anyhow::Result<tempfile::TempDir> {
        std::fs::create_dir_all(&self.data)?;
        anyhow::ensure!(
            !std::fs::symlink_metadata(&self.data)?
                .file_type()
                .is_symlink(),
            "verification_scratch_invalid"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.data, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(tempfile::tempdir_in(&self.data)?)
    }
    pub(super) fn purge(&self) -> Result<(), ApiError> {
        match std::fs::symlink_metadata(&self.data) {
            Ok(meta) if meta.file_type().is_symlink() => Err(failed()),
            Ok(_) => std::fs::remove_dir_all(&self.data).map_err(|_| failed()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(failed()),
        }
    }
}
fn failed() -> ApiError {
    ApiError::unavailable("cloud_workspace_effects_storage_failed")
}
/// Independent journal: it survives a caller's transaction rollback or lost create response.
pub(super) async fn creating(
    pool: &PgPool,
    workspace: &str,
    operation: &str,
    name: &str,
) -> Result<(), ApiError> {
    let ready: bool =
        sqlx::query_scalar("SELECT to_regclass('zuse_rust_provider_creates') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if ready {
        sqlx::query("INSERT INTO zuse_rust_provider_creates(operation_id,workspace_id,machine_name,outcome,updated_at) VALUES($1,$2,$3,'pending',$4) ON CONFLICT(operation_id) DO UPDATE SET outcome='pending',updated_at=EXCLUDED.updated_at").bind(operation).bind(workspace).bind(name).bind(chrono::Utc::now().timestamp_millis()).execute(pool).await?;
    }
    Ok(())
}
pub(super) async fn created(
    pool: &PgPool,
    operation: &str,
    result: &Result<crate::providers::Machine, crate::providers::ProviderError>,
) -> Result<(), ApiError> {
    let ready: bool =
        sqlx::query_scalar("SELECT to_regclass('zuse_rust_provider_creates') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if ready {
        let (state, id) = match result {
            Ok(m) => ("confirmed", Some(m.id.as_str())),
            Err(crate::providers::ProviderError::Rejected) => ("rejected", None),
            Err(_) => ("unknown", None),
        };
        sqlx::query("UPDATE zuse_rust_provider_creates SET outcome=$2,machine_id=COALESCE($3,machine_id),updated_at=$4 WHERE operation_id=$1").bind(operation).bind(state).bind(id).bind(chrono::Utc::now().timestamp_millis()).execute(pool).await?;
    }
    Ok(())
}
