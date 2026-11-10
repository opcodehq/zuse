//! Original-resource retirement and observed occupancy survive project removal.
use super::*;
impl worker::CacheControl {
    pub(super) async fn retire_one(&self, now: i64) -> Result<bool, ApiError> {
        let candidates:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(e) FROM zuse_rust_project_cache_entries e WHERE e.state='ready' AND NOT EXISTS(SELECT 1 FROM api_cloud_workspaces w WHERE w.state<>'deleted' AND w.request_config->'rustProjectCache'->>'cacheKey'=e.cache_key) AND (e.revoked_at IS NOT NULL OR NOT EXISTS(SELECT 1 FROM api_cloud_projects p WHERE p.account_id=e.owner_id AND p.project_id=e.project_id AND p.included=true) OR NOT EXISTS(SELECT 1 FROM cloud_managed_bases b JOIN cloud_project_cache_capabilities c USING(provider,release_id) WHERE b.provider=e.provider AND b.release_id=e.base_release_id AND b.state='verified' AND c.state='certified' AND b.artifact_id=e.base_artifact_id)) ORDER BY e.verified_at,e.cache_key LIMIT 32").fetch_all(&self.pool).await?;
        let mut candidate = None;
        for entry in candidates {
            let identity = serde_json::from_value::<CacheIdentity>(entry["identity"].clone());
            if let Ok(identity) = identity {
                if identity.staging_pilot_sha256.is_some() && entry["revoked_at"].is_null() {
                    let mut tx = self.pool.begin().await?;
                    let active = super::pilot::authorized_tx(&mut tx, &identity, now)
                        .await
                        .unwrap_or(false);
                    tx.commit().await?;
                    if active {
                        continue;
                    }
                }
            }
            candidate = Some(entry);
            break;
        }
        let Some(candidate) = candidate else {
            return Ok(false);
        };
        let mut tx = self.pool.begin().await?;
        let _: Vec<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(b) FROM zuse_rust_managed_base_admission_lock($1,$2) b",
        )
        .bind(candidate["provider"].as_str())
        .bind(candidate["base_release_id"].as_str())
        .fetch_all(&mut *tx)
        .await?;
        let _: Vec<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(c) FROM zuse_rust_project_cache_capability_lock($1,$2) c",
        )
        .bind(candidate["provider"].as_str())
        .bind(candidate["base_release_id"].as_str())
        .fetch_all(&mut *tx)
        .await?;
        let operation:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_project_cache_operations o WHERE operation_id=$1 FOR UPDATE").bind(candidate["preparation_operation_id"].as_str()).fetch_optional(&mut *tx).await?;
        let Some(operation) = operation else {
            return Err(ApiError::conflict(
                "project_cache_retirement_receipt_missing",
            ));
        };
        let current:Value=sqlx::query_scalar("SELECT to_jsonb(e) FROM zuse_rust_project_cache_entries e WHERE cache_key=$1 FOR UPDATE").bind(candidate["cache_key"].as_str()).fetch_one(&mut *tx).await?;
        if current["state"] != "ready"
            || operation["state"] != "ready"
            || operation["cache_key"] != current["cache_key"]
            || operation["resource_receipts"]["provider"]["artifactId"] != current["artifact_id"]
        {
            return Ok(false);
        }
        // After the entry lock waits for earlier admissions, this separate
        // statement sees their committed task references. Never retire under
        // an original allocation whose outcome may still be uncertain.
        let referenced:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE state<>'deleted' AND request_config->'rustProjectCache'->>'cacheKey'=$1)").bind(current["cache_key"].as_str()).fetch_one(&mut *tx).await?;
        let revoked:bool=sqlx::query_scalar("SELECT $4 OR NOT EXISTS(SELECT 1 FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true) OR NOT EXISTS(SELECT 1 FROM cloud_managed_bases b JOIN cloud_project_cache_capabilities c USING(provider,release_id) WHERE b.provider=$3 AND b.release_id=$5 AND b.state='verified' AND c.state='certified' AND b.artifact_id=$6)").bind(current["owner_id"].as_str()).bind(current["project_id"].as_str()).bind(current["provider"].as_str()).bind(!current["revoked_at"].is_null()).bind(current["base_release_id"].as_str()).bind(current["base_artifact_id"].as_str()).fetch_one(&mut *tx).await?;
        let pilot_active = if current["revoked_at"].is_null() {
            if let Ok(identity) =
                serde_json::from_value::<CacheIdentity>(current["identity"].clone())
            {
                super::pilot::authorized_tx(&mut tx, &identity, now)
                    .await
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        };
        if referenced || !revoked || pilot_active {
            return Ok(false);
        }
        // Preserve one active cleanup/preparation chain per owner. Uncertainty
        // continues holding that slot; no replacement resource is invented.
        let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_project_cache_operations WHERE owner_id=$1 AND operation_id<>$2 AND state IN('running','uncertain','cleaning'))").bind(current["owner_id"].as_str()).bind(operation["operation_id"].as_str()).fetch_one(&mut *tx).await?;
        if busy {
            return Ok(false);
        }
        sqlx::query("UPDATE zuse_rust_project_cache_entries SET state='retired',revoked_at=coalesce(revoked_at,$2) WHERE cache_key=$1").bind(current["cache_key"].as_str()).bind(now).execute(&mut *tx).await?;
        let result=sqlx::query("UPDATE zuse_rust_project_cache_operations SET state='cleaning',phase='cleanup',lease_id=NULL,lease_expires_at=NULL,next_attempt_at=$2,last_error_code='project_cache_retired',updated_at=$2 WHERE operation_id=$1").bind(operation["operation_id"].as_str()).bind(now).execute(&mut *tx).await;
        if let Err(error) = result {
            if error.as_database_error().and_then(|e| e.constraint())
                == Some("zuse_rust_project_cache_one_active_owner")
            {
                return Ok(false);
            }
            return Err(error.into());
        }
        tx.commit().await?;
        Ok(true)
    }
}
pub(super) async fn account_tx(
    tx: &mut Transaction<'_, Postgres>,
    row: &Value,
    resources: &Value,
    now: i64,
) -> Result<(), ApiError> {
    for (kind, key, absence) in [
        ("builder", "builderId", "builderAbsent"),
        ("verifier", "verifierId", "verifierAbsent"),
        ("artifact", "artifactId", "unpublishedArtifactAbsent"),
    ] {
        let Some(id) = resources[key].as_str() else {
            continue;
        };
        let original:Option<String>=sqlx::query_scalar("SELECT resource_id FROM zuse_rust_project_cache_resources WHERE operation_id=$1 AND resource_kind=$2").bind(row["operation_id"].as_str()).bind(kind).fetch_optional(&mut **tx).await?;
        if original.as_deref().is_some_and(|original| original != id) {
            return Err(ApiError::conflict(
                "project_cache_resource_identity_changed",
            ));
        }
        // A worker's supplied clock may precede an earlier observation. Keep
        // the first absence receipt ordered after discovery, and never infer
        // absence from an update which contains no absence observation.
        sqlx::query("INSERT INTO zuse_rust_project_cache_resources(operation_id,owner_id,project_id,provider,resource_kind,resource_id,first_observed_at,last_observed_at,absent_observed_at) VALUES($1,$2,$3,$4,$5,$6,$7,$7,$8) ON CONFLICT(operation_id,resource_kind) DO UPDATE SET last_observed_at=greatest(zuse_rust_project_cache_resources.last_observed_at,EXCLUDED.last_observed_at),absent_observed_at=coalesce(zuse_rust_project_cache_resources.absent_observed_at,CASE WHEN EXCLUDED.absent_observed_at IS NOT NULL THEN greatest(zuse_rust_project_cache_resources.first_observed_at,EXCLUDED.absent_observed_at) END)").bind(row["operation_id"].as_str()).bind(row["owner_id"].as_str()).bind(row["project_id"].as_str()).bind(row["provider"].as_str()).bind(kind).bind(id).bind(now).bind((resources[absence]==true).then_some(now)).execute(&mut **tx).await?;
    }
    Ok(())
}
