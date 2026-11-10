//! A retained guest's bootstrap extra is immutable across credential renewal.
//! Privileged memory transport therefore needs fresh server authority before the
//! guest receives Resume, independently of its stale locally retained flag.
use super::*;
const ADMISSION: &str = "rustMemoryTransportAdmission";
pub(in crate::runtime) fn pilot_binding(row: &Value) -> Value {
    json!({"version":1,"accountId":row["account_id"],"provider":row["provider"],"generation":row["request_config"]["runtimeGeneration"],"sourceMachineId":row["provider_sandbox_id"],"storageIncarnationId":row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"],"candidateSha256":row["request_config"]["managedCandidateSha256"],"managedReleaseId":row["request_config"]["managedReleaseId"]})
}
pub(in crate::runtime) fn pilot_admission(row: &Value, enabled: bool) -> Value {
    let mut marker = pilot_binding(row);
    marker["enabled"] = json!(enabled);
    marker
}
pub(super) fn same_pilot_row(before: &Value, after: &Value) -> bool {
    super::super::internal_observations::same_binding(before, after)
        && pilot_binding(before) == pilot_binding(after)
}
pub(super) fn parked_memory_resume(row: &Value, d: &Directive) -> bool {
    let parked = &row["request_config"]["rustMemoryParked"];
    d.action == Action::Resume
        && !d.full_shutdown
        && matches!(d.phase.as_str(), "waking" | "launching")
        && row["request_config"]["agent"] == "codex"
        && row["request_config"]["managedBaseReadiness"] == "uncertified-staging"
        && parked["version"] == 1
        && parked["generation"] == d.generation
        && parked["sourceMachineId"] == d.source_machine_id
        && parked["storageIncarnationId"] == d.storage_incarnation_id
}
impl RuntimeControl {
    /// Caller already holds the workspace lock. `allowed` was checked against
    /// CURRENT operator and full candidate pin before acquiring that SQL lock.
    pub(in crate::runtime) async fn memory_resume_authority_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        op: &Value,
        d: &Directive,
        early: &Value,
        allowed: bool,
        now: i64,
    ) -> Result<Option<Value>, ApiError> {
        if row["request_config"]["rustRuntimeRepair"]["resumeOperationId"] == d.operation_id
            && super::super::repair::leases_blocked(row)
        {
            let repair:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_runtime_repairs WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 AND source_machine_id=$4 AND storage_incarnation_id=$5 AND resume_operation_id=$6 AND phase='waiting-for-runtime')")
                .bind(row["request_config"]["rustRuntimeRepair"]["operationId"].as_str()).bind(required(row,"workspace_id")?).bind(d.generation).bind(&d.source_machine_id).bind(&d.storage_incarnation_id).bind(&d.operation_id).fetch_one(&mut **tx).await?;
            if repair {
                return Ok(None);
            }
        }
        let prior = &row["request_config"][ADMISSION];
        let allowed = allowed && same_pilot_row(early, row);
        // New bootstraps explicitly record both true AND false. A retained old
        // guest with no record is ambiguous, never evidence of a false flag.
        let enabled = prior["enabled"].as_bool();
        let bound = enabled.is_some_and(|value| *prior == pilot_admission(row, value));
        if !parked_memory_resume(row, d) {
            if allowed && prior.is_null() {
                sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustMemoryTransportAdmission}',$2),revision=revision+1 WHERE workspace_id=$1")
                    .bind(required(row,"workspace_id")?).bind(pilot_admission(row,true)).execute(&mut **tx).await?;
            }
            return Ok(None);
        }
        if (!prior.is_null() && !bound) || (!allowed && enabled != Some(false)) {
            return self
                .memory_recovery_report_tx(
                    tx,
                    row,
                    op,
                    d,
                    &d.operation_id,
                    super::memory_recovery::MEMORY_RECOVERY_CODE,
                    now,
                )
                .await
                .map(Some);
        }
        // Compatibility for a previously booted fe94 pilot: an authenticated
        // Query under the still-valid exact operator/pin establishes the fence.
        // This never enables a guest option or authorizes a socket operation.
        if allowed && prior.is_null() {
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustMemoryTransportAdmission}',$2),revision=revision+1 WHERE workspace_id=$1")
                .bind(required(row,"workspace_id")?).bind(pilot_admission(row,true)).execute(&mut **tx).await?;
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_transport_admission_binds_false_separately_from_missing_or_mismatched() {
        let row = json!({"account_id":"pilot","provider":"boxd","provider_sandbox_id":"machine","request_config":{"runtimeGeneration":1,"rustRuntimeStorage":{"storageIncarnationId":"disk"},"managedCandidateSha256":"a".repeat(64),"managedReleaseId":"release","agent":"codex"}});
        let yes = pilot_admission(&row, true);
        let no = pilot_admission(&row, false);
        assert_ne!(yes, no);
        assert_ne!(no, Value::Null);
        assert_eq!(no["enabled"], false);
        for (path, value) in [
            ("account_id", json!("other")),
            ("provider", json!("boat")),
            ("provider_sandbox_id", json!("replacement")),
        ] {
            let mut changed = row.clone();
            changed[path] = value;
            assert!(!same_pilot_row(&row, &changed));
            assert_ne!(yes, pilot_admission(&changed, true));
        }
        for key in [
            "runtimeGeneration",
            "rustRuntimeStorage",
            "managedCandidateSha256",
            "managedReleaseId",
        ] {
            let mut changed = row.clone();
            changed["request_config"][key] = Value::Null;
            assert!(!same_pilot_row(&row, &changed));
            assert_ne!(yes, pilot_admission(&changed, true));
        }
    }
}
