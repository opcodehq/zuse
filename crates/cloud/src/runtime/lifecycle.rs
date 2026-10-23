//! Durable retained-machine lifecycle. No expiry path can destroy a machine.
use super::*;
use crate::{
    access::Access,
    jobs,
    providers::{Machine, MachineState, ProviderError, boxd::Boxd},
};
use async_trait::async_trait;
use zuse_protocol::cloud_lifecycle::{Action, CAPABILITY, Directive, RuntimeRequest};

pub const MIGRATION: &str = include_str!("../../migrations/0005_lifecycle.sql");
pub const JOB_KIND: &str = "workspace-lifecycle";
#[async_trait]
pub trait RetainedProvider: Send + Sync {
    async fn inspect_retained(&self, id: &str) -> Result<Option<Machine>, ProviderError>;
    async fn pause_retained(&self, id: &str) -> Result<(), ProviderError>;
    async fn wake_retained(&self, id: &str, state: &MachineState) -> Result<(), ProviderError>;
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
        match state {
            MachineState::Paused => self.wake(id).await,
            MachineState::Stopped => self.start(id).await,
            _ => Err(ProviderError::Conflict),
        }
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
            "sudo -n -u zuse env ZUSE_RUNTIME_RETAINED=true ZUSE_CLOUD_WORKSPACE_ID={} ZUSE_API_URL={} ZUSE_RUNTIME_GENERATION={} ZUSE_STORAGE_INCARNATION_ID={} /usr/local/lib/zuse/launch-rust-runtime",
            q(workspace),
            q(api),
            d.generation,
            q(&d.storage_incarnation_id)
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
    if op["generation"] != row["request_config"]["runtimeGeneration"]
        || op["source_machine_id"] != row["provider_sandbox_id"]
        || op["storage_incarnation_id"]
            != row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
        || deleted(row)
    {
        return Err(ApiError::conflict("cloud_lifecycle_fenced"));
    }
    Ok(op)
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
        "UPDATE zuse_rust_lifecycle_operations SET phase=$2,updated_at=$3 WHERE operation_id=$1",
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
        let mut tx = self.pool.begin().await?;
        // Restore admission advances ownership; take its exclusive lock before the workspace row.
        sqlx::query("SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        if row["account_id"] != access.owner_id {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        if access.membership.is_some() || access.owner_id != access.actor.account_id {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        if access.workspace_permission(&access.owner_id, &row["request_config"])?
            != crate::access::Permission::Edit
        {
            return Err(ApiError::forbidden("workspace_access_denied"));
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
            || row["provider"] != "boxd"
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
            require_resume_capacity(&mut tx, &row, now).await?;
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
            require_resume_capacity(&mut tx, &row, now).await?;
        }
        let operation = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations (operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9)")
            .bind(&operation).bind(id).bind(command).bind(generation).bind(required(&row,"provider_sandbox_id")?).bind(required(&config["rustRuntimeStorage"],"storageIncarnationId")?).bind(action.as_str()).bind(start).bind(now).execute(&mut *tx).await?;
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
        tx.commit().await?;
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
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace(&mut tx, id).await?;
        if deleted(&row)
            || !storage_enrolled(&row)
            || row["runtime_credential_hash"]
                .as_str()
                .is_none_or(|h| !equal(h, &hash(credential)))
            || row["request_config"]["runtimeCredentialExpiresAtMs"]
                .as_i64()
                .unwrap_or(0)
                <= now
        {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if row["request_config"]["rustLifecycleOperation"].is_null() {
            return Ok(json!({"directive":null}));
        }
        let op = current(&mut tx, &row).await?;
        let d = directive(&op)?;
        match request {
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
            RuntimeRequest::Resumed { operation_id } => {
                if operation_id != d.operation_id
                    || d.action != Action::Resume
                    || !matches!(d.phase.as_str(), "waking" | "launching" | "ready")
                    || row["desired_state"] != "ready"
                {
                    return Err(ApiError::conflict("cloud_lifecycle_receipt_rejected"));
                }
                phase(&mut tx, &row, &op, "ready", now).await?;
                sqlx::query("UPDATE api_cloud_workspaces SET state='ready',runtime_state='online',status_code='ready',next_action_at=$2,revision=revision+1 WHERE workspace_id=$1").bind(id).bind(now+self.idle_ms).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
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
        let result = self.lifecycle_step(provider, &job, now).await;
        if let Err(error) = &result {
            jobs::retry(&self.pool, &job, clock_ms(), error.code).await?;
            sqlx::query("UPDATE api_cloud_workspaces w SET status_code=$3,revision=revision+1,updated_at=GREATEST(updated_at+1,(extract(epoch from clock_timestamp())*1000)::bigint) WHERE workspace_id=$1 AND request_config->'rustLifecycleOperation'->>'id'=$2 AND desired_state<>'deleted' AND state NOT IN ('deleting','deleted') AND request_config->'rustLifecycleOperation'->>'phase' NOT IN ('paused','ready') AND EXISTS(SELECT 1 FROM zuse_rust_control_owners o WHERE o.resource_kind='workspace' AND o.resource_id=w.workspace_id AND o.backend='rust' AND o.generation=$4)").bind(&job.resource_id).bind(job.payload["operationId"].as_str()).bind(error.code).bind(job.generation).execute(&self.pool).await?;
        }
        result.map(|_| true)
    }
    async fn lifecycle_step(
        &self,
        provider: &dyn RetainedProvider,
        job: &jobs::Job,
        now: i64,
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        let op = current(&mut tx, &row).await?;
        let d = directive(&op)?;
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3 FOR UPDATE)").bind(&job.id).bind(&job.lease_token).bind(clock_ms()).fetch_one(&mut *tx).await?;
        if !active || generation != job.generation || job.payload["operationId"] != d.operation_id {
            return Err(ApiError::conflict("cloud_lifecycle_fenced"));
        }
        let mut complete = matches!(d.phase.as_str(), "paused" | "ready");
        if d.phase == "quiescing" {
            if now - op["created_at"].as_i64().unwrap_or(now) > 120_000 {
                return Err(ApiError::unavailable("cloud_runtime_quiesce_failed"));
            }
        } else if !complete {
            if d.action == Action::Resume {
                require_resume_capacity(&mut tx, &row, now).await?;
            }
            // Serialize each provider mutation against workspace delete, unarchive and ownership handoff.
            let machine = provider
                .inspect_retained(&d.source_machine_id)
                .await
                .map_err(provider_error)?
                .ok_or_else(|| ApiError::conflict("cloud_retained_machine_missing"))?;
            if machine.id != d.source_machine_id {
                return Err(ApiError::conflict(
                    "cloud_retained_machine_identity_conflict",
                ));
            }
            if d.action == Action::Resume {
                match machine.state {
                    MachineState::Paused | MachineState::Stopped => provider
                        .wake_retained(&machine.id, &machine.state)
                        .await
                        .map_err(provider_error)?,
                    MachineState::Running => {
                        provider
                            .launch_retained(&d, &job.resource_id, &self.signer.issuer())
                            .await
                            .map_err(provider_error)?;
                        phase(&mut tx, &row, &op, "launching", clock_ms()).await?;
                    }
                    MachineState::Starting => {}
                    _ => {
                        return Err(ApiError::unavailable(
                            "cloud_retained_machine_state_unknown",
                        ));
                    }
                }
            } else {
                match machine.state {
                    MachineState::Running => {
                        provider
                            .pause_retained(&machine.id)
                            .await
                            .map_err(provider_error)?;
                        phase(&mut tx, &row, &op, "pausing", clock_ms()).await?;
                    }
                    MachineState::Paused | MachineState::Stopped => {
                        phase(&mut tx, &row, &op, "paused", clock_ms()).await?;
                        sqlx::query("UPDATE api_cloud_workspaces SET state=$2,runtime_state='offline',status_code=$3,next_action_at=COALESCE(archive_delete_at,9007199254740991) WHERE workspace_id=$1").bind(&job.resource_id).bind(if d.action==Action::Archive {"archived"}else{"paused"}).bind(if d.action==Action::Archive {"archived-retained"}else{"paused"}).execute(&mut *tx).await?;
                        complete = true;
                    }
                    _ => {
                        return Err(ApiError::unavailable(
                            "cloud_retained_machine_state_unknown",
                        ));
                    }
                }
            }
        }
        if !complete && job.attempts >= 120 {
            return Err(ApiError::unavailable("cloud_lifecycle_timed_out"));
        }
        let saved = sqlx::query("UPDATE zuse_rust_jobs SET state=$3,due_at=$4,lease_token=NULL,lease_expires_at=NULL,last_error_code=NULL,updated_at=$5 WHERE job_id=$1 AND lease_token=$2 AND lease_expires_at>$5")
            .bind(&job.id).bind(&job.lease_token).bind(if complete {"completed"}else{"pending"}).bind(clock_ms()+2000).bind(clock_ms()).execute(&mut *tx).await?;
        if saved.rows_affected() != 1 {
            return Err(ApiError::conflict("cloud_lifecycle_fenced"));
        }
        tx.commit().await?;
        Ok(())
    }
}
pub(super) async fn require_resume_capacity(
    tx: &mut Transaction<'_, Postgres>,
    row: &Value,
    now: i64,
) -> Result<(), ApiError> {
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
