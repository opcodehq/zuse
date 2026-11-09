//! Short, generation-fenced admissions for archive helper and provider operations.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Effect {
    request_id: String,
    state: String,
    receipt: Option<ProviderOperation>,
}
fn sleeping(state: &MachineState) -> bool {
    matches!(
        state,
        MachineState::Standby | MachineState::Paused | MachineState::Stopped
    )
}
impl RuntimeControl {
    pub(super) async fn recovery_stage_locked(
        &self,
        job: &jobs::Job,
        phase: &str,
    ) -> Result<(Transaction<'static, Postgres>, Value, Value), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        active_job(&mut tx, job).await?;
        let op:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_recovery_operations o WHERE operation_id=$1 AND workspace_id=$2 FOR UPDATE")
            .bind(job.payload["operationId"].as_str()).bind(&job.resource_id).fetch_optional(&mut *tx).await?;
        let op = op.ok_or_else(fenced)?;
        if deleted(&row)
            || generation != job.generation
            || op["generation"] != generation
            || !(op["phase"] == phase
                || phase == "upload-complete"
                    && matches!(op["phase"].as_str(), Some("uploading" | "verifying")))
            || op["source_machine_id"] != row["provider_sandbox_id"]
            || op["binding"]["workspaceId"] != row["workspace_id"]
            || op["binding"]["chatId"] != row["chat_id"]
            || op["binding"]["sessionId"] != row["initial_session_id"]
            || op["binding"]["rustStorage"]["accountId"] != row["account_id"]
            || op["storage_incarnation_id"]
                != row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
        {
            return Err(fenced());
        }
        if phase == "cancelled" {
            if row["request_config"]["rustRecoveryCancellation"] != op["operation_id"] {
                return Err(fenced());
            }
        } else if row["desired_state"] != "archived"
            && !super::super::recreation::capture_intent(&row)
            || archive(&row)?.id != required(&op, "operation_id")?
        {
            return Err(fenced());
        }
        Ok((tx, row, op))
    }
    async fn recovery_stage_defer(&self, job: &jobs::Job, phase: &str) -> Result<(), ApiError> {
        let (mut tx, _, _) = self.recovery_stage_locked(job, phase).await?;
        defer(&mut tx, job, false, clock_ms()).await?;
        tx.commit().await?;
        Ok(())
    }
    async fn recovery_observe(
        &self,
        job: &jobs::Job,
        phase: &str,
        machine: Option<&Machine>,
    ) -> Result<(), ApiError> {
        let (mut tx, row, op) = self.recovery_stage_locked(job, phase).await?;
        if machine.is_some_and(|m| op["source_machine_id"] != m.id) {
            return Err(fenced());
        }
        observe(&mut tx, &row, &op, machine, clock_ms()).await?;
        tx.commit().await?;
        Ok(())
    }
    async fn recovery_effect(
        &self,
        provider: &dyn RecoveryProvider,
        job: &jobs::Job,
        phase: &str,
        key: &str,
        desired: DesiredState,
        state: &MachineState,
    ) -> Result<bool, ApiError> {
        if !matches!(key, "wake" | "pause" | "cancelWake") {
            return Err(fenced());
        }
        let (mut tx, _, op) = self.recovery_stage_locked(job, phase).await?;
        let source = required(&op, "source_machine_id")?.to_owned();
        let value = &op["provider_effects"][key];
        let prior: Option<Effect> = if value.is_null() {
            None
        } else {
            Some(serde_json::from_value(value.clone()).map_err(|_| fenced())?)
        };
        if let Some(effect) = prior.as_ref().filter(|e| e.state != "rejected") {
            let receipt = effect.receipt.clone().unwrap_or(ProviderOperation {
                resource_id: source.clone(),
                operation_id: None,
                desired,
            });
            if receipt.resource_id != source || receipt.desired != desired {
                return Err(fenced());
            }
            if effect.state == "completed" {
                tx.commit().await?;
                return Ok(true);
            }
            tx.commit().await?;
            let observed = provider
                .recovery_state(&receipt)
                .await
                .map_err(|_| ApiError::unavailable("archive-provider-outcome-unknown"))?;
            let (mut tx, _, current) = self.recovery_stage_locked(job, phase).await?;
            if current["provider_effects"][key]["requestId"] != effect.request_id {
                return Err(fenced());
            }
            match observed {
                OperationState::Completed => {
                    let completed = Effect {
                        request_id: effect.request_id.clone(),
                        state: "completed".into(),
                        receipt: Some(receipt),
                    };
                    sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,$2,$3),updated_at=$4 WHERE operation_id=$1")
                        .bind(required(&current,"operation_id")?).bind(vec![key]).bind(json!(completed)).bind(clock_ms()).execute(&mut *tx).await?;
                    tx.commit().await?;
                    return Ok(true);
                }
                OperationState::Pending | OperationState::Retrying => {
                    tx.commit().await?;
                    return Ok(false);
                }
                OperationState::Superseded => {
                    return Err(ApiError::conflict("archive-provider-operation-superseded"));
                }
            }
        }
        let effect = Effect {
            request_id: uuid::Uuid::new_v4().to_string(),
            state: "submitting".into(),
            receipt: None,
        };
        sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,$2,$3),updated_at=$4 WHERE operation_id=$1")
            .bind(required(&op,"operation_id")?).bind(vec![key]).bind(json!(effect)).bind(clock_ms()).execute(&mut *tx).await?;
        tx.commit().await?;
        let result = match desired {
            DesiredState::Running => provider.wake_receipt(&source, state).await,
            DesiredState::Sleeping => provider.pause_receipt(&source).await,
            _ => return Err(fenced()),
        }
        .and_then(|r| {
            if r.resource_id == source && r.desired == desired {
                Ok(r)
            } else {
                Err(ProviderError::InvalidResponse)
            }
        });
        let record = Effect {
            request_id: effect.request_id.clone(),
            state: match &result {
                Ok(_) => "accepted",
                Err(ProviderError::Rejected) => "rejected",
                Err(_) => "uncertain",
            }
            .into(),
            receipt: result.as_ref().ok().cloned(),
        };
        let saved=sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,$2,$3),updated_at=$4 WHERE operation_id=$1 AND workspace_id=$5 AND generation=$6 AND source_machine_id=$7 AND provider_effects->$8->>'requestId'=$9")
            .bind(required(&op,"operation_id")?).bind(vec![key]).bind(json!(record)).bind(clock_ms()).bind(&job.resource_id).bind(job.generation).bind(&source).bind(key).bind(&effect.request_id).execute(&self.pool).await?;
        if saved.rows_affected() != 1 {
            return Err(fenced());
        }
        result.map_err(|_| ApiError::unavailable("archive-provider-operation-failed"))?;
        Ok(false)
    }
    pub(super) async fn recovery_upload(
        &self,
        provider: &dyn RecoveryProvider,
        job: &jobs::Job,
    ) -> Result<(), ApiError> {
        let (mut tx, row, op) = self.recovery_stage_locked(job, "uploading").await?;
        let mut a = archive(&row)?;
        if a.expires_at_ms <= clock_ms() {
            a.expires_at_ms = clock_ms() + TTL;
            sqlx::query(
                "UPDATE zuse_rust_recovery_operations SET expires_at=$2 WHERE operation_id=$1",
            )
            .bind(&a.id)
            .bind(a.expires_at_ms)
            .execute(&mut *tx)
            .await?;
            save(&mut tx, &row, &a, "archive-uploading", clock_ms()).await?;
        }
        tx.commit().await?;
        let machine = provider
            .inspect(&a.source_sandbox_id)
            .await
            .map_err(|_| failed())?
            .ok_or_else(|| ApiError::unavailable("archive-source-missing"))?;
        self.recovery_observe(job, "uploading", Some(&machine))
            .await?;
        // Resolve a previous accepted/uncertain wake even if inspection now says running.
        if !op["provider_effects"]["wake"].is_null() || sleeping(&machine.state) {
            if !self
                .recovery_effect(
                    provider,
                    job,
                    "uploading",
                    "wake",
                    DesiredState::Running,
                    &machine.state,
                )
                .await?
            {
                return self.recovery_stage_defer(job, "uploading").await;
            }
        }
        match machine.state {
            MachineState::Running => {
                let (tx, row, op) = self.recovery_stage_locked(job, "uploading").await?;
                let plan = self.capture_plan(&row, &archive(&row)?, op["binding"].clone())?;
                tx.commit().await?;
                // The helper's durable operation ID/runner lock/uploaded receipt makes
                // repeated launch of this same capture idempotent, including a lost exec reply.
                provider
                    .capture(&plan)
                    .await
                    .map_err(|_| ApiError::unavailable("archive-capture-failed"))?;
            }
            MachineState::Starting => {}
            _ => return Err(failed()),
        }
        // Capture can publish its manifest before execute returns. Advancing to
        // verification is legitimate progress, but cancellation/deletion still fences us.
        self.recovery_stage_defer(job, "upload-complete").await
    }
    pub(super) async fn recovery_prepare_verification(
        &self,
        provider: &dyn RecoveryProvider,
        job: &jobs::Job,
    ) -> Result<bool, ApiError> {
        let (tx, _, op) = self.recovery_stage_locked(job, "verifying").await?;
        tx.commit().await?;
        let source = required(&op, "source_machine_id")?;
        let machine = provider.inspect(source).await.map_err(|_| failed())?;
        self.recovery_observe(job, "verifying", machine.as_ref())
            .await?;
        if let Some(machine) = machine {
            if machine.state == MachineState::Running {
                if op["helper_stopped"] != true {
                    let (tx, _, _) = self.recovery_stage_locked(job, "verifying").await?;
                    tx.commit().await?;
                    provider
                        .stop_capture(source, required(&op, "operation_id")?)
                        .await
                        .map_err(|_| failed())?;
                    let (mut tx, _, current) = self.recovery_stage_locked(job, "verifying").await?;
                    sqlx::query("UPDATE zuse_rust_recovery_operations SET helper_stopped=true,updated_at=$2 WHERE operation_id=$1").bind(required(&current,"operation_id")?).bind(clock_ms()).execute(&mut *tx).await?;
                    tx.commit().await?;
                }
                self.recovery_effect(
                    provider,
                    job,
                    "verifying",
                    "pause",
                    DesiredState::Sleeping,
                    &machine.state,
                )
                .await?;
                self.recovery_stage_defer(job, "verifying").await?;
                return Ok(false);
            }
            if !sleeping(&machine.state) {
                self.recovery_stage_defer(job, "verifying").await?;
                return Ok(false);
            }
            if !op["provider_effects"]["pause"].is_null()
                && !self
                    .recovery_effect(
                        provider,
                        job,
                        "verifying",
                        "pause",
                        DesiredState::Sleeping,
                        &machine.state,
                    )
                    .await?
            {
                self.recovery_stage_defer(job, "verifying").await?;
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub(super) async fn recovery_cancel(
        &self,
        provider: &dyn RecoveryProvider,
        job: &jobs::Job,
    ) -> Result<(), ApiError> {
        let (tx, _, op) = self.recovery_stage_locked(job, "cancelled").await?;
        tx.commit().await?;
        let source = required(&op, "source_machine_id")?;
        let machine = provider
            .inspect(source)
            .await
            .map_err(|_| failed())?
            .ok_or_else(|| ApiError::unavailable("archive-source-missing"))?;
        self.recovery_observe(job, "cancelled", Some(&machine))
            .await?;
        if !op["provider_effects"]["wake"].is_null()
            && !self
                .recovery_effect(
                    provider,
                    job,
                    "cancelled",
                    "wake",
                    DesiredState::Running,
                    &machine.state,
                )
                .await?
        {
            return self.recovery_stage_defer(job, "cancelled").await;
        }
        if sleeping(&machine.state) && op["helper_stopped"] != true {
            self.recovery_effect(
                provider,
                job,
                "cancelled",
                "cancelWake",
                DesiredState::Running,
                &machine.state,
            )
            .await?;
            return self.recovery_stage_defer(job, "cancelled").await;
        }
        if !op["provider_effects"]["cancelWake"].is_null()
            && !self
                .recovery_effect(
                    provider,
                    job,
                    "cancelled",
                    "cancelWake",
                    DesiredState::Running,
                    &machine.state,
                )
                .await?
        {
            return self.recovery_stage_defer(job, "cancelled").await;
        }
        if machine.state == MachineState::Starting {
            return self.recovery_stage_defer(job, "cancelled").await;
        }
        if machine.state == MachineState::Running {
            let (tx, _, _) = self.recovery_stage_locked(job, "cancelled").await?;
            tx.commit().await?;
            provider
                .stop_capture(source, required(&op, "operation_id")?)
                .await
                .map_err(|_| ApiError::unavailable("archive-cancel-failed"))?;
            let (mut tx, _, current) = self.recovery_stage_locked(job, "cancelled").await?;
            sqlx::query("UPDATE zuse_rust_recovery_operations SET helper_stopped=true,updated_at=$2 WHERE operation_id=$1").bind(required(&current,"operation_id")?).bind(clock_ms()).execute(&mut *tx).await?;
            tx.commit().await?;
            self.recovery_effect(
                provider,
                job,
                "cancelled",
                "pause",
                DesiredState::Sleeping,
                &machine.state,
            )
            .await?;
            return self.recovery_stage_defer(job, "cancelled").await;
        }
        if !sleeping(&machine.state) {
            return Err(failed());
        }
        if !op["provider_effects"]["pause"].is_null()
            && !self
                .recovery_effect(
                    provider,
                    job,
                    "cancelled",
                    "pause",
                    DesiredState::Sleeping,
                    &machine.state,
                )
                .await?
        {
            return self.recovery_stage_defer(job, "cancelled").await;
        }
        let (mut tx, _, current) = self.recovery_stage_locked(job, "cancelled").await?;
        sqlx::query("UPDATE zuse_rust_recovery_operations SET helper_stopped=true,updated_at=$2 WHERE operation_id=$1").bind(required(&current,"operation_id")?).bind(clock_ms()).execute(&mut *tx).await?;
        sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash=$2,request_config=request_config-'rustRecoveryCancellation',revision=revision+1 WHERE workspace_id=$1").bind(&job.resource_id).bind(current["source_credential_hash"].as_str()).execute(&mut *tx).await?;
        defer(&mut tx, job, true, clock_ms()).await?;
        tx.commit().await?;
        Ok(())
    }
}
