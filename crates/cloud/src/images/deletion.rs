//! Original snapshot records are the durable cleanup outbox; workspace contents are never deleted here.
use super::worker::{ImageProvider, provider_error, snapshot_guard};
use super::*;
impl ImageControl {
    pub async fn delete_image(
        &self,
        account: &str,
        snapshot: &str,
        at: i64,
    ) -> Result<(), ApiError> {
        if snapshot.is_empty() || snapshot.len() > 512 || snapshot.chars().any(char::is_control) {
            return Err(ApiError::bad_request("invalid_snapshot_id"));
        }
        let mut tx = self.owned(account).await?;
        require_billing_owner(&mut tx, account).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("project-catalog:{account}"))
            .execute(&mut *tx)
            .await?;
        snapshot_guard(&mut tx, account).await?;
        let record:Option<Value>=sqlx::query_scalar("SELECT record FROM api_cloud_snapshots WHERE provider='box' AND account_id=$1 AND snapshot_id=$2 FOR UPDATE").bind(account).bind(snapshot).fetch_optional(&mut *tx).await?;
        let record = record.ok_or_else(|| ApiError::not_found("cloud_snapshot_not_found"))?;
        validate_record(&record, account, snapshot)?;
        if matches!(record["state"].as_str(), Some("deleting" | "deleted")) {
            tx.commit().await?;
            return Ok(());
        }
        let building:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_project_builds WHERE account_id=$1 AND provider IN ('box','boxd') AND state IN ('queued','building','sanitizing'))").bind(account).fetch_one(&mut *tx).await?;
        if building {
            return Err(ApiError::conflict("cloud_image_build_in_progress"));
        }
        if snapshot_in_use(&mut tx, account, snapshot).await? {
            return Err(ApiError::conflict("cloud_snapshot_in_use"));
        }
        supported_provider(&mut tx, account, &record).await?;
        // Retain the unsettled checkpoint and fractional remainder. The billing owner
        // settles through this cutoff even if provider deletion fails or is redelivered.
        sqlx::query("UPDATE api_cloud_snapshots SET state='deleting',record=record||$3 WHERE provider='box' AND account_id=$1 AND snapshot_id=$2").bind(account).bind(snapshot).bind(json!({"state":"deleting","stoppedAtMs":at,"deletionReason":"user","attempts":0,"nextAttemptAtMs":at})).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub(super) async fn delete_once(&self, provider: &dyn ImageProvider) -> Result<bool, ApiError> {
        let allowed = self.accounts.iter().cloned().collect::<Vec<_>>();
        let candidate:Option<(String,String)>=sqlx::query_as("SELECT s.account_id,s.snapshot_id FROM api_cloud_snapshots s JOIN zuse_rust_control_owners o ON o.resource_kind='image-catalog' AND o.resource_id=s.account_id AND o.backend='rust' WHERE EXISTS(SELECT 1 FROM zuse_rust_control_owners p WHERE p.resource_kind='project-catalog' AND p.resource_id=s.account_id AND p.backend='rust') AND s.provider='box' AND s.account_id=ANY($1) AND s.state='deleting' AND COALESCE((s.record->>'nextAttemptAtMs')::bigint,0)<=$2 ORDER BY s.created_at,s.snapshot_id LIMIT 1").bind(allowed).bind(now()).fetch_optional(&self.pool).await?;
        let Some((account, id)) = candidate else {
            return Ok(false);
        };
        let mut tx = self.owned(&account).await?;
        snapshot_guard(&mut tx, &account).await?;
        let record:Option<Value>=sqlx::query_scalar("SELECT record FROM api_cloud_snapshots WHERE provider='box' AND account_id=$1 AND snapshot_id=$2 AND state='deleting' AND COALESCE((record->>'nextAttemptAtMs')::bigint,0)<=$3 FOR UPDATE SKIP LOCKED").bind(&account).bind(&id).bind(now()).fetch_optional(&mut *tx).await?;
        let Some(mut record) = record else {
            return Ok(false);
        };
        validate_record(&record, &account, &id)?;
        if record["state"] != "deleting" {
            return Err(ApiError::conflict("image_snapshot_identity_mismatch"));
        }
        let result = async {
            supported_provider(&mut tx, &account, &record).await?;
            if snapshot_in_use(&mut tx, &account, &id).await? {
                return Err(ApiError::conflict("cloud_snapshot_in_use"));
            }
            if let Some(snapshot) = provider.snapshot(&id).await.map_err(provider_error)? {
                if snapshot.id != id {
                    return Err(ApiError::conflict("image_snapshot_identity_mismatch"));
                }
                provider
                    .delete_snapshot(&id)
                    .await
                    .map_err(provider_error)?;
            }
            Ok(())
        }
        .await;
        record["attempts"] = json!(record["attempts"].as_i64().unwrap_or(0).saturating_add(1));
        match result {
            Ok(()) => {
                record["state"] = json!("deleted");
                record["deletedAtMs"] = json!(now());
                record
                    .as_object_mut()
                    .ok_or_else(unavailable)?
                    .remove("lastError");
            }
            Err(error) => {
                record["lastError"] = json!(error.code);
                record["nextAttemptAtMs"] = json!(now() + 2000);
            }
        }
        sqlx::query("UPDATE api_cloud_snapshots SET state=$3,record=$4 WHERE provider='box' AND account_id=$1 AND snapshot_id=$2").bind(&account).bind(&id).bind(text(&record,"state")?).bind(&record).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(true)
    }
}
fn validate_record(record: &Value, account: &str, id: &str) -> Result<(), ApiError> {
    if record["accountId"] != account
        || record["snapshotId"] != id
        || record["provider"] != "box"
        || !record["buildId"].is_string()
    {
        return Err(ApiError::conflict("image_snapshot_identity_mismatch"));
    }
    Ok(())
}
/// Accepted allocation/restore work pins the source until its target machine is known.
/// An uncertain create is not evidence that the source is no longer needed.
pub(super) async fn snapshot_in_use(
    tx: &mut Transaction<'_, Postgres>,
    account: &str,
    snapshot: &str,
) -> Result<bool, ApiError> {
    for (table, join) in [
        ("zuse_rust_allocations", "a.workspace_id=w.workspace_id"),
        ("zuse_rust_restores", "a.workspace_id=w.workspace_id"),
    ] {
        let present: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(table)
            .fetch_one(&mut **tx)
            .await?;
        if present {
            let used:bool=sqlx::query_scalar(&format!("SELECT EXISTS(SELECT 1 FROM {table} a JOIN api_cloud_workspaces w ON {join} WHERE w.account_id=$1 AND a.snapshot_id=$2 AND a.machine_id IS NULL AND a.phase<>'ready' AND w.state<>'deleted')")).bind(account).bind(snapshot).fetch_one(&mut **tx).await?;
            if used {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
/// Read-side fence for admission. The caller holds it through its durable allocation intent.
pub(crate) async fn snapshot_read_guard(
    tx: &mut Transaction<'_, Postgres>,
    account: &str,
    snapshot: Option<&str>,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended($1,0))")
        .bind(format!("snapshot:{account}"))
        .execute(&mut **tx)
        .await?;
    let present: bool = sqlx::query_scalar("SELECT to_regclass('api_cloud_snapshots') IS NOT NULL")
        .fetch_one(&mut **tx)
        .await?;
    if let Some(snapshot) = snapshot.filter(|_| present) {
        let unavailable:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_snapshots WHERE account_id=$1 AND provider IN ('box','boxd') AND snapshot_id=$2 AND state IN ('deleting','deleted'))").bind(account).bind(snapshot).fetch_one(&mut **tx).await?;
        if unavailable {
            return Err(ApiError::conflict("cloud_snapshot_unavailable"));
        }
    }
    Ok(())
}

// Connected-provider images require their original account credential adapter.
// A platform-key NotFound cannot prove that a customer's snapshot was removed.
async fn supported_provider(
    tx: &mut Transaction<'_, Postgres>,
    account: &str,
    record: &Value,
) -> Result<(), ApiError> {
    let settings:Option<Value>=sqlx::query_scalar("SELECT settings FROM api_cloud_project_builds WHERE account_id=$1 AND build_id=$2 FOR SHARE").bind(account).bind(text(record,"buildId")?).fetch_optional(&mut **tx).await?;
    if settings.is_some_and(|s| s["providerConnectionId"].is_string()) {
        return Err(ApiError::conflict(
            "cloud_image_provider_connection_required",
        ));
    }
    Ok(())
}
