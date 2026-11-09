//! Fresh API-owned eligibility before retained/recovery compute is started.
//! Cleanup (pause/archive/delete) is intentionally independent of the departing
//! task creator so an active organization administrator can still stop costs.
use super::*;
impl RuntimeControl {
    pub(super) async fn worker_job_scope_access(
        &self,
        job: &crate::jobs::Job,
        kind: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(&job.resource_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        self.recreation_effect_scope(&row).await?;
        let organization = row["account_id"]
            .as_str()
            .is_some_and(|o| o.starts_with("organization:"));
        if !organization && row["request_config"]["managedCandidateSha256"].is_null() {
            return Ok(());
        }
        let operation = job.payload["operationId"]
            .as_str()
            .ok_or_else(|| ApiError::conflict("worker_scope_operation_invalid"))?;
        if kind == "lifecycle" {
            let action:Option<String>=sqlx::query_scalar("SELECT action FROM zuse_rust_lifecycle_operations WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3").bind(operation).bind(&job.resource_id).bind(job.generation).fetch_optional(&self.pool).await?;
            match action.as_deref() {
                Some("resume") => {}
                Some("pause" | "archive" | "unarchive") => return Ok(()),
                _ => return Err(ApiError::conflict("worker_scope_operation_invalid")),
            }
        } else if kind == "repair" {
            let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_runtime_repairs WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 AND source_machine_id=$4 AND storage_incarnation_id=$5 AND phase NOT IN ('ready','attention-required'))").bind(operation).bind(&job.resource_id).bind(job.generation).bind(row["provider_sandbox_id"].as_str()).bind(row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"].as_str()).fetch_one(&self.pool).await?;
            if !valid || row["request_config"]["rustRuntimeRepair"]["operationId"] != operation {
                return Err(ApiError::conflict("worker_scope_operation_invalid"));
            }
        } else if kind != "restore" {
            return Err(ApiError::conflict("worker_scope_operation_invalid"));
        }
        self.candidate_row_allowed(&row, now).await?;
        if !organization {
            return Ok(());
        }
        let proof = self.signer.allocation_scope_proof(
            &job.resource_id,
            &format!("{kind}:{operation}"),
            job.generation,
            now,
        )?;
        if self.scope_authority_configured() {
            return self
                .authorize_worker_effect_scope(&job.resource_id, &proof, now)
                .await;
        }
        let mut url = reqwest::Url::parse(&self.signer.issuer())
            .map_err(|_| ApiError::unavailable("worker_scope_unavailable"))?;
        url.path_segments_mut()
            .map_err(|_| ApiError::unavailable("worker_scope_unavailable"))?
            .clear()
            .extend([
                "v1",
                "cloud",
                "workspaces",
                &job.resource_id,
                "worker-effect-scope",
            ]);
        let response = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ApiError::unavailable("worker_scope_unavailable"))?
            .post(url)
            .bearer_auth(proof)
            .send()
            .await
            .map_err(|_| ApiError::unavailable("worker_scope_unavailable"))?;
        if response.status() != reqwest::StatusCode::NO_CONTENT {
            return Err(ApiError::forbidden("worker_scope_denied"));
        }
        Ok(())
    }
    pub async fn authorize_worker_effect_scope(
        &self,
        id: &str,
        proof: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let claims = self.signer.verify_allocation_scope(proof, id, now)?;
        let operation = claims["operationId"]
            .as_str()
            .ok_or_else(|| ApiError::forbidden("worker_scope_denied"))?;
        let (kind, operation) = operation
            .split_once(':')
            .ok_or_else(|| ApiError::forbidden("worker_scope_denied"))?;
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        if blocked(&row)
            || (row["desired_state"] != "ready"
                && !(kind == "repair" && row["desired_state"] == "paused"))
            || row["request_config"]["runtimeGeneration"] != claims["generation"]
            || !super::scope::native_organization_task(&row)
        {
            return Err(ApiError::forbidden("worker_scope_denied"));
        }
        let exists = match kind {
            "lifecycle" => {
                if row["request_config"]["rustLifecycleOperation"]["id"] != operation {
                    return Err(ApiError::forbidden("worker_scope_denied"));
                }
                sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM zuse_rust_lifecycle_operations WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 AND action='resume' AND phase IN ('waking','launching','ready'))").bind(operation).bind(id).bind(claims["generation"].as_i64()).fetch_one(&self.pool).await?
            }
            "restore" => {
                if row["request_config"]["rustRestoreOperationId"] != operation {
                    return Err(ApiError::forbidden("worker_scope_denied"));
                }
                sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM zuse_rust_restores WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3)").bind(operation).bind(id).bind(claims["generation"].as_i64()).fetch_one(&self.pool).await?
            }
            "repair" => {
                if row["request_config"]["rustRuntimeRepair"]["operationId"] != operation {
                    return Err(ApiError::forbidden("worker_scope_denied"));
                }
                sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM zuse_rust_runtime_repairs WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 AND source_machine_id=$4 AND storage_incarnation_id=$5 AND phase NOT IN ('ready','attention-required'))").bind(operation).bind(id).bind(claims["generation"].as_i64()).bind(row["provider_sandbox_id"].as_str()).bind(row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"].as_str()).fetch_one(&self.pool).await?
            }
            _ => false,
        };
        if !exists {
            return Err(ApiError::forbidden("worker_scope_denied"));
        }
        self.scope_access(id, None, true).await?;
        if kind == "repair" {
            let actor:String=sqlx::query_scalar("SELECT actor_id FROM zuse_rust_runtime_repairs WHERE operation_id=$1 AND workspace_id=$2").bind(operation).bind(id).fetch_one(&self.pool).await?;
            self.scope_access(id, Some(&actor), true).await?;
        }
        Ok(())
    }
}
