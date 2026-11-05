//! A remote PUT may outlive its caller; its cleanup authority must outlive FK rows.
use super::*;
#[derive(Clone)]
pub(super) struct ObjectWriteIntent {
    id: String,
    key: String,
    owner: String,
    workspace: String,
    generation: i64,
    source: Value,
    disk: Value,
    fence: i64,
}
impl ObjectWriteIntent {
    pub(super) async fn prepare(
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        key: &str,
        purpose: &str,
        now: i64,
    ) -> Result<Self, ApiError> {
        if key.len() > 1024 || !matches!(purpose, "tool" | "asset-chunk" | "asset-manifest" | "transcript") {
            return Err(ApiError::bad_request("invalid_object_write_intent"));
        }
        let workspace = required(row, "workspace_id")?.to_owned();
        let prefix = format!("workspaces/{}/", crate::transcript::component(&workspace));
        if !key.starts_with(&prefix) {
            return Err(ApiError::conflict("object_write_scope_mismatch"));
        }
        let intent = Self {
            id: uuid::Uuid::new_v4().to_string(),
            key: key.into(),
            owner: required(row, "account_id")?.into(),
            workspace,
            generation: row["request_config"]["runtimeGeneration"]
                .as_i64()
                .unwrap_or(1),
            source: row["provider_sandbox_id"].clone(),
            disk: row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"].clone(),
            fence: super::mailbox::destruction_fence(row),
        };
        // Bound malicious/cancelled retries without holding this lock over I/O.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("cloud-object-intents:{}", intent.workspace))
            .execute(&mut **tx)
            .await?;
        let pending: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM zuse_rust_object_write_intents WHERE workspace_id=$1",
        )
        .bind(&intent.workspace)
        .fetch_one(&mut **tx)
        .await?;
        if pending >= 128 {
            return Err(ApiError::conflict("object_write_recovery_required"));
        }
        sqlx::query("INSERT INTO zuse_rust_object_write_intents(intent_id,workspace_id,account_id,object_key,purpose,generation,source_machine_id,storage_incarnation_id,destruction_fence,created_at,next_reconcile_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)").bind(&intent.id).bind(&intent.workspace).bind(&intent.owner).bind(&intent.key).bind(purpose).bind(intent.generation).bind(intent.source.as_str()).bind(intent.disk.as_str()).bind(intent.fence).bind(now).bind(now+60000).execute(&mut **tx).await?;
        Ok(intent)
    }
    pub(super) fn authorize_result(&self, row: &Value) -> Result<(), ApiError> {
        if row["workspace_id"] != self.workspace
            || row["account_id"] != self.owner
            || matches!(row["state"].as_str(), Some("deleting" | "deleted"))
            || row["desired_state"] == "deleted"
            || row["request_config"]["runtimeGeneration"]
                .as_i64()
                .unwrap_or(1)
                != self.generation
            || row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"] != self.disk
            || (!self.source.is_null() && row["provider_sandbox_id"] != self.source)
            || super::mailbox::destruction_fence(row) != self.fence
        {
            return Err(ApiError::conflict("object_write_runtime_fenced"));
        }
        Ok(())
    }
    /// Must share the transaction publishing the ciphertext checkpoint ready.
    /// Only this acknowledged physical attempt is removed; retries each have
    /// their own receipt because another identical PUT may still be in flight.
    pub(super) async fn commit(
        self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
    ) -> Result<(), ApiError> {
        self.authorize_result(row)?;
        let deleted=sqlx::query("DELETE FROM zuse_rust_object_write_intents WHERE intent_id=$1 AND workspace_id=$2 AND account_id=$3 AND object_key=$4 AND generation=$5 AND destruction_fence=$6").bind(&self.id).bind(&self.workspace).bind(&self.owner).bind(&self.key).bind(self.generation).bind(self.fence).execute(&mut **tx).await?;
        if deleted.rows_affected() != 1 {
            return Err(ApiError::conflict("object_write_intent_fenced"));
        }
        Ok(())
    }
}
impl RuntimeControl {
    /// One bounded cleanup slice. Unacknowledged remote writes retain their
    /// receipt even after an empty purge; a later PUT completion is reswept.
    pub async fn reconcile_object_write_once(&self, now: i64) -> Result<bool, ApiError> {
        let mut tx = self.pool.begin().await?;
        let ready: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_object_write_intents') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await?;
        if !ready {
            return Ok(false);
        }
        let next:Option<(String,String,String)>=sqlx::query_as("SELECT intent_id,workspace_id,object_key FROM zuse_rust_object_write_intents WHERE next_reconcile_at<=$1 ORDER BY next_reconcile_at,intent_id LIMIT 1 FOR UPDATE SKIP LOCKED").bind(now).fetch_optional(&mut *tx).await?;
        let Some((intent, workspace, key)) = next else {
            return Ok(false);
        };
        sqlx::query(
            "UPDATE zuse_rust_object_write_intents SET next_reconcile_at=$2+LEAST(3600000::bigint,60000::bigint*(1::bigint<<LEAST(cleanup_attempts,6)::int)),cleanup_attempts=cleanup_attempts+1 WHERE intent_id=$1",
        )
        .bind(&intent)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let deleted:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE workspace_id=$1 AND state NOT IN ('deleting','deleted') AND desired_state<>'deleted')").bind(&workspace).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        if !deleted {
            return Ok(true);
        }
        let prefix = format!("workspaces/{}/", crate::transcript::component(&workspace));
        if !key.starts_with(&prefix) {
            return Err(ApiError::conflict("object_write_scope_mismatch"));
        }
        let objects = self
            .transcript_objects
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_object_store_unavailable"))?;
        let empty = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            objects.purge_prefix(&key),
        )
        .await
        .map_err(|_| ApiError::unavailable("object_write_cleanup_timeout"))??;
        if empty {
            sqlx::query(
                "UPDATE zuse_rust_object_write_intents SET last_empty_at=$2 WHERE intent_id=$1",
            )
            .bind(intent)
            .bind(now)
            .execute(&self.pool)
            .await?;
        }
        Ok(true)
    }
}
