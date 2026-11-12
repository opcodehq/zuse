//! A committed diagnostic gateway owns preparation, including failed stages.
use super::*;

pub(super) const ACTIVE: &str = "cloud_allocation_preparation_active";
/// The job error when a hold ends because the guest stopped preparing.
pub(super) const ABANDONED: &str = "cloud_allocation_preparation_abandoned";
/// How long an offline guest may leave preparation unchanged before the hold
/// ends. An online guest is still working and is held however long it takes.
pub(super) const ABANDON_AFTER_MS: i64 = 30 * 60 * 1000;

pub(super) fn active(workspace: &Value, operation: &Value) -> bool {
    let config = &workspace["request_config"];
    let ingress = &config["rustPreparationIngress"];
    workspace["state"] == "setup"
        && workspace["desired_state"] == "ready"
        && (workspace["runtime_state"] != "online"
            || (config["rustFastStartVersion"] == 1 && config["rustAgentPreparationReady"] == true))
        && config["runtimeRpcProtocol"] == zuse_protocol::cloud_gateway::RUST_RPC
        && config["rustPreparationVersion"] == 1
        && storage_enrolled(workspace)
        && operation["phase"] == "launching"
        && operation["workspace_id"] == workspace["workspace_id"]
        && operation["operation_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
        && operation["operation_id"] == config["rustAllocationOperationId"]
        && operation["generation"].as_i64().is_some_and(|g| g > 0)
        && operation["generation"] == config["runtimeGeneration"]
        && operation["machine_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
        && operation["machine_id"] == workspace["provider_sandbox_id"]
        && operation["storage_incarnation_id"]
            == config["rustRuntimeStorage"]["storageIncarnationId"]
        && ingress["version"] == 1
        && ingress["operationId"] == operation["operation_id"]
        && ingress["generation"] == operation["generation"]
        && ingress["sourceMachineId"] == operation["machine_id"]
        && ingress["storageIncarnationId"] == operation["storage_incarnation_id"]
}

impl RuntimeControl {
    pub(super) async fn hold_preparing_allocation(
        &self,
        job: &jobs::Job,
    ) -> Result<bool, ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        if generation != job.generation || blocked(&row) {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        let operation: Value = sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3")
            .bind(job.payload["operationId"].as_str()).bind(&job.resource_id).bind(job.generation).fetch_one(&mut *tx).await?;
        if !active(&row, &operation) {
            tx.commit().await?;
            return Ok(false);
        }
        let now = clock_ms();
        // A guest that went offline mid-preparation (often after a failed
        // stage) never reports again; without a bound this rechecks forever.
        // End the job and keep the workspace's failure for the user to act on.
        let idle = now - row["updated_at"].as_i64().unwrap_or(now);
        if row["runtime_state"] != "online" && idle > ABANDON_AFTER_MS {
            let changed = sqlx::query("UPDATE zuse_rust_jobs SET state='failed',last_error_code=$3,lease_token=NULL,lease_expires_at=NULL,updated_at=$4 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$4")
                .bind(&job.id).bind(&job.lease_token).bind(ABANDONED).bind(now).execute(&mut *tx).await?;
            if changed.rows_affected() != 1 {
                return Err(ApiError::conflict("cloud_allocation_fenced"));
            }
            tx.commit().await?;
            tracing::warn!(workspace = %job.resource_id, idle_ms = idle,
                status = row["status_code"].as_str().unwrap_or(""),
                "Preparation abandoned by an offline guest; allocation stopped");
            return Ok(true);
        }
        // Ready/command admission is driven by the guest. This bounded recovery
        // observation neither delays it nor consumes the allocation retry budget.
        let changed = sqlx::query("UPDATE zuse_rust_jobs SET state='pending',attempts=GREATEST(attempts-1,0),last_error_code=NULL,lease_token=NULL,lease_expires_at=NULL,due_at=$3,updated_at=$4 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$4")
            .bind(&job.id).bind(&job.lease_token).bind(now + 10_000).bind(now).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        tx.commit().await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
