//! Cache retirement before the first provider attempt can fall back to the
//! original managed base. An uncertain original allocation never changes source.
use super::*;
impl crate::runtime::RuntimeControl {
    pub(crate) async fn project_cache_allocation_source(
        &self,
        workspace: &mut Value,
        operation: &mut Value,
    ) -> Result<bool, ApiError> {
        if workspace["request_config"]["rustProjectCache"].is_null()
            || operation["phase"] != "planned"
            || operation["machine_id"].is_string()
        {
            return Ok(false);
        }
        let id = workspace["workspace_id"]
            .as_str()
            .ok_or_else(|| ApiError::conflict("project_cache_allocation_fenced"))?;
        let op = operation["operation_id"]
            .as_str()
            .ok_or_else(|| ApiError::conflict("project_cache_allocation_fenced"))?;
        let pool = self.shared_pool();
        let mut tx = pool.begin().await?;
        let current: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 FOR UPDATE",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        let current_op:Value=sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE").bind(op).bind(id).fetch_one(&mut *tx).await?;
        if current_op["phase"] != "planned"
            || current_op["machine_id"].is_string()
            || current_op["generation"] != operation["generation"]
            || current["request_config"]["runtimeGeneration"] != current_op["generation"]
        {
            return Err(ApiError::conflict("project_cache_allocation_fenced"));
        }
        let cache = &current["request_config"]["rustProjectCache"];
        let identity: CacheIdentity = serde_json::from_value(cache["identity"].clone())
            .map_err(|_| ApiError::conflict("project_cache_identity_invalid"))?;
        if identity.owner_id != current["account_id"].as_str().unwrap_or_default()
            || identity.project_id != current["project_id"].as_str().unwrap_or_default()
            || identity.provider.as_str() != current["provider"].as_str().unwrap_or_default()
            || cache["artifactId"] != current_op["snapshot_id"]
            || identity.key()? != cache["cacheKey"].as_str().unwrap_or_default()
        {
            return Err(ApiError::conflict("project_cache_allocation_fenced"));
        }
        let hit = lookup_tx(&mut tx, &identity).await?;
        if let Some(hit) = hit {
            if hit.artifact_id != current_op["snapshot_id"]
                || json!(hit.receipt) != cache["receipt"]
            {
                return Err(ApiError::conflict("project_cache_allocation_fenced"));
            }
            tx.commit().await?;
            *workspace = current;
            *operation = current_op;
            return Ok(false);
        }
        // No remote attempt is possible while phase remains planned: the
        // allocator durably writes creating before its first provider call.
        if identity.staging_pilot_sha256.is_some() {
            if !super::pilot::authorized_tx(
                &mut tx,
                &identity,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?
            {
                return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
            }
        } else {
            let base:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(b) FROM zuse_rust_managed_base_admission_lock($1,$2) b WHERE state='verified'").bind(identity.provider.as_str()).bind(&identity.base_release_id).fetch_optional(&mut *tx).await?;
            if base
                .as_ref()
                .is_none_or(|base| base["artifact_id"] != identity.base_artifact_id)
            {
                return Err(ApiError::conflict("project_cache_managed_base_retired"));
            }
        }
        let mut config = current["request_config"].clone();
        let object = config
            .as_object_mut()
            .ok_or_else(|| ApiError::conflict("project_cache_allocation_fenced"))?;
        object.remove("rustProjectCache");
        object.remove("providerArtifactBinding");
        if !cache["baseArtifactBinding"].is_null() {
            let binding: crate::providers::artifact::ArtifactBinding =
                serde_json::from_value(cache["baseArtifactBinding"].clone())
                    .map_err(|_| ApiError::conflict("project_cache_artifact_binding_invalid"))?;
            binding
                .validate(identity.provider, &identity.base_artifact_id)
                .map_err(|_| ApiError::conflict("project_cache_artifact_binding_invalid"))?;
            config["providerArtifactBinding"] = json!(binding);
        }
        config["repositoryCache"] = json!("none");
        if let Some(checkout) = config["rustRepositoryCheckout"].as_object_mut() {
            checkout.remove("cachePath");
        }
        config["rustRepositoryCheckout"]["fetchRemote"] = json!(true);
        config["projectCacheFallback"] = json!("artifact-no-longer-compatible");
        let updated:Value=sqlx::query_scalar("UPDATE api_cloud_workspaces w SET request_config=$2,revision=revision+1 WHERE workspace_id=$1 RETURNING to_jsonb(w)").bind(id).bind(config).fetch_one(&mut *tx).await?;
        let updated_op:Value=sqlx::query_scalar("UPDATE zuse_rust_allocations a SET snapshot_id=$2 WHERE operation_id=$1 AND phase='planned' RETURNING to_jsonb(a)").bind(op).bind(&identity.base_artifact_id).fetch_one(&mut *tx).await?;
        tx.commit().await?;
        *workspace = updated;
        *operation = updated_op;
        Ok(true)
    }
}
