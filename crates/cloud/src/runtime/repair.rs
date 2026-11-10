//! Durable same-computer repair with generation, native disk and command fences.
//! Only the resident supervisor may retire the bound old process tree and start
//! the retained engine. Original attempted command IDs are never replayed.
use super::*;
use crate::{
    access::{Access, Permission},
    jobs,
    providers::{Machine, MachineState, ProviderError},
};
use async_trait::async_trait;
use serde::Deserialize;
mod preparation;
#[cfg(test)]
mod tests;
mod worker;
pub const JOB_KIND: &str = "workspace-runtime-repair";
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start {
    pub idempotency_key: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Resolve {
    pub decision: String,
}
#[async_trait]
pub trait RepairProvider: Send + Sync {
    fn provider_id(&self) -> &'static str;
    async fn inspect(&self, machine: &str) -> Result<Option<Machine>, ProviderError>;
    async fn rpc(&self, machine: &str, request: &Value) -> Result<Value, ProviderError>;
}
#[async_trait]
impl RepairProvider for super::provider::RuntimeProvider {
    fn provider_id(&self) -> &'static str {
        self.provider.id().as_str()
    }
    async fn inspect(&self, machine: &str) -> Result<Option<Machine>, ProviderError> {
        self.provider.inspect(machine).await
    }
    async fn rpc(&self, machine: &str, request: &Value) -> Result<Value, ProviderError> {
        let request = serde_json::to_string(request).map_err(|_| ProviderError::Rejected)?;
        if request.len() > 65536 {
            return Err(ProviderError::Rejected);
        }
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        let command = format!(
            "printf %s {} | sudo -n -u zuse python3 /usr/local/lib/zuse/runtime-supervisor.py rpc",
            quote(&request)
        );
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(45),
            self.provider.execute(machine, &command),
        )
        .await
        .map_err(|_| ProviderError::Unavailable)??;
        if result.exit_code != 0 || result.stdout.len() > 65536 {
            return Err(ProviderError::InvalidResponse);
        }
        serde_json::from_slice(&result.stdout).map_err(|_| ProviderError::InvalidResponse)
    }
}
fn public(op: &Value) -> Value {
    json!({"operationId":op["operation_id"],"workspaceId":op["workspace_id"],"state":op["phase"],"requiresAttention":matches!(op["phase"].as_str(),Some("attention-required"|"failed")),"recovery":op["recovery"],"errorCode":op["error_code"],"createdAt":op["created_at"],"updatedAt":op["updated_at"]})
}
pub(super) fn leases_blocked(row: &Value) -> bool {
    row["request_config"]["rustRuntimeRepair"]["phase"]
        .as_str()
        .is_some_and(|phase| phase != "ready")
}
pub(crate) fn workspace_metadata(row: &Value) -> Value {
    let config = &row["request_config"];
    let lifecycle_pending = config["rustLifecycleOperation"]["phase"]
        .as_str()
        .is_some_and(|phase| !matches!(phase, "ready" | "paused"));
    let memory_attention = lifecycle::valid_memory_recovery(row)
        && config["rustMemoryTransportRecovery"]["operationId"]
            == config["rustLifecycleOperation"]["id"];
    let available = config["rustRuntimeRepairVersion"] == 1
        && config["rustRuntimeSupervisorVersion"] == 1
        && storage_enrolled(row)
        && !blocked(row)
        && (row["state"] == "ready" || preparation::failed_preparation_ingress(row))
        && row["desired_state"] == "ready"
        && !leases_blocked(row)
        && (!lifecycle_pending || memory_attention)
        && !super::recreation::leases_blocked(row);
    let marker = &config["rustRuntimeRepair"];
    let operation = marker["operationId"]
        .as_str()
        .filter(|s| uuid::Uuid::parse_str(s).is_ok());
    let phase = marker["phase"].as_str().filter(|s| {
        matches!(
            *s,
            "quiescing"
                | "stopping"
                | "stopped"
                | "starting"
                | "waiting-for-runtime"
                | "attention-required"
                | "ready"
                | "failed"
        )
    });
    let summary = match (operation, phase) {
        (Some(operation), Some(phase)) => {
            json!({"operationId":operation,"state":phase,"requiresAttention":phase=="attention-required"||phase=="failed","updatedAt":marker["updatedAt"].as_i64(),"errorCode":marker["errorCode"].as_str().filter(|s|s.len()<=128&&s.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'||b==b'-'))})
        }
        _ => Value::Null,
    };
    json!({"runtimeRepairAvailable":available,"runtimeRepair":summary})
}
impl RuntimeControl {
    pub async fn restart_runtime(
        &self,
        id: &str,
        access: &Access,
        input: Start,
        now: i64,
    ) -> Result<Value, ApiError> {
        if input.idempotency_key.is_empty()
            || input.idempotency_key.len() > 160
            || input.idempotency_key.chars().any(char::is_control)
        {
            return Err(ApiError::bad_request("invalid_runtime_repair_request"));
        }
        self.scope_admission(access).await?;
        self.scope_access(id, Some(&access.actor.account_id), true)
            .await?;
        let available: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_runtime_repairs') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if !available {
            return Err(ApiError::conflict("runtime_repair_migration_required"));
        }
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        if access.workspace_permission(required(&row, "account_id")?, &row["request_config"])?
            != Permission::Edit
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        let previous:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_runtime_repairs r WHERE workspace_id=$1 AND idempotency_key=$2").bind(id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await?;
        if let Some(previous) = previous {
            return Ok(public(&previous));
        }
        let config = &row["request_config"];
        if config["rustRuntimeRepairVersion"] != 1
            || config["rustRuntimeSupervisorVersion"] != 1
            || !storage_enrolled(&row)
        {
            return Err(ApiError::conflict("runtime_repair_managed_base_required"));
        }
        if blocked(&row)
            || (row["state"] != "ready" && !preparation::failed_preparation_ingress(&row))
            || row["desired_state"] != "ready"
        {
            return Err(ApiError::conflict(
                "runtime_repair_requires_running_computer",
            ));
        }
        let memory_attention = lifecycle::valid_memory_recovery(&row)
            && config["rustMemoryTransportRecovery"]["operationId"]
                == config["rustLifecycleOperation"]["id"];
        if (config["rustLifecycleOperation"]["phase"]
            .as_str()
            .is_some_and(|phase| !matches!(phase, "ready" | "paused"))
            && !memory_attention)
            || leases_blocked(&row)
            || super::recreation::leases_blocked(&row)
        {
            return Err(ApiError::conflict("runtime_repair_in_progress"));
        }
        lifecycle::require_resume_capacity(self.staging_candidate.as_deref(), &mut tx, &row, now)
            .await?;
        let commands:Vec<String>=sqlx::query_scalar("SELECT command_id FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND ever_leased AND state NOT IN ('applied','rejected','expired','cancelled','outcome-unknown') ORDER BY workspace_sequence LIMIT 129").bind(id).fetch_all(&mut *tx).await?;
        if commands.len() > 128 {
            return Err(ApiError::conflict(
                "runtime_repair_command_inventory_too_large",
            ));
        }
        let operation = uuid::Uuid::new_v4().to_string();
        let pause = uuid::Uuid::new_v4().to_string();
        let resume = uuid::Uuid::new_v4().to_string();
        let source = required(&row, "provider_sandbox_id")?;
        let incarnation = required(&config["rustRuntimeStorage"], "storageIncarnationId")?;
        sqlx::query("INSERT INTO zuse_rust_runtime_repairs(operation_id,workspace_id,generation,source_machine_id,storage_incarnation_id,actor_id,idempotency_key,pause_operation_id,resume_operation_id,phase,command_ids,created_at,updated_at,run_started_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,'quiescing',$10,$11,$11,$11)").bind(&operation).bind(id).bind(generation).bind(source).bind(incarnation).bind(&access.actor.account_id).bind(&input.idempotency_key).bind(&pause).bind(&resume).bind(json!(commands)).bind(now).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'pause','quiescing',$7,$7)").bind(&pause).bind(id).bind(format!("repair-quiesce:{operation}")).bind(generation).bind(source).bind(incarnation).bind(now).execute(&mut *tx).await?;
        let mut config = config.clone();
        if memory_attention {
            config["rustMemoryTransportRecovery"]["repairOperationId"] = json!(operation);
        }
        config["rustLifecycleOperation"] = json!({"id":pause,"action":"pause","phase":"quiescing"});
        config["rustRuntimeRepair"] = json!({"operationId":operation,"pauseOperationId":pause,"resumeOperationId":resume,"phase":"quiescing","updatedAt":now});
        sqlx::query("UPDATE api_cloud_workspaces SET state='pausing',desired_state='paused',status_code='runtime-repair-quiescing',request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(id).bind(config).bind(now).execute(&mut *tx).await?;
        jobs::enqueue(
            &mut tx,
            &format!("runtime-repair:{operation}"),
            JOB_KIND,
            id,
            generation,
            &json!({"operationId":operation}),
            now,
        )
        .await?;
        let op: Value = sqlx::query_scalar(
            "SELECT to_jsonb(r) FROM zuse_rust_runtime_repairs r WHERE operation_id=$1",
        )
        .bind(&operation)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        let _ = self.mailbox_changes.send(());
        Ok(public(&op))
    }
    pub async fn runtime_repair_status(
        &self,
        id: &str,
        operation: &str,
        access: &Access,
    ) -> Result<Value, ApiError> {
        self.scope_access(id, Some(&access.actor.account_id), false)
            .await?;
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace(&mut tx, id).await?;
        access.workspace_permission(required(&row, "account_id")?, &row["request_config"])?;
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_runtime_repairs r WHERE operation_id=$1 AND workspace_id=$2").bind(operation).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("runtime_repair_not_found"))?;
        Ok(public(&op))
    }
    pub async fn retry_runtime_repair(
        &self,
        id: &str,
        operation: &str,
        access: &Access,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.scope_admission(access).await?;
        self.scope_access(id, Some(&access.actor.account_id), true)
            .await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        if access.workspace_permission(required(&row, "account_id")?, &row["request_config"])?
            != Permission::Edit
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_runtime_repairs r WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE").bind(operation).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("runtime_repair_not_found"))?;
        let now = now.max(op["updated_at"].as_i64().unwrap_or(0).saturating_add(1));
        if blocked(&row)
            || op["generation"] != generation
            || row["provider_sandbox_id"] != op["source_machine_id"]
            || row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
                != op["storage_incarnation_id"]
            || row["request_config"]["rustRuntimeRepair"]["operationId"] != operation
        {
            return Err(ApiError::conflict("runtime_repair_fenced"));
        }
        if matches!(op["phase"].as_str(), Some("ready" | "attention-required"))
            || op["error_code"].is_null()
        {
            return Err(ApiError::conflict("runtime_repair_retry_not_required"));
        }
        // Continue the same supervisor operation/phase. Neither old command IDs nor
        // an allocation are submitted again, including after a lost start reply.
        let reset=sqlx::query("UPDATE zuse_rust_jobs SET state='pending',attempts=0,due_at=$3,lease_token=NULL,lease_expires_at=NULL,last_error_code=NULL,updated_at=$3 WHERE kind=$1 AND resource_id=$2 AND payload->>'operationId'=$4 AND (state IN ('failed','pending') OR (state='running' AND lease_expires_at<=$3))").bind(JOB_KIND).bind(id).bind(now).bind(operation).execute(&mut *tx).await?;
        if reset.rows_affected() != 1 {
            return Err(ApiError::conflict("runtime_repair_worker_active"));
        }
        sqlx::query("UPDATE zuse_rust_runtime_repairs SET error_code=NULL,run_started_at=$2,updated_at=GREATEST(updated_at+1,$2) WHERE operation_id=$1").bind(operation).bind(now).execute(&mut *tx).await?;
        let mut config = row["request_config"].clone();
        config["rustRuntimeRepair"]["errorCode"] = Value::Null;
        config["rustRuntimeRepair"]["updatedAt"] = json!(now);
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(id).bind(config).bind(now).execute(&mut *tx).await?;
        let result: Value = sqlx::query_scalar(
            "SELECT to_jsonb(r) FROM zuse_rust_runtime_repairs r WHERE operation_id=$1",
        )
        .bind(operation)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(public(&result))
    }
    pub async fn resolve_runtime_repair(
        &self,
        id: &str,
        operation: &str,
        access: &Access,
        input: Resolve,
        now: i64,
    ) -> Result<Value, ApiError> {
        if !matches!(input.decision.as_str(), "continue" | "discard") {
            return Err(ApiError::bad_request("runtime_repair_resolution_required"));
        }
        self.scope_admission(access).await?;
        self.scope_access(id, Some(&access.actor.account_id), true)
            .await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        if access.workspace_permission(required(&row, "account_id")?, &row["request_config"])?
            != Permission::Edit
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_runtime_repairs r WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE").bind(operation).bind(id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("runtime_repair_not_found"))?;
        let now = now.max(op["updated_at"].as_i64().unwrap_or(0).saturating_add(1));
        if op["phase"] == "ready" {
            if op["resolution"] == input.decision {
                return Ok(public(&op));
            }
            return Err(ApiError::conflict("runtime_repair_resolution_changed"));
        }
        if op["phase"] != "attention-required"
            || op["generation"] != generation
            || row["request_config"]["rustRuntimeRepair"]["operationId"] != operation
        {
            return Err(ApiError::conflict(
                "runtime_repair_not_ready_for_resolution",
            ));
        }
        if input.decision == "discard" {
            let pending:Vec<String>=sqlx::query_scalar("SELECT command_id FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND NOT ever_leased AND state IN ('accepted','waiting-for-runtime','blocked')").bind(id).fetch_all(&mut *tx).await?;
            for cid in pending {
                super::mailbox::change(&mut tx,id,&cid,"state='cancelled',category='runtime-repair-user-discarded',lease_token=NULL,lease_deadline=NULL",now).await?;
            }
        }
        sqlx::query("UPDATE zuse_rust_runtime_repairs SET phase='ready',resolution=$3,updated_at=$4 WHERE operation_id=$1 AND workspace_id=$2").bind(operation).bind(id).bind(&input.decision).bind(now).execute(&mut *tx).await?;
        let mut config = row["request_config"].clone();
        config["rustRuntimeRepair"]["phase"] = json!("ready");
        config["rustRuntimeRepair"]["updatedAt"] = json!(now);
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(id).bind(config).bind(now).execute(&mut *tx).await?;
        let op: Value = sqlx::query_scalar(
            "SELECT to_jsonb(r) FROM zuse_rust_runtime_repairs r WHERE operation_id=$1",
        )
        .bind(operation)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        let _ = self.mailbox_changes.send(());
        Ok(public(&op))
    }
}
