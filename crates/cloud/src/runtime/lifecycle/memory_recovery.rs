//! Typed attention is separate from canonical provider lifecycle phases. The
//! original operation never receives a fake Ready/Parked receipt.
use super::*;
pub(super) const MEMORY_RECOVERY_CODE: &str = "codex_memory_transport_recovery_required";
fn original_candidate(row: &Value) -> bool {
    let config = &row["request_config"];
    config["managedBaseReadiness"] == "uncertified-staging"
        && config["managedReleaseId"]
            .as_str()
            .is_some_and(|v| !v.is_empty())
        && config["managedCandidateSha256"]
            .as_str()
            .is_some_and(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}
pub(in crate::runtime) fn valid_memory_recovery(row: &Value) -> bool {
    let config = &row["request_config"];
    let marker = &config["rustMemoryTransportRecovery"];
    marker["version"] == 1
        && original_candidate(row)
        && marker["candidateSha256"] == config["managedCandidateSha256"]
        && marker["managedReleaseId"] == config["managedReleaseId"]
        && marker["code"] == MEMORY_RECOVERY_CODE
        && marker["generation"] == config["runtimeGeneration"]
        && marker["sourceMachineId"] == row["provider_sandbox_id"]
        && marker["storageIncarnationId"] == config["rustRuntimeStorage"]["storageIncarnationId"]
        && marker["operationId"]
            .as_str()
            .is_some_and(|v| uuid::Uuid::parse_str(v).is_ok())
}
pub(in crate::runtime) fn memory_recovery_for(row: &Value, op: &Value) -> Option<Value> {
    (valid_memory_recovery(row)
        && row["request_config"]["rustMemoryTransportRecovery"]["operationId"]
            == op["operation_id"])
        .then(|| row["request_config"]["rustMemoryTransportRecovery"].clone())
}
impl RuntimeControl {
    pub(in crate::runtime) async fn memory_recovery_report_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        op: &Value,
        d: &Directive,
        operation: &str,
        code: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        if code != MEMORY_RECOVERY_CODE
            || !original_candidate(row)
            || operation != d.operation_id
            || d.full_shutdown
            || row["request_config"]["agent"] != "codex"
            || !matches!(
                (d.action, d.phase.as_str()),
                (Action::Pause, "quiescing") | (Action::Resume, "waking" | "launching")
            )
        {
            return Err(ApiError::conflict("cloud_memory_recovery_receipt_rejected"));
        }
        if let Some(prior) = memory_recovery_for(row, op) {
            return Ok(json!({"directive":null,"recoveryRequired":prior}));
        }
        if !row["request_config"]["rustMemoryTransportRecovery"].is_null() {
            return Err(ApiError::conflict("cloud_memory_recovery_receipt_rejected"));
        }
        // This terminal receipt grants no effect. Operator withdrawal/expired
        // pilot configuration must not prevent cleanup of an originally bound
        // candidate; the privileged reset has its separate renewed flag gate.
        let marker = json!({"version":1,"operationId":operation,"action":d.action,"generation":d.generation,"sourceMachineId":d.source_machine_id,"storageIncarnationId":d.storage_incarnation_id,"candidateSha256":row["request_config"]["managedCandidateSha256"],"managedReleaseId":row["request_config"]["managedReleaseId"],"code":code,"reportedAt":now});
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustMemoryTransportRecovery}',$2),state='ready',desired_state='ready',runtime_state='offline',status_code=$3,revision=revision+1,updated_at=GREATEST(updated_at+1,$4),next_action_at=9007199254740991 WHERE workspace_id=$1")
   .bind(required(row,"workspace_id")?).bind(&marker).bind(MEMORY_RECOVERY_CODE).bind(now).execute(&mut **tx).await?;
        sqlx::query("UPDATE zuse_rust_lifecycle_operations SET provider_effects=jsonb_set(provider_effects,'{memoryRecoveryRequired}',$2),updated_at=GREATEST(updated_at+1,$3) WHERE operation_id=$1")
   .bind(operation).bind(&marker).bind(now).execute(&mut **tx).await?;
        // Fence an in-flight worker too; its late delivery cannot clear attention or
        // manufacture a successful resume. Provider receipts remain inspectable.
        sqlx::query("UPDATE zuse_rust_jobs SET state='failed',lease_token=NULL,lease_expires_at=NULL,last_error_code=$4,updated_at=GREATEST(updated_at+1,$5) WHERE kind=$1 AND resource_id=$2 AND payload->>'operationId'=$3 AND state IN ('pending','running')")
   .bind(JOB_KIND).bind(required(row,"workspace_id")?).bind(operation).bind(MEMORY_RECOVERY_CODE).bind(now).execute(&mut **tx).await?;
        Ok(json!({"directive":null,"recoveryRequired":marker}))
    }
}
