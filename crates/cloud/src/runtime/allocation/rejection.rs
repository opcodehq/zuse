//! A known rejected create requires an explicit authorized retry of its original intent.
//! So does a first start that passed its deadline without a provider machine:
//! both stop visibly (status `cloud_provider_rejected`, Retry and Delete).
use super::*;
use crate::access::{Access, Permission};
use serde::Deserialize;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Retry {
    pub command_id: String,
}
fn source_matches(row: &Value, op: &Value, marker: &Value) -> bool {
    marker["version"] == 1
        && marker["operationId"] == op["operation_id"]
        && marker["generation"] == op["generation"]
        && marker["artifactId"] == op["snapshot_id"]
        && marker["machineName"] == op["machine_name"]
        && marker["storageIncarnationId"] == op["storage_incarnation_id"]
        && row["request_config"]["rustAllocationOperationId"] == op["operation_id"]
        && row["request_config"]["runtimeGeneration"] == op["generation"]
        && row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
            == op["storage_incarnation_id"]
}
/// Marker state for a start whose provider machine exists but never enrolled
/// a runtime before the deadline. Its Retry runs the start again on that same
/// machine; no create is involved.
const MACHINE_PRESENT: &str = "machine-present";
fn machine_present(marker: &Value, row: &Value) -> bool {
    marker["createReceiptState"] == MACHINE_PRESENT
        && marker["machineId"].is_string()
        && marker["machineId"] == row["provider_sandbox_id"]
}
pub(crate) fn metadata(row: &Value) -> Value {
    let marker = &row["request_config"]["rustAllocationRejection"];
    let available = marker["version"] == 1
        && marker["operationId"] == row["request_config"]["rustAllocationOperationId"]
        && marker["generation"] == row["request_config"]["runtimeGeneration"]
        && marker["storageIncarnationId"]
            == row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
        && row["state"] == "provisioning"
        && row["desired_state"] == "ready"
        && (row["provider_sandbox_id"].is_null() || machine_present(marker, row))
        && !blocked(row);
    if available {
        json!({"operationId":marker["operationId"],"state":"rejected","errorCode":"cloud_provider_rejected","reason":marker["reason"].as_str().unwrap_or("rejected"),"retryAvailable":true,"rejectedAt":marker["rejectedAt"]})
    } else {
        Value::Null
    }
}
/// The create journal of an operation that has no provider machine identity.
/// `unknown`/`pending` only ever stop visibly after the start deadline; a
/// Retry then still reconciles the exact name before any new create.
fn receipt_state(receipt: Option<&Value>, name: &str) -> Option<&'static str> {
    match receipt {
        None => Some("not-submitted"),
        Some(r) if r["machine_name"] == name && r["machine_id"].is_null() => {
            match r["outcome"].as_str() {
                Some("rejected") => Some("rejected"),
                Some("unknown") => Some("unknown"),
                Some("pending") => Some("pending"),
                _ => None,
            }
        }
        _ => None,
    }
}
/// When the current start began: the original request, or the latest
/// authorized Retry of it.
fn start_began(row: &Value, op: &Value) -> i64 {
    let permit = &row["request_config"]["rustAllocationDispatchAuthorization"];
    let retried = (permit["operationId"] == op["operation_id"])
        .then(|| permit["approvedAt"].as_i64())
        .flatten()
        .unwrap_or(0);
    op["created_at"].as_i64().unwrap_or(0).max(retried)
}
/// A Retry renews the boot grant unless it still covers what the Retry needs:
/// the next attempt, or for a machine-present Retry the whole restarted start
/// deadline (a fresh grant lasts `BOOT_TTL`, longer than that deadline).
fn boot_refresh_needed(expires_at: i64, now: i64, machine_present: bool) -> bool {
    let needed = if machine_present {
        super::effects::ALLOCATION_START_DEADLINE_MS + 60_000
    } else {
        60_000
    };
    expires_at <= now.saturating_add(needed)
}
/// The start's machine exists, belongs to this exact operation, and the start
/// has run past its deadline without an enrolled runtime.
fn started_overdue(row: &Value, op: &Value, now: i64) -> bool {
    let retry = &row["request_config"]["rustAllocationStartRetry"];
    let retried = (retry["operationId"] == op["operation_id"])
        .then(|| retry["approvedAt"].as_i64())
        .flatten()
        .unwrap_or(0);
    matches!(op["phase"].as_str(), Some("allocated" | "launching"))
        && op["machine_id"].is_string()
        && op["machine_id"] == row["provider_sandbox_id"]
        && row["request_config"]["rustAllocationOperationId"] == op["operation_id"]
        && now.saturating_sub(start_began(row, op).max(retried))
            >= super::effects::ALLOCATION_START_DEADLINE_MS
}
impl RuntimeControl {
    pub(super) async fn stop_rejected_allocation(
        &self,
        job: &jobs::Job,
        now: i64,
    ) -> Result<bool, ApiError> {
        self.stop_allocation(job, now, false).await
    }
    /// Stops a first start that has had no provider machine for
    /// `ALLOCATION_START_DEADLINE_MS`, whatever its last error was.
    pub(super) async fn stop_overdue_allocation(
        &self,
        job: &jobs::Job,
        now: i64,
    ) -> Result<bool, ApiError> {
        self.stop_allocation(job, now, true).await
    }
    /// Stops a first start whose provider machine exists but has not enrolled
    /// a runtime within `ALLOCATION_START_DEADLINE_MS`, whatever kept it (a
    /// helper that keeps refusing, a runtime that never came up). Like the
    /// machine-less case it stops visibly with Retry and Delete; Retry runs the
    /// start again on the same machine. The machine itself stays under the
    /// awake backstop until Retry or Delete.
    pub(super) async fn stop_overdue_started_allocation(
        &self,
        job: &jobs::Job,
        now: i64,
    ) -> Result<bool, ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        let config = &row["request_config"];
        if generation != job.generation
            || blocked(&row)
            || row["desired_state"] != "ready"
            || row["state"] != "provisioning"
            || !row["provider_sandbox_id"].is_string()
            || !row["runtime_credential_hash"].is_null()
            || !config["rustStorageEnrollment"].is_null()
            || !config["runtimeBootstrapReceipt"].is_null()
            || row["runtime_state"] == "online"
            || super::super::repair::leases_blocked(&row)
            || super::super::recreation::leases_blocked(&row)
        {
            return Ok(false);
        }
        let op:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 FOR UPDATE")
            .bind(job.payload["operationId"].as_str()).bind(&job.resource_id).bind(generation).fetch_optional(&mut *tx).await?;
        let Some(op) = op else { return Ok(false) };
        if !started_overdue(&row, &op, now) {
            return Ok(false);
        }
        let changed=sqlx::query("UPDATE zuse_rust_jobs SET state='failed',last_error_code='cloud_provider_rejected',lease_token=NULL,lease_expires_at=NULL,updated_at=$3 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3 AND resource_generation=$4")
            .bind(&job.id).bind(&job.lease_token).bind(now).bind(generation).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Ok(false);
        }
        let mut config = config.clone();
        config["rustAllocationRejection"] = json!({"version":1,"operationId":op["operation_id"],"generation":generation,"artifactId":op["snapshot_id"],"machineName":op["machine_name"],"machineId":op["machine_id"],"storageIncarnationId":op["storage_incarnation_id"],"rejectedAt":now,"createReceiptState":MACHINE_PRESENT,"reason":"deadline"});
        sqlx::query("UPDATE api_cloud_workspaces SET status_code='cloud_provider_rejected',request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1")
            .bind(&job.resource_id).bind(config).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        tracing::warn!(workspace=%job.resource_id, operation=op["operation_id"].as_str().unwrap_or(""),
            machine=op["machine_id"].as_str().unwrap_or(""),
            "Cloud start did not enroll a runtime before its deadline; Retry or Delete required");
        Ok(true)
    }
    async fn stop_allocation(
        &self,
        job: &jobs::Job,
        now: i64,
        overdue: bool,
    ) -> Result<bool, ApiError> {
        let journal_exists: bool = crate::db::schema::present(
            &self.pool,
            "SELECT to_regclass('zuse_rust_provider_creates') IS NOT NULL",
        )
        .await?;
        if !journal_exists {
            return Ok(false);
        }
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        if generation != job.generation
            || blocked(&row)
            || row["desired_state"] != "ready"
            || row["state"] != "provisioning"
            || !row["provider_sandbox_id"].is_null()
        {
            return Ok(false);
        }
        let op:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 FOR UPDATE")
            .bind(job.payload["operationId"].as_str()).bind(&job.resource_id).bind(generation).fetch_optional(&mut *tx).await?;
        let Some(op) = op else { return Ok(false) };
        let phase_ok = op["phase"] == "creating" || (overdue && op["phase"] == "planned");
        if !phase_ok
            || !op["machine_id"].is_null()
            || row["request_config"]["rustAllocationOperationId"] != op["operation_id"]
        {
            return Ok(false);
        }
        if overdue
            && now.saturating_sub(start_began(&row, &op))
                < super::effects::ALLOCATION_START_DEADLINE_MS
        {
            return Ok(false);
        }
        let receipt:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM zuse_rust_provider_creates c WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE")
            .bind(op["operation_id"].as_str()).bind(&job.resource_id).fetch_optional(&mut *tx).await?;
        let Some(create_state) = receipt_state(receipt.as_ref(), required(&op, "machine_name")?)
        else {
            return Ok(false);
        };
        if !overdue && !matches!(create_state, "not-submitted" | "rejected") {
            return Ok(false);
        }
        let changed=sqlx::query("UPDATE zuse_rust_jobs SET state='failed',last_error_code='cloud_provider_rejected',lease_token=NULL,lease_expires_at=NULL,updated_at=$3 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3 AND resource_generation=$4")
            .bind(&job.id).bind(&job.lease_token).bind(now).bind(generation).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Ok(false);
        }
        let mut config = row["request_config"].clone();
        config["rustAllocationRejection"] = json!({"version":1,"operationId":op["operation_id"],"generation":generation,"artifactId":op["snapshot_id"],"machineName":op["machine_name"],"storageIncarnationId":op["storage_incarnation_id"],"rejectedAt":now,"createReceiptState":create_state,"reason":if overdue {"deadline"} else {"rejected"}});
        sqlx::query("UPDATE api_cloud_workspaces SET status_code='cloud_provider_rejected',request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1")
            .bind(&job.resource_id).bind(config).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        tracing::warn!(workspace=%job.resource_id, operation=op["operation_id"].as_str().unwrap_or(""),
            create_state, overdue, "Cloud allocation stopped; Retry or Delete required");
        Ok(true)
    }
    #[tracing::instrument(name="cloud_allocation_stage", skip_all, fields(workspace=%job.resource_id, operation="begin_dispatch"))]
    pub(super) async fn begin_allocation_dispatch(
        &self,
        job: &jobs::Job,
        operation: &Value,
        absent_confirmed: bool,
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, lease_until, observed_operation, capacity_observation) = if let Some(
            snapshot,
        ) =
            self.allocation_dispatch_snapshot(&mut tx, job).await?
        {
            (
                snapshot.workspace,
                snapshot.generation,
                snapshot.capacity["leaseUntil"].as_i64(),
                Some(snapshot.operation),
                Some(snapshot.capacity),
            )
        } else {
            let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
            let lease_until:Option<i64>=sqlx::query_scalar("SELECT lease_expires_at FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3 AND resource_generation=$4 FOR UPDATE")
                    .bind(&job.id).bind(&job.lease_token).bind(clock_ms()).bind(generation).fetch_optional(&mut *tx).await?;
            (row, generation, lease_until, None, None)
        };
        let now = clock_ms();
        if lease_until.is_none()
            || generation != job.generation
            || blocked(&row)
            || row["desired_state"] != "ready"
            || row["state"] != "provisioning"
            || !row["provider_sandbox_id"].is_null()
            || super::super::repair::leases_blocked(&row)
            || super::super::recreation::leases_blocked(&row)
        {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        let current: Value = if let Some(operation) = observed_operation {
            operation
        } else {
            sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 FOR UPDATE")
                .bind(operation["operation_id"].as_str()).bind(&job.resource_id).bind(generation).fetch_one(&mut *tx).await?
        };
        if current["phase"] != "creating"
            || !current["machine_id"].is_null()
            || row["request_config"]["rustAllocationOperationId"] != current["operation_id"]
            || [
                "operation_id",
                "snapshot_id",
                "machine_name",
                "storage_incarnation_id",
            ]
            .iter()
            .any(|k| current[*k] != operation[*k])
        {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        let journal_ready: bool = if capacity_observation.is_some() {
            true
        } else {
            crate::db::schema::present_in(
                &mut *tx,
                "SELECT to_regclass('zuse_rust_provider_creates') IS NOT NULL",
            )
            .await?
        };
        if !journal_ready {
            tx.commit().await?;
            return Ok(());
        }
        let journal: Option<Value> = if let Some(observed) = &capacity_observation {
            observed
                .get("journal")
                .filter(|journal| !journal.is_null())
                .cloned()
        } else {
            sqlx::query_scalar("SELECT to_jsonb(c) FROM zuse_rust_provider_creates c WHERE operation_id=$1 FOR UPDATE")
                .bind(current["operation_id"].as_str()).fetch_optional(&mut *tx).await?
        };
        if journal.as_ref().is_some_and(|j| {
            j["workspace_id"] != row["workspace_id"] || j["machine_name"] != current["machine_name"]
        }) {
            return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
        }
        let mut config = row["request_config"].clone();
        let permit = &config["rustAllocationDispatchAuthorization"];
        let prior_rejected = journal
            .as_ref()
            .is_some_and(|j| j["outcome"] == "rejected" && j["machine_id"].is_null());
        let has_permit = permit["version"] == 1
            && permit["state"] == "approved"
            && source_matches(&row, &current, permit)
            && permit["expectedReceipt"] == journal.clone().unwrap_or(Value::Null)
            && permit["commandId"].as_str().is_some_and(|id| {
                let accepted = &config["rustAllocationRetries"][id];
                accepted["state"] == "accepted"
                    && accepted["commandId"] == permit["commandId"]
                    && source_matches(&row, &current, accepted)
                    && accepted["priorProviderReceipt"] == permit["expectedReceipt"]
            });
        if prior_rejected && !has_permit {
            return Err(ApiError::unavailable("cloud_provider_rejected"));
        }
        // An uncertain create (lost response, or a worker that died after the
        // dispatch claim) that is still not visible by its exact name after the
        // resolution window was never accepted: dispatch it again under the
        // same name. A late original then conflicts on that name and is
        // adopted by the next reconciliation instead of duplicating.
        let unresolved_since = journal
            .as_ref()
            .filter(|j| {
                matches!(j["outcome"].as_str(), Some("unknown" | "pending"))
                    && j["machine_id"].is_null()
            })
            .and_then(|j| j["updated_at"].as_i64());
        let resolved_absent = absent_confirmed
            && unresolved_since.is_some_and(|since| {
                since <= now.saturating_sub(super::effects::UNRESOLVED_CREATE_WINDOW_MS)
            });
        if journal.is_some() && !prior_rejected && !resolved_absent {
            return Err(ApiError::unavailable(
                "cloud_provider_allocation_outcome_unknown",
            ));
        }
        // An approved retry for a preflight failure must also match its original
        // no-submission observation; a dispatched permit can never be reused.
        if journal.is_none() && !permit.is_null() && !has_permit {
            return Err(ApiError::unavailable(
                "cloud_provider_allocation_outcome_unknown",
            ));
        }
        self.allocation_snapshot_capacity(&mut tx, &row, capacity_observation.as_ref(), now)
            .await?;
        if prior_rejected {
            sqlx::query("UPDATE zuse_rust_provider_creates SET outcome='pending',updated_at=$2 WHERE operation_id=$1 AND outcome='rejected' AND machine_id IS NULL").bind(current["operation_id"].as_str()).bind(now).execute(&mut *tx).await?;
        } else if resolved_absent {
            let changed=sqlx::query("UPDATE zuse_rust_provider_creates SET outcome='pending',updated_at=$2 WHERE operation_id=$1 AND outcome IN ('unknown','pending') AND machine_id IS NULL AND updated_at=$3").bind(current["operation_id"].as_str()).bind(now).bind(unresolved_since).execute(&mut *tx).await?;
            if changed.rows_affected() != 1 {
                return Err(ApiError::conflict("cloud_allocation_fenced"));
            }
            tracing::warn!(workspace=%job.resource_id, operation=current["operation_id"].as_str().unwrap_or(""),
                "Uncertain provider create not found by name after the resolution window; dispatching again");
        } else {
            sqlx::query("INSERT INTO zuse_rust_provider_creates(operation_id,workspace_id,machine_name,outcome,updated_at) VALUES($1,$2,$3,'pending',$4)")
                .bind(current["operation_id"].as_str()).bind(&job.resource_id).bind(current["machine_name"].as_str()).bind(now).execute(&mut *tx).await?;
        }
        if has_permit {
            config["rustAllocationDispatchAuthorization"]["state"] = json!("dispatched");
            config["rustAllocationDispatchAuthorization"]["dispatchedAt"] = json!(now);
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(&job.resource_id).bind(config).bind(now).execute(&mut *tx).await?;
        }
        if lease_until.is_none_or(|until| until <= clock_ms()) {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn retry_allocation(
        &self,
        id: &str,
        operation: &str,
        access: &Access,
        input: Retry,
        now: i64,
    ) -> Result<Value, ApiError> {
        if uuid::Uuid::parse_str(&input.command_id).is_err()
            || uuid::Uuid::parse_str(operation).is_err()
        {
            return Err(ApiError::bad_request("invalid_allocation_retry"));
        }
        self.scope_admission(access).await?;
        self.scope_access(id, Some(&access.actor.account_id), true)
            .await?;
        let observed: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        self.candidate_row_allowed(&observed, now).await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        let now = clock_ms();
        if access.workspace_permission(required(&row, "account_id")?, &row["request_config"])?
            != Permission::Edit
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        if blocked(&row)
            || row["desired_state"] != "ready"
            || row["request_config"]["rustAllocationOperationId"] != operation
            || row["request_config"]["runtimeGeneration"] != generation
            || row["account_id"] != observed["account_id"]
            || row["request_config"] != observed["request_config"]
        {
            return Err(ApiError::conflict("cloud_allocation_retry_fenced"));
        }
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 FOR UPDATE")
            .bind(operation).bind(id).bind(generation).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::conflict("cloud_allocation_retry_fenced"))?;
        let mut config = row["request_config"].clone();
        if let Some(prior) = config["rustAllocationRetries"].get(&input.command_id) {
            if prior["actorId"] != access.actor.account_id
                || prior["operationId"] != operation
                || prior["generation"] != generation
                || prior["artifactId"] != op["snapshot_id"]
                || prior["machineName"] != op["machine_name"]
                || prior["storageIncarnationId"] != op["storage_incarnation_id"]
            {
                return Err(ApiError::conflict("cloud_allocation_retry_conflict"));
            }
            return Ok(prior.clone());
        }
        let present = machine_present(&config["rustAllocationRejection"], &row);
        let phase_ok = if present {
            matches!(op["phase"].as_str(), Some("allocated" | "launching"))
                && op["machine_id"] == config["rustAllocationRejection"]["machineId"]
        } else {
            matches!(op["phase"].as_str(), Some("creating" | "planned"))
                && op["machine_id"].is_null()
        };
        if metadata(&row).is_null()
            || !source_matches(&row, &op, &config["rustAllocationRejection"])
            || !phase_ok
            || !row["runtime_credential_hash"].is_null()
            || !config["rustStorageEnrollment"].is_null()
            || !config["runtimeBootstrapReceipt"].is_null()
            || super::super::repair::leases_blocked(&row)
            || super::super::recreation::leases_blocked(&row)
        {
            return Err(ApiError::conflict("cloud_allocation_retry_unavailable"));
        }
        let receipt:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(c) FROM zuse_rust_provider_creates c WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE")
            .bind(operation).bind(id).fetch_optional(&mut *tx).await?;
        let expected = config["rustAllocationRejection"]["createReceiptState"]
            .as_str()
            .unwrap_or("rejected");
        // A machine-present start already has its machine: nothing is created
        // again, so there is no create receipt to reconcile.
        if !present
            && receipt_state(receipt.as_ref(), required(&op, "machine_name")?) != Some(expected)
        {
            return Err(ApiError::conflict("cloud_allocation_retry_outcome_unknown"));
        }
        self.allocation_capacity(&mut tx, &row, now).await?;
        let ledger = config
            .get("rustAllocationRetries")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !ledger.is_object() || ledger.as_object().is_some_and(|v| v.len() >= 128) {
            return Err(ApiError::conflict("cloud_allocation_retry_ledger_full"));
        }
        config["rustAllocationRetries"] = ledger;
        let receipt = json!({"version":1,"priorProviderReceipt":receipt,"commandId":input.command_id,"workspaceId":id,"operationId":operation,"actorId":access.actor.account_id,"generation":generation,"artifactId":op["snapshot_id"],"machineName":op["machine_name"],"storageIncarnationId":op["storage_incarnation_id"],"state":"accepted","requestedAt":now});
        config["rustAllocationRetries"][&input.command_id] = receipt.clone();
        if present {
            // The deadline restarts from this Retry.
            config["rustAllocationStartRetry"] = json!({"version":1,"operationId":operation,"commandId":input.command_id,"approvedAt":now});
        } else {
            config["rustAllocationDispatchAuthorization"] = json!({"version":1,"state":"approved","commandId":input.command_id,"operationId":operation,"generation":generation,"artifactId":op["snapshot_id"],"machineName":op["machine_name"],"storageIncarnationId":op["storage_incarnation_id"],"expectedReceipt":receipt["priorProviderReceipt"],"approvedAt":now});
        }
        config
            .as_object_mut()
            .ok_or_else(|| ApiError::conflict("cloud_allocation_retry_fenced"))?
            .remove("rustAllocationRejection");
        let changed=sqlx::query("UPDATE zuse_rust_jobs SET state='pending',due_at=$4,last_error_code=NULL,max_attempts=GREATEST(max_attempts,attempts+12),updated_at=$4 WHERE resource_id=$1 AND resource_generation=$2 AND kind='workspace-allocate' AND payload->>'operationId'=$3 AND state='failed' AND last_error_code='cloud_provider_rejected' AND lease_token IS NULL AND lease_expires_at IS NULL")
            .bind(id).bind(generation).bind(operation).bind(now).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ApiError::conflict("cloud_allocation_retry_not_required"));
        }
        // No recipient or storage has enrolled. Only an explicit authorized retry
        // can refresh an expired boot grant, preserving operation/name/disk.
        if boot_refresh_needed(op["boot_expires_at"].as_i64().unwrap_or(0), now, present) {
            let boot = Zeroizing::new(format!(
                "rust_boot_{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ));
            let sealed = self
                .master
                .seal_api(&boot_context(id, operation, generation), &boot)
                .map_err(|_| ApiError::unavailable("cloud_boot_grant_unavailable"))?;
            sqlx::query("UPDATE zuse_rust_allocations SET sealed_boot_token=$2,boot_expires_at=$3,updated_at=$4 WHERE operation_id=$1").bind(operation).bind(sealed).bind(now+BOOT_TTL).bind(now).execute(&mut *tx).await?;
            sqlx::query("UPDATE api_cloud_workspaces SET runtime_boot_token_hash=$2,runtime_boot_token_expires_at=$3 WHERE workspace_id=$1").bind(id).bind(hash(&boot)).bind(now+BOOT_TTL).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,status_code='allocating',revision=revision+1,updated_at=GREATEST(updated_at+1,$3),next_action_at=$3 WHERE workspace_id=$1").bind(id).bind(config).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod machine_present_tests {
    use super::*;
    const DEADLINE: i64 = super::super::effects::ALLOCATION_START_DEADLINE_MS;
    fn started() -> (Value, Value) {
        let op = json!({"operation_id":"op","phase":"launching","machine_id":"bx_one","created_at":1_000,
            "snapshot_id":"art","machine_name":"name","storage_incarnation_id":"disk","generation":1});
        let row = json!({"state":"provisioning","desired_state":"ready","provider_sandbox_id":"bx_one",
            "request_config":{"rustAllocationOperationId":"op","runtimeGeneration":1,
                "rustRuntimeStorage":{"storageIncarnationId":"disk"}}});
        (row, op)
    }
    #[test]
    fn a_started_machine_that_never_enrolls_is_overdue_only_after_the_deadline() {
        let (mut row, mut op) = started();
        assert!(!started_overdue(&row, &op, 1_000 + DEADLINE - 1));
        assert!(started_overdue(&row, &op, 1_000 + DEADLINE));
        // A Retry restarts the deadline.
        row["request_config"]["rustAllocationStartRetry"] =
            json!({"version":1,"operationId":"op","approvedAt":50_000});
        assert!(!started_overdue(&row, &op, 1_000 + DEADLINE));
        assert!(started_overdue(&row, &op, 50_000 + DEADLINE));
        // Another operation's machine, or a start before its machine, is not this one.
        row["provider_sandbox_id"] = json!("bx_other");
        assert!(!started_overdue(&row, &op, i64::MAX / 2));
        row["provider_sandbox_id"] = json!("bx_one");
        op["phase"] = json!("creating");
        assert!(!started_overdue(&row, &op, i64::MAX / 2));
    }
    #[test]
    fn a_machine_present_retry_keeps_the_boot_grant_through_its_restarted_deadline() {
        let now = 1_000_000;
        // A grant with 15 minutes left serves a create retry, not a restarted
        // 10-minute start that may finish near its end.
        let fifteen = now + 15 * 60_000;
        assert!(!boot_refresh_needed(fifteen, now, false));
        assert!(!boot_refresh_needed(fifteen, now, true));
        let eight = now + 8 * 60_000;
        assert!(!boot_refresh_needed(eight, now, false));
        assert!(boot_refresh_needed(eight, now, true));
        // A fresh grant always covers the restarted deadline.
        assert!(!boot_refresh_needed(now + BOOT_TTL, now, true));
        assert!(boot_refresh_needed(now, now, false));
    }
    #[test]
    fn a_machine_present_stop_offers_retry_for_that_machine_only() {
        let (mut row, op) = started();
        row["request_config"]["rustAllocationRejection"] = json!({"version":1,"operationId":"op",
            "generation":1,"artifactId":op["snapshot_id"],"machineName":op["machine_name"],
            "machineId":"bx_one","storageIncarnationId":"disk","rejectedAt":9,
            "createReceiptState":MACHINE_PRESENT,"reason":"deadline"});
        let shown = metadata(&row);
        assert_eq!(shown["state"], "rejected");
        assert_eq!(shown["retryAvailable"], true);
        assert_eq!(shown["errorCode"], "cloud_provider_rejected");
        row["provider_sandbox_id"] = json!("bx_replaced");
        assert!(metadata(&row).is_null());
        // A machine-less rejection never applies once a machine exists.
        row["provider_sandbox_id"] = json!("bx_one");
        row["request_config"]["rustAllocationRejection"]["createReceiptState"] = json!("rejected");
        assert!(metadata(&row).is_null());
    }
}
