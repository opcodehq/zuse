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
pub(super) async fn created_identity(
    pool: &PgPool,
    workspace: &str,
    operation: &str,
    name: &str,
) -> Result<Option<String>, ApiError> {
    let ready: bool =
        sqlx::query_scalar("SELECT to_regclass('zuse_rust_provider_creates') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if !ready {
        return Ok(None);
    }
    read_created_identity(pool, workspace, operation, name).await
}
async fn read_created_identity(
    pool: &PgPool,
    workspace: &str,
    operation: &str,
    name: &str,
) -> Result<Option<String>, ApiError> {
    let row:Option<(String,String,Option<String>)>=sqlx::query_as("SELECT workspace_id,machine_name,machine_id FROM zuse_rust_provider_creates WHERE operation_id=$1").bind(operation).fetch_optional(pool).await?;
    match row {
        Some((w, n, id)) if w == workspace && n == name => Ok(id),
        Some(_) => Err(ApiError::conflict(
            "cloud_provider_allocation_identity_conflict",
        )),
        None => Ok(None),
    }
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
        let saved=sqlx::query("INSERT INTO zuse_rust_provider_creates(operation_id,workspace_id,machine_name,outcome,updated_at) VALUES($1,$2,$3,'pending',$4) ON CONFLICT(operation_id) DO UPDATE SET outcome='pending',updated_at=EXCLUDED.updated_at WHERE zuse_rust_provider_creates.workspace_id=EXCLUDED.workspace_id AND zuse_rust_provider_creates.machine_name=EXCLUDED.machine_name AND zuse_rust_provider_creates.outcome='rejected' AND zuse_rust_provider_creates.machine_id IS NULL").bind(operation).bind(workspace).bind(name).bind(chrono::Utc::now().timestamp_millis()).execute(pool).await?;
        if saved.rows_affected() != 1 {
            return Err(ApiError::unavailable(
                "cloud_provider_allocation_outcome_unknown",
            ));
        }
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
        write_created_result(pool, operation, result).await?;
    }
    Ok(())
}
async fn write_created_result(
    pool: &PgPool,
    operation: &str,
    result: &Result<crate::providers::Machine, crate::providers::ProviderError>,
) -> Result<(), ApiError> {
        let (state, id) = match result {
            Ok(m) => ("confirmed", Some(m.id.as_str())),
            Err(crate::providers::ProviderError::Rejected) => ("rejected", None),
            Err(_) => ("unknown", None),
        };
        sqlx::query("UPDATE zuse_rust_provider_creates SET outcome=$2,machine_id=COALESCE($3,machine_id),updated_at=$4 WHERE operation_id=$1").bind(operation).bind(state).bind(id).bind(chrono::Utc::now().timestamp_millis()).execute(pool).await?;
    Ok(())
}

impl RuntimeControl {
    pub(super) async fn allocation_created_identity(
        &self, workspace: &str, operation: &str, name: &str,
    ) -> Result<Option<String>, ApiError> {
        if self.startup_schema.get().is_some_and(|schema| schema.2) {
            read_created_identity(&self.pool,workspace,operation,name).await
        } else {
            created_identity(&self.pool,workspace,operation,name).await
        }
    }
    pub(super) async fn record_allocation_result(
        &self, operation: &str,
        result: &Result<crate::providers::Machine, crate::providers::ProviderError>,
    ) -> Result<(), ApiError> {
        // Cache table availability only. Read and persist the original receipt
        // on every call; no ownership, outcome or authorization is cached.
        if self.startup_schema.get().is_some_and(|schema| schema.2) {
            write_created_result(&self.pool,operation,result).await
        } else {
            created(&self.pool,operation,result).await
        }
    }
}
