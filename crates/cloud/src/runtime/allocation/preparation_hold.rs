//! A committed diagnostic gateway owns preparation, including failed stages.
use super::*;

pub(super) const ACTIVE: &str = "cloud_allocation_preparation_active";
/// The job error when a hold ends because the guest stopped preparing.
pub(super) const ABANDONED: &str = "cloud_allocation_preparation_abandoned";
/// How long an offline guest may leave preparation unchanged before the hold
/// ends. An online guest is still working and is held however long it takes.
pub(super) const ABANDON_AFTER_MS: i64 = 30 * 60 * 1000;
/// Workspace config key naming the allocation whose hold was abandoned, so a
/// later ready receipt can complete it without probing on every receipt.
const ABANDONED_MARKER: &str = "rustPreparationHoldAbandoned";

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
            // Internal bookkeeping only: the user-visible status and revision
            // stay as they were.
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,ARRAY[$2],to_jsonb($3::text)) WHERE workspace_id=$1")
                .bind(&job.resource_id).bind(ABANDONED_MARKER).bind(job.payload["operationId"].as_str()).execute(&mut *tx).await?;
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

    /// Completes an allocation whose hold was abandoned when its guest comes
    /// back and reports ready. A live hold would have completed it on its next
    /// recheck; the abandoned job no longer runs, so the receipt does it here.
    ///
    /// Runs inside the receipt's transaction, under its workspace lock, after
    /// the receipt was authenticated and applied. `row` is the workspace after
    /// that update and `generation` the ownership generation held by the lock.
    /// Only an allocation job failed with [`ABANDONED`] qualifies; every other
    /// failure stays failed. Without the abandonment marker for the current
    /// allocation this returns before any query, so ordinary receipts pay
    /// nothing. Success clears the marker; repeat receipts change nothing.
    pub(in crate::runtime) async fn reconcile_abandoned_allocation(
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        generation: i64,
        now: i64,
    ) -> Result<bool, ApiError> {
        let config = &row["request_config"];
        if !config[ABANDONED_MARKER].is_string()
            || config[ABANDONED_MARKER] != config["rustAllocationOperationId"]
        {
            return Ok(false);
        }
        // The same completion the allocation worker accepts as canonical.
        let ready = row["runtime_state"] == "online"
            && storage_enrolled(row)
            && (config["rustCompletedLaunchCommandId"].is_string() || Self::early_agent_ready(row));
        let (Some(operation), Some(machine), Some(disk)) = (
            config["rustAllocationOperationId"].as_str(),
            row["provider_sandbox_id"].as_str(),
            config["rustRuntimeStorage"]["storageIncarnationId"].as_str(),
        ) else {
            return Ok(false);
        };
        if !ready
            || blocked(row)
            || row["desired_state"] != "ready"
            || config["runtimeGeneration"].as_i64() != Some(generation)
            || crate::runtime::repair::leases_blocked(row)
            || crate::runtime::recreation::leases_blocked(row)
        {
            return Ok(false);
        }
        // Allocation, job and marker change together or not at all: the
        // allocation only moves while its abandoned job exists, the job only
        // completes once that allocation moved, and the marker only clears
        // once the job completed.
        let job = "j.kind=$5 AND j.resource_id=$2 AND j.resource_generation=$3 AND j.payload->>'operationId'=$1 AND j.state='failed' AND j.last_error_code=$6 AND j.lease_token IS NULL";
        let statement = format!(
            "WITH allocation AS (UPDATE zuse_rust_allocations SET phase='ready',sealed_boot_token=NULL,updated_at=GREATEST(updated_at,$4) WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 AND phase='launching' AND machine_id=$7 AND storage_incarnation_id=$8 AND EXISTS(SELECT 1 FROM zuse_rust_jobs j WHERE {job}) RETURNING operation_id), job AS (UPDATE zuse_rust_jobs j SET state='completed',last_error_code=NULL,updated_at=GREATEST(updated_at,$4) WHERE {job} AND EXISTS(SELECT 1 FROM allocation) RETURNING job_id) UPDATE api_cloud_workspaces SET request_config=request_config-$9 WHERE workspace_id=$2 AND EXISTS(SELECT 1 FROM job)"
        );
        let changed = sqlx::query(&statement)
            .bind(operation)
            .bind(&row["workspace_id"].as_str())
            .bind(generation)
            .bind(now)
            .bind(JOB_KIND)
            .bind(ABANDONED)
            .bind(machine)
            .bind(disk)
            .bind(ABANDONED_MARKER)
            .execute(&mut **tx)
            .await?;
        let reconciled = changed.rows_affected() == 1;
        if reconciled {
            tracing::info!(
                workspace = row["workspace_id"].as_str().unwrap_or(""),
                operation,
                "Guest became ready after an abandoned preparation hold; allocation completed"
            );
        }
        Ok(reconciled)
    }
}

#[cfg(test)]
mod tests;
