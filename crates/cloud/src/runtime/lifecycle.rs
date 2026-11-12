//! Durable retained-machine lifecycle. No expiry path can destroy a machine.
use super::*;
use crate::providers::contract::{DesiredState, OperationState, ProviderOperation};
use crate::{
    access::Access,
    jobs,
    providers::{Machine, MachineState, ProviderError, boxd::Boxd},
};
use async_trait::async_trait;
use zuse_protocol::cloud_lifecycle::{Action, CAPABILITY, Directive, RuntimeRequest};
mod admission_intent;
mod authority_connection;
mod idle;
mod locking;
pub(super) mod memory_authority;
mod memory_recovery;
pub mod scheduling;
pub mod wakeup;
pub(super) use memory_recovery::{memory_recovery_for, valid_memory_recovery};
mod task_wake;
mod worker;

pub const MIGRATION: &str = include_str!("../../migrations/0005_lifecycle.sql");
pub const JOB_KIND: &str = "workspace-lifecycle";
#[async_trait]
pub trait RetainedProvider: Send + Sync {
    async fn retained_launch_ready(&self, _id: &str) -> Result<bool, ProviderError> {
        Ok(true)
    }
    /// Separate, opt-in owned start after hydration; never repairs an old image.
    async fn bootstrap_retained_runtime(
        &self,
        _request: &crate::managed_bases::bootstrap::Request,
    ) -> Result<bool, ProviderError> {
        Err(ProviderError::Rejected)
    }
    fn provider_id(&self) -> &'static str {
        "boxd"
    }
    async fn inspect_retained(&self, id: &str) -> Result<Option<Machine>, ProviderError>;
    async fn pause_retained(&self, id: &str) -> Result<(), ProviderError>;
    async fn wake_retained(&self, id: &str, state: &MachineState) -> Result<(), ProviderError>;
    async fn sleep_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.pause_retained(id).await?;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Sleeping,
        })
    }
    async fn memory_sleep_receipt(&self, _id: &str) -> Result<ProviderOperation, ProviderError> {
        Err(ProviderError::Rejected)
    }
    async fn memory_receipt_state(
        &self,
        _receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        Err(ProviderError::Rejected)
    }
    async fn wake_receipt(
        &self,
        id: &str,
        state: &MachineState,
    ) -> Result<ProviderOperation, ProviderError> {
        self.wake_retained(id, state).await?;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Running,
        })
    }
    async fn receipt_state(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        if receipt.operation_id.is_some() {
            return Err(ProviderError::InvalidResponse);
        }
        let machine = self.inspect_retained(&receipt.resource_id).await?;
        let done = match (receipt.desired, machine.as_ref().map(|m| &m.state)) {
            (
                DesiredState::Sleeping,
                Some(MachineState::Standby | MachineState::Paused | MachineState::Stopped),
            ) => true,
            (DesiredState::Running, Some(MachineState::Running)) => true,
            _ => false,
        };
        Ok(if done {
            OperationState::Completed
        } else {
            OperationState::Pending
        })
    }
    async fn launch_retained(
        &self,
        directive: &Directive,
        workspace: &str,
        api: &str,
    ) -> Result<(), ProviderError>;
}
#[async_trait]
impl RetainedProvider for Boxd {
    async fn inspect_retained(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.inspect(id).await
    }
    async fn pause_retained(&self, id: &str) -> Result<(), ProviderError> {
        self.hibernate(id).await
    }
    async fn wake_retained(&self, id: &str, state: &MachineState) -> Result<(), ProviderError> {
        self.activate(id, state).await
    }
    async fn memory_sleep_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        crate::providers::contract::Provider::sleep_memory(self, id).await
    }
    async fn memory_receipt_state(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        crate::providers::contract::Provider::memory_operation(self, receipt).await
    }
    async fn launch_retained(
        &self,
        d: &Directive,
        workspace: &str,
        api: &str,
    ) -> Result<(), ProviderError> {
        fn q(v: &str) -> String {
            format!("'{}'", v.replace('\'', "'\\''"))
        }
        let command = format!(
            "sudo -n -u zuse env ZUSE_RUNTIME_RETAINED=true ZUSE_CLOUD_WORKSPACE_ID={} ZUSE_API_URL={} ZUSE_RUNTIME_GENERATION={} ZUSE_STORAGE_INCARNATION_ID={} ZUSE_RUNTIME_RESUME_OPERATION_ID={} /usr/local/lib/zuse/launch-rust-runtime",
            q(workspace),
            q(api),
            d.generation,
            q(&d.storage_incarnation_id),
            q(&d.operation_id)
        );
        if self.exec(&d.source_machine_id, &command).await?.exit_code != 0 {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
}
fn provider_error(_: ProviderError) -> ApiError {
    ApiError::unavailable("cloud_retained_provider_failed")
}
pub(super) fn deleted(row: &Value) -> bool {
    row["desired_state"] == "deleted"
        || matches!(row["state"].as_str(), Some("deleting" | "deleted"))
        || row["request_config"]["cloudMailboxLifecyclePending"]["action"] == "delete"
        || row["request_config"]["cloudMailboxLifecycleDelivered"]["action"] == "delete"
}
fn directive(op: &Value) -> Result<Directive, ApiError> {
    Ok(Directive {
        full_shutdown: op["provider_effects"]["recreationFullShutdown"] == true,
        operation_id: required(op, "operation_id")?.into(),
        action: serde_json::from_value(op["action"].clone())
            .map_err(|_| ApiError::conflict("cloud_lifecycle_metadata_invalid"))?,
        phase: required(op, "phase")?.into(),
        generation: op["generation"].as_i64().ok_or_else(rejected)?,
        storage_incarnation_id: required(op, "storage_incarnation_id")?.into(),
        source_machine_id: required(op, "source_machine_id")?.into(),
    })
}
async fn current(tx: &mut Transaction<'_, Postgres>, row: &Value) -> Result<Value, ApiError> {
    let op:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_lifecycle_operations o WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE")
        .bind(row["request_config"]["rustLifecycleOperation"]["id"].as_str()).bind(required(row,"workspace_id")?).fetch_optional(&mut **tx).await?;
    let op = op.ok_or_else(|| ApiError::conflict("cloud_lifecycle_operation_missing"))?;
    validate_current(row, &op)?;
    Ok(op)
}
fn validate_current(row: &Value, op: &Value) -> Result<(), ApiError> {
    if op["generation"] != row["request_config"]["runtimeGeneration"]
        || op["source_machine_id"] != row["provider_sandbox_id"]
        || op["storage_incarnation_id"]
            != row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
        || deleted(row)
    {
        return Err(ApiError::conflict("cloud_lifecycle_fenced"));
    }
    Ok(())
}
fn progress(config: &mut Value, id: &str, action: Action, phase: &str) {
    config["rustLifecycleOperation"] = json!({"id":id,"action":action,"phase":phase});
}
async fn phase(
    tx: &mut Transaction<'_, Postgres>,
    row: &Value,
    op: &Value,
    next: &str,
    now: i64,
) -> Result<(), ApiError> {
    let d = directive(op)?;
    let mut config = row["request_config"].clone();
    progress(&mut config, &d.operation_id, d.action, next);
    sqlx::query(
        "UPDATE zuse_rust_lifecycle_operations SET phase=$2,updated_at=$3,provider_effects=jsonb_set(provider_effects,'{timings}',jsonb_build_object($2::text,$3::bigint)||COALESCE(provider_effects->'timings','{}'::jsonb)) WHERE operation_id=$1",
    )
    .bind(&d.operation_id)
    .bind(next)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1").bind(required(row,"workspace_id")?).bind(config).bind(now).execute(&mut **tx).await?;
    Ok(())
}
impl RuntimeControl {
    pub async fn lifecycle_ready(&self) -> Result<(), ApiError> {
        let ready: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_lifecycle_operations') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if !ready {
            return Err(ApiError::unavailable("cloud_lifecycle_schema_not_ready"));
        }
        Ok(())
    }
    pub async fn lifecycle_action(
        &self,
        id: &str,
        access: &Access,
        action: Action,
        command: Option<&str>,
        now: i64,
    ) -> Result<Value, ApiError> {
        if command.is_some_and(|s| s.is_empty() || s.len() > 512 || s.chars().any(char::is_control))
        {
            return Err(ApiError::bad_request("invalid_lifecycle_command_id"));
        }
        let command = command
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        self.scope_admission(access).await?;
        self.scope_access(id, Some(&access.actor.account_id), true)
            .await?;
        let mut tx = self.pool.begin().await?;
        // Restore admission advances ownership; take its exclusive lock before the workspace row.
        let (row, generation, _) = locking::workspace(&mut tx, id, true).await?;
        if row["account_id"] != access.owner_id {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        if access.workspace_permission(&access.owner_id, &row["request_config"])?
            != crate::access::Permission::Edit
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        if super::recreation::leases_blocked(&row) {
            return Err(ApiError::conflict("computer_recreation_in_progress"));
        }
        if self
            .recovery_action(&mut tx, &row, action, &command, now)
            .await?
        {
            let (row, _, _) = locked_workspace(&mut tx, id).await?;
            tx.commit().await?;
            return Ok(crate::store::Workspace { row }.public());
        }
        let prior:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_lifecycle_operations o WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(&command).fetch_optional(&mut *tx).await?;
        if let Some(prior) = prior {
            if prior["action"] != action.as_str() {
                return Err(ApiError::conflict("cloud_lifecycle_command_id_reused"));
            }
            if !deleted(&row)
                && row["request_config"]["rustLifecycleOperation"]["id"] == prior["operation_id"]
                && prior["generation"] == generation
            {
                sqlx::query("UPDATE zuse_rust_jobs SET state='pending',attempts=0,due_at=$2,last_error_code=NULL,updated_at=$2 WHERE kind=$3 AND resource_id=$1 AND payload->>'operationId'=$4 AND state='failed'")
                    .bind(id).bind(now).bind(JOB_KIND).bind(prior["operation_id"].as_str()).execute(&mut *tx).await?;
            }
            tx.commit().await?;
            return Ok(crate::store::Workspace { row }.public());
        }
        let config = &row["request_config"];
        if deleted(&row)
            || (row["provider"] != "boxd"
                && !(config["rustProviderControllerVersion"] == 1
                    && zuse_protocol::cloud_computers::ProviderId::ALL
                        .iter()
                        .any(|p| row["provider"] == p.as_str())))
            || !storage_enrolled(&row)
            || !config["rustStorageEnrollment"]["capabilities"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == CAPABILITY))
        {
            return Err(ApiError::conflict(
                "cloud_lifecycle_runtime_update_required",
            ));
        }
        if action == Action::Resume
            && row["desired_state"] == "ready"
            && (row["state"] == "ready"
                || (config["rustLifecycleOperation"]["action"] == "resume"
                    && matches!(row["state"].as_str(), Some("queued" | "provisioning"))))
        {
            require_resume_capacity(self.staging_candidate.as_deref(), &mut tx, &row, now).await?;
            return Ok(crate::store::Workspace { row }.public());
        }
        if config["rustNativeTranscriptEnabled"] == true {
            if self.transcript_objects.is_none() {
                return Err(ApiError::unavailable("cloud_transcript_store_unavailable"));
            }
            if !config["rustStorageEnrollment"]["capabilities"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "rust-transcript-projection-v1"))
            {
                return Err(ApiError::conflict(
                    "cloud_lifecycle_runtime_update_required",
                ));
            }
        }
        let pending = config["rustLifecycleOperation"]["phase"]
            .as_str()
            .is_some_and(|p| !matches!(p, "paused" | "ready"));
        if pending {
            return Err(ApiError::conflict("cloud_lifecycle_in_progress"));
        }
        let asleep = row["state"] == "paused" || row["state"] == "archived";
        let (state, desired, start) = match action {
            Action::Unarchive if row["state"] == "archived" => ("paused", "paused", "paused"),
            Action::Archive if asleep => ("archived", "archived", "paused"),
            Action::Archive if row["state"] == "ready" => ("archiving", "archived", "quiescing"),
            Action::Pause if row["state"] == "paused" => ("paused", "paused", "paused"),
            Action::Pause if row["state"] == "ready" => ("pausing", "paused", "quiescing"),
            Action::Resume if row["state"] == "paused" => ("provisioning", "ready", "waking"),
            _ => return Err(ApiError::conflict("cloud_lifecycle_transition_unavailable")),
        };
        if asleep {
            let previous = current(&mut tx, &row).await?;
            if previous["phase"] != "paused" {
                return Err(ApiError::conflict("cloud_lifecycle_quiescence_required"));
            }
        }
        if action == Action::Resume {
            require_resume_capacity(self.staging_candidate.as_deref(), &mut tx, &row, now).await?;
        }
        let operation = uuid::Uuid::new_v4().to_string();
        let mut config = config.clone();
        progress(&mut config, &operation, action, start);
        let requested = if action == Action::Archive {
            Some(now)
        } else {
            row["archive_requested_at"]
                .as_i64()
                .filter(|_| action != Action::Unarchive)
        };
        let deadline = if action == Action::Archive {
            Some(now.saturating_add(zuse_protocol::cloud::ARCHIVE_RETENTION_MS))
        } else {
            row["archive_delete_at"]
                .as_i64()
                .filter(|_| action != Action::Unarchive)
        };
        let row = if action == Action::Resume && !access.owner_id.starts_with("organization:") {
            admission_intent::persist(
                &mut tx,
                admission_intent::Intent {
                    operation: &operation,
                    command: &command,
                    row: &row,
                    generation,
                    action,
                    phase: start,
                    state,
                    desired,
                    config,
                    requested,
                    deadline,
                    now,
                },
            )
            .await?
        } else {
            sqlx::query("INSERT INTO zuse_rust_lifecycle_operations (operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9)")
            .bind(&operation).bind(id).bind(&command).bind(generation).bind(required(&row,"provider_sandbox_id")?).bind(required(&config["rustRuntimeStorage"],"storageIncarnationId")?).bind(action.as_str()).bind(start).bind(now).execute(&mut *tx).await?;
            sqlx::query("UPDATE api_cloud_workspaces SET state=$2,desired_state=$3,status_code=$4,request_config=$5,archive_requested_at=$6,archive_delete_at=$7,revision=revision+1,updated_at=GREATEST(updated_at+1,$8),next_action_at=$8 WHERE workspace_id=$1")
            .bind(id).bind(state).bind(desired).bind(format!("{}-{start}",action.as_str())).bind(config).bind(requested).bind(deadline).bind(now).execute(&mut *tx).await?;
            if start != "paused" {
                let job = jobs::enqueue(
                    &mut tx,
                    &format!("lifecycle:{operation}"),
                    JOB_KIND,
                    id,
                    generation,
                    &json!({"operationId":operation}),
                    now,
                )
                .await?;
                sqlx::query("UPDATE zuse_rust_jobs SET max_attempts=120 WHERE job_id=$1")
                    .bind(job)
                    .execute(&mut *tx)
                    .await?;
            }
            let (row, _, _) = locked_workspace(&mut tx, id).await?;
            row
        };
        tx.commit().await?;
        wakeup::send();
        Ok(crate::store::Workspace { row }.public())
    }
    /// May authenticate a fenced guest only for shutdown receipt delivery, never execution.
    pub async fn lifecycle_runtime(
        &self,
        id: &str,
        credential: &str,
        request: RuntimeRequest,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.lifecycle_runtime_pilot(
            id,
            credential,
            request,
            now,
            env::var("ZUSE_DEPLOYMENT_ENVIRONMENT").ok().as_deref(),
            env::var("ZUSE_CLOUD_INTERNAL_CODEX_MEMORY_TRANSPORT_OWNER")
                .ok()
                .as_deref(),
        )
        .await
    }
    pub(in crate::runtime) async fn lifecycle_runtime_pilot(
        &self,
        id: &str,
        credential: &str,
        request: RuntimeRequest,
        now: i64,
        environment: Option<&str>,
        pilot_owner: Option<&str>,
    ) -> Result<Value, ApiError> {
        // Candidate validation commits before workspace locks, while the
        // personal RAM path retains one checkout across those transactions.
        let (early, mut connection) = self.runtime_authority_connection(id, &request).await?;
        let shared = connection.is_some();
        let candidate_started = std::time::Instant::now();
        let pilot_allowed = if matches!(
            &request,
            RuntimeRequest::Query | RuntimeRequest::Resumed { .. }
        ) {
            self.internal_codex_tool_observations_on(
                &early,
                environment,
                pilot_owner,
                now,
                connection.as_mut().map(|c| &mut **c),
            )
            .await
        } else {
            false
        };
        let candidate_finished = std::time::Instant::now();
        let mut tx = match connection.as_mut() {
            Some(connection) => sqlx::Connection::begin(&mut **connection).await?,
            None => self.pool.begin().await?,
        };
        let transaction_started = std::time::Instant::now();
        let (row, loaded_operation) = if shared {
            locking::workspace_operation(&mut tx, id).await?
        } else {
            let (row, _, _) = locking::workspace(&mut tx, id, false).await?;
            (row, None)
        };
        // A request timestamp cannot extend authority while it waits on a lock.
        let authority_now = if shared { now.max(clock_ms()) } else { now };
        let pilot_allowed = pilot_allowed
            && (!shared
                || self
                    .staging_candidate
                    .as_ref()
                    .is_some_and(|pin| pin.expires_at > authority_now));
        if deleted(&row)
            || !storage_enrolled(&row)
            || row["runtime_credential_hash"]
                .as_str()
                .is_none_or(|h| !equal(h, &hash(credential)))
            || row["request_config"]["runtimeCredentialExpiresAtMs"]
                .as_i64()
                .unwrap_or(0)
                <= authority_now
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if shared {
            tracing::debug!(
                target: "zuse_cloud::resume_timing",
                candidate_check_us = candidate_finished.duration_since(candidate_started).as_micros() as u64,
                transaction_begin_us = transaction_started.duration_since(candidate_finished).as_micros() as u64,
                workspace_lock_read_us = transaction_started.elapsed().as_micros() as u64,
                "memory_resume_authority_fence"
            );
        }
        if row["request_config"]["rustLifecycleOperation"].is_null() {
            if matches!(&request, RuntimeRequest::RecoveryRequired { .. }) {
                return Err(ApiError::conflict("cloud_memory_recovery_receipt_rejected"));
            }
            return Ok(json!({"directive":null}));
        }
        let op = match loaded_operation {
            Some(operation) => {
                validate_current(&row, &operation)?;
                operation
            }
            None => current(&mut tx, &row).await?,
        };
        let d = directive(&op)?;
        if let RuntimeRequest::RecoveryRequired { operation_id, code } = &request {
            let result = self
                .memory_recovery_report_tx(&mut tx, &row, &op, &d, operation_id, code, now)
                .await?;
            tx.commit().await?;
            let _ = self.mailbox_changes.send(());
            return Ok(result);
        }
        if let Some(recovery) = memory_recovery_for(&row, &op) {
            if !matches!(&request, RuntimeRequest::Query) {
                return Err(ApiError::conflict("cloud_memory_recovery_receipt_rejected"));
            }
            tx.commit().await?;
            return Ok(json!({"directive":null,"recoveryRequired":recovery}));
        }
        if matches!(
            &request,
            RuntimeRequest::Query | RuntimeRequest::Resumed { .. }
        ) {
            if let Some(result) = self
                .memory_resume_authority_tx(&mut tx, &row, &op, &d, &early, pilot_allowed, now)
                .await?
            {
                tx.commit().await?;
                let _ = self.mailbox_changes.send(());
                return Ok(result);
            }
        }
        let acknowledged = !matches!(&request, RuntimeRequest::Query);
        let completed_on_ack = matches!(&request, RuntimeRequest::Resumed { .. });
        match request {
            RuntimeRequest::RecoveryRequired { .. } => {
                unreachable!("handled before receipt mutation")
            }
            RuntimeRequest::Query => {}
            RuntimeRequest::Quiesced { operation_id } => {
                if operation_id != d.operation_id
                    || !matches!(d.action, Action::Pause | Action::Archive)
                    || !matches!(
                        d.phase.as_str(),
                        "quiescing" | "quiesced" | "pausing" | "paused"
                    )
                {
                    return Err(ApiError::conflict("cloud_lifecycle_receipt_rejected"));
                }
                if d.phase != "quiescing" {
                    return Ok(json!({"directive":d}));
                }
                phase(&mut tx, &row, &op, "quiesced", now).await?;
                sqlx::query("UPDATE api_cloud_workspaces SET runtime_state='offline',status_code='runtime-quiesced' WHERE workspace_id=$1").bind(id).execute(&mut *tx).await?;
            }
            RuntimeRequest::Parked { operation_id } => {
                let config = &row["request_config"];
                if config["rustRuntimeRepair"]["pauseOperationId"] == operation_id
                    && super::repair::leases_blocked(&row)
                {
                    return Err(ApiError::conflict(
                        "cloud_runtime_repair_full_shutdown_required",
                    ));
                }
                if operation_id != d.operation_id
                    || d.action != Action::Pause
                    || d.full_shutdown
                    || !matches!(
                        d.phase.as_str(),
                        "quiescing" | "quiesced" | "pausing" | "paused"
                    )
                    || config["rustMemorySleepVersion"] != 1
                    || !matches!(row["provider"].as_str(), Some("boxd" | "e2b"))
                    || !config["rustStorageEnrollment"]["capabilities"]
                        .as_array()
                        .is_some_and(|c| {
                            c.iter()
                                .any(|v| v == zuse_protocol::cloud_lifecycle::MEMORY_CAPABILITY)
                        })
                {
                    return Err(ApiError::conflict("cloud_memory_park_receipt_rejected"));
                }
                let marker = json!({"version":1,"operationId":d.operation_id,"sourceMachineId":d.source_machine_id,"generation":d.generation,"storageIncarnationId":d.storage_incarnation_id});
                if d.phase != "quiescing" {
                    if config["rustMemoryParked"] != marker {
                        return Err(ApiError::conflict("cloud_memory_park_receipt_rejected"));
                    }
                    return Ok(json!({"directive":d}));
                }
                phase(&mut tx, &row, &op, "quiesced", now).await?;
                sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustMemoryParked}',$2),runtime_state='offline',status_code='runtime-memory-parked' WHERE workspace_id=$1")
                    .bind(id).bind(marker).execute(&mut *tx).await?;
            }
            RuntimeRequest::Resumed { operation_id } => {
                if operation_id != d.operation_id
                    || d.action != Action::Resume
                    || !matches!(d.phase.as_str(), "waking" | "launching" | "ready")
                    || row["desired_state"] != "ready"
                {
                    return Err(ApiError::conflict("cloud_lifecycle_receipt_rejected"));
                }
                // This authenticated Ready receipt is stronger than a worker
                // observing it two seconds later. Complete ONLY its matching
                // durable job atomically, fencing any late worker delivery.
                sqlx::query("WITH operation AS (UPDATE zuse_rust_lifecycle_operations SET phase='ready',updated_at=GREATEST(updated_at,$3),provider_effects=jsonb_set(provider_effects,'{timings}',jsonb_build_object('ready',$3::bigint)||COALESCE(provider_effects->'timings','{}'::jsonb)) WHERE operation_id=$2 AND workspace_id=$1 AND generation=$4 AND source_machine_id=$5 AND storage_incarnation_id=$6 RETURNING operation_id), workspace AS (UPDATE api_cloud_workspaces SET state='ready',runtime_state='online',status_code='ready',request_config=jsonb_set(request_config-'rustMemoryParked','{rustLifecycleOperation}',$7),next_action_at=$8,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1 AND EXISTS(SELECT 1 FROM operation) RETURNING workspace_id) UPDATE zuse_rust_jobs SET state='completed',lease_token=NULL,lease_expires_at=NULL,last_error_code=NULL,updated_at=GREATEST(updated_at,$3) WHERE resource_id=$1 AND resource_generation=$4 AND kind=$9 AND payload->>'operationId'=$2 AND state IN ('pending','running') AND EXISTS(SELECT 1 FROM workspace)")
                    .bind(id).bind(&d.operation_id).bind(now).bind(d.generation).bind(&d.source_machine_id).bind(&d.storage_incarnation_id).bind(json!({"id":d.operation_id,"action":d.action,"phase":"ready"})).bind(now+self.idle_ms).bind(JOB_KIND).execute(&mut *tx).await?;
            }
        }
        if acknowledged && !completed_on_ack {
            // Nonterminal ACKs never steal or extend a running lease. If its current delivery
            // races this ACK, lifecycle_save observes the changed phase.
            sqlx::query("UPDATE zuse_rust_jobs SET due_at=LEAST(due_at,$4) WHERE resource_id=$1 AND resource_generation=$2 AND kind=$3 AND payload->>'operationId'=$5 AND state='pending'")
                .bind(id).bind(d.generation).bind(JOB_KIND).bind(now).bind(&d.operation_id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        if completed_on_ack {
            crate::events::hint::send();
        } else if acknowledged {
            wakeup::send();
        }
        Ok(json!({"directive":d}))
    }
    pub async fn lifecycle_once(
        &self,
        provider: &dyn RetainedProvider,
        now: i64,
    ) -> Result<bool, ApiError> {
        let Some(job) = jobs::claim(&self.pool, JOB_KIND, "workspace", now, 120_000).await? else {
            return Ok(false);
        };
        self.lifecycle_claimed(provider, &job, now).await?;
        Ok(true)
    }
    pub async fn lifecycle_registry_once(
        &self,
        registry: &crate::providers::registry::Registry,
        now: i64,
    ) -> Result<bool, ApiError> {
        let Some(job) = jobs::claim(&self.pool, JOB_KIND, "workspace", now, 120000).await? else {
            return Ok(false);
        };
        let adapter = self.provider_for_job(registry, &job).await?;
        self.lifecycle_claimed(&adapter, &job, now).await?;
        Ok(true)
    }
    async fn lifecycle_claimed(
        &self,
        provider: &dyn RetainedProvider,
        job: &jobs::Job,
        now: i64,
    ) -> Result<(), ApiError> {
        let result = self.lifecycle_step(provider, job, now).await;
        if let Err(error) = &result {
            // A computer its provider no longer has will not come back. Count
            // consecutive "missing" answers (claims and other outcomes don't
            // count) and stop after a few, leaving the workspace settled so
            // the user can recreate it instead of watching "Starting computer".
            let missing = if error.code == MACHINE_MISSING {
                sqlx::query_scalar::<_, i32>("UPDATE zuse_rust_jobs SET payload=jsonb_set(payload,'{missingObservations}',to_jsonb(COALESCE((payload->>'missingObservations')::int,0)+1)) WHERE job_id=$1 AND lease_token=$2 AND state='running' RETURNING (payload->>'missingObservations')::int")
                    .bind(&job.id).bind(&job.lease_token).fetch_optional(&self.pool).await?.unwrap_or(0)
            } else {
                self.clear_missing_observations(job).await?;
                0
            };
            if missing >= MISSING_CONFIRMATIONS {
                sqlx::query("UPDATE zuse_rust_jobs SET state='failed',last_error_code=$3,lease_token=NULL,lease_expires_at=NULL,updated_at=$4 WHERE job_id=$1 AND lease_token=$2 AND state='running'")
                    .bind(&job.id).bind(&job.lease_token).bind(error.code).bind(clock_ms()).execute(&self.pool).await?;
                sqlx::query("UPDATE api_cloud_workspaces SET state='paused',desired_state='paused',runtime_state='offline',status_code=$3,request_config=jsonb_set(request_config,'{rustLifecycleOperation,phase}','\"paused\"'),revision=revision+1,updated_at=GREATEST(updated_at+1,$4) WHERE workspace_id=$1 AND request_config->'rustLifecycleOperation'->>'id'=$2 AND desired_state<>'deleted' AND state NOT IN ('deleting','deleted')")
                    .bind(&job.resource_id).bind(job.payload["operationId"].as_str()).bind(error.code).bind(clock_ms()).execute(&self.pool).await?;
                tracing::warn!(workspace = %job.resource_id, missing,
                    "Retained computer missing at its provider; lifecycle stopped");
                return result;
            }
            jobs::retry(&self.pool, &job, clock_ms(), error.code).await?;
            sqlx::query("UPDATE api_cloud_workspaces w SET status_code=$3,revision=revision+1,updated_at=GREATEST(updated_at+1,(extract(epoch from clock_timestamp())*1000)::bigint) WHERE workspace_id=$1 AND request_config->'rustLifecycleOperation'->>'id'=$2 AND desired_state<>'deleted' AND state NOT IN ('deleting','deleted') AND request_config->'rustLifecycleOperation'->>'phase' NOT IN ('paused','ready') AND request_config->'rustMemoryTransportRecovery'->>'operationId' IS DISTINCT FROM $2 AND EXISTS(SELECT 1 FROM zuse_rust_control_owners o WHERE o.resource_kind='workspace' AND o.resource_id=w.workspace_id AND o.backend='rust' AND o.generation=$4)").bind(&job.resource_id).bind(job.payload["operationId"].as_str()).bind(error.code).bind(job.generation).execute(&self.pool).await?;
        } else if job.payload.get("missingObservations").is_some() {
            self.clear_missing_observations(job).await?;
        }
        result
    }
    /// Only consecutive "missing" answers count toward giving up.
    async fn clear_missing_observations(&self, job: &jobs::Job) -> Result<(), ApiError> {
        sqlx::query("UPDATE zuse_rust_jobs SET payload=payload-'missingObservations' WHERE job_id=$1 AND payload ? 'missingObservations'")
            .bind(&job.id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
/// The provider no longer has the retained computer (in any cluster).
pub(crate) const MACHINE_MISSING: &str = "cloud_retained_machine_missing";
/// Consecutive "missing" observations before giving up.
const MISSING_CONFIRMATIONS: i32 = 3;

pub(super) async fn require_resume_capacity(
    pin: Option<&crate::managed_bases::candidate::StagingBasePin>,
    tx: &mut Transaction<'_, Postgres>,
    row: &Value,
    now: i64,
) -> Result<(), ApiError> {
    if crate::managed_bases::candidate::compute_allowance(tx, pin, row, now).await? {
        return Ok(());
    }
    let account = required(row, "account_id")?;
    let entitled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND (status IN ('active','grace') OR (status='ended' AND paid_through>$2)) AND (paid_through IS NULL OR paid_through>$2))").bind(account).bind(now).fetch_one(&mut **tx).await?;
    if !entitled {
        return Err(ApiError::forbidden("cloud_entitlement_required"));
    }
    crate::billing::require_capacity(tx, account, now).await
}
fn clock_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
