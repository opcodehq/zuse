//! Short, generation-fenced admissions for archive helper and provider operations.
use super::*;
use serde::{Deserialize, Serialize};

/// How long an unanswered effect is given before an unchanged machine proves
/// it never happened. Covers the provider's own request timeout and a status
/// that lags an accepted request.
pub(super) const EFFECT_SETTLE_MS: i64 = 90_000;
/// An effect still unresolved after this long needs a person, not more polling.
pub(super) const EFFECT_WAIT_LIMIT_MS: i64 = 10 * 60_000;
/// Submissions of one effect that may each be proven not to have happened
/// before the operation stops and asks for attention.
pub(super) const EFFECT_MAX_SUBMISSIONS: u32 = 3;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Effect {
    request_id: String,
    /// submitting | accepted | uncertain | rejected | completed | not-applied
    state: String,
    receipt: Option<ProviderOperation>,
    /// When this state was recorded. Effects written before this field
    /// existed start their settle window on the next observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recorded_at_ms: Option<i64>,
    /// Which submission of this effect this is, starting at 1.
    #[serde(default)]
    submissions: u32,
    /// The provider's error kind for a submission that did not return a receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}
fn sleeping(state: &MachineState) -> bool {
    matches!(
        state,
        MachineState::Standby | MachineState::Paused | MachineState::Stopped
    )
}
/// The machine is still exactly where it was before the effect was submitted.
fn before_effect(desired: DesiredState, state: &MachineState) -> bool {
    match desired {
        DesiredState::Running => sleeping(state),
        DesiredState::Sleeping => *state == MachineState::Running,
        DesiredState::Deleted => false,
    }
}
fn error_kind(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::NotFound => "not-found",
        ProviderError::Rejected => "rejected",
        ProviderError::Conflict => "conflict",
        ProviderError::RateLimited => "rate-limited",
        ProviderError::Unavailable => "unavailable",
        ProviderError::OutcomeUnknown => "outcome-unknown",
        ProviderError::InvalidResponse => "invalid-response",
    }
}
/// Every submission was proven not to have happened.
fn failed_code(key: &str) -> &'static str {
    match key {
        "wake" | "cancelWake" => "archive-provider-wake-failed",
        _ => "archive-provider-pause-failed",
    }
}
/// The effect neither completed nor provably failed within the wait limit.
fn unconfirmed_code(key: &str) -> &'static str {
    match key {
        "wake" | "cancelWake" => "archive-provider-wake-unconfirmed",
        _ => "archive-provider-pause-unconfirmed",
    }
}
enum Pending {
    Wait,
    Resubmit(u32),
    Attention(&'static str, bool),
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
    /// Submit, or reconcile, one provider effect of this recovery operation.
    ///
    /// An accepted or unanswered effect is never blindly replayed. It is
    /// observed by its original identity; an unanswered one is resubmitted
    /// (with a fresh request id) only after inspection proves the machine is
    /// still where it was before the effect, past the settle window. Anything
    /// still unresolved after the wait limit, or every allowed submission
    /// proven not to have happened, stops the job with an attention code
    /// instead of polling forever.
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
        let (tx, _, op) = self.recovery_stage_locked(job, phase).await?;
        tx.commit().await?;
        let source = required(&op, "source_machine_id")?.to_owned();
        let value = &op["provider_effects"][key];
        let prior: Option<Effect> = if value.is_null() {
            None
        } else {
            Some(serde_json::from_value(value.clone()).map_err(|_| fenced())?)
        };
        let mut state = state.clone();
        let mut submissions = 1;
        if let Some(effect) = prior
            .as_ref()
            .filter(|e| !matches!(e.state.as_str(), "rejected" | "not-applied"))
        {
            let receipt = effect.receipt.clone().unwrap_or(ProviderOperation {
                resource_id: source.clone(),
                operation_id: None,
                desired,
            });
            if receipt.resource_id != source || receipt.desired != desired {
                return Err(fenced());
            }
            if effect.state == "completed" {
                return Ok(true);
            }
            let observed = provider
                .recovery_state(&receipt)
                .await
                .map_err(|_| ApiError::unavailable("archive-provider-outcome-unknown"))?;
            let pending = match observed {
                OperationState::Completed => {
                    let (mut tx, _, current) = self.recovery_stage_locked(job, phase).await?;
                    if current["provider_effects"][key]["requestId"] != effect.request_id {
                        return Err(fenced());
                    }
                    let completed = Effect {
                        state: "completed".into(),
                        receipt: Some(receipt),
                        recorded_at_ms: Some(clock_ms()),
                        ..effect.clone()
                    };
                    sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,$2,$3),updated_at=$4 WHERE operation_id=$1")
                        .bind(required(&current,"operation_id")?).bind(vec![key]).bind(json!(completed)).bind(clock_ms()).execute(&mut *tx).await?;
                    tx.commit().await?;
                    return Ok(true);
                }
                OperationState::Superseded => {
                    return Err(ApiError::conflict("archive-provider-operation-superseded"));
                }
                OperationState::Pending | OperationState::Retrying => {
                    let now = clock_ms();
                    match effect.recorded_at_ms {
                        // Written before effects carried a time: start its
                        // settle window now rather than trusting its age.
                        None => {
                            self.recovery_effect_record(
                                job,
                                phase,
                                key,
                                effect,
                                Effect {
                                    recorded_at_ms: Some(now),
                                    ..effect.clone()
                                },
                            )
                            .await?;
                            Pending::Wait
                        }
                        Some(since) => {
                            let unanswered = effect.receipt.is_none()
                                && observed == OperationState::Pending
                                && matches!(effect.state.as_str(), "uncertain" | "submitting");
                            if unanswered && now - since >= EFFECT_SETTLE_MS {
                                let machine = provider.inspect(&source).await.map_err(|_| {
                                    ApiError::unavailable("archive-provider-outcome-unknown")
                                })?;
                                match machine {
                                    Some(m)
                                        if m.id == source && before_effect(desired, &m.state) =>
                                    {
                                        // Proven not to have happened.
                                        let made = effect.submissions.max(1);
                                        state = m.state;
                                        if made >= EFFECT_MAX_SUBMISSIONS {
                                            Pending::Attention(failed_code(key), true)
                                        } else {
                                            Pending::Resubmit(made + 1)
                                        }
                                    }
                                    _ if now - since >= EFFECT_WAIT_LIMIT_MS => {
                                        Pending::Attention(unconfirmed_code(key), false)
                                    }
                                    _ => Pending::Wait,
                                }
                            } else if now - since >= EFFECT_WAIT_LIMIT_MS {
                                Pending::Attention(unconfirmed_code(key), false)
                            } else {
                                Pending::Wait
                            }
                        }
                    }
                }
            };
            match pending {
                Pending::Wait => return Ok(false),
                Pending::Attention(code, not_applied) => {
                    return self
                        .recovery_effect_attention(job, phase, key, effect, code, not_applied)
                        .await;
                }
                Pending::Resubmit(next) => {
                    tracing::warn!(
                        key,
                        submission = next,
                        error = effect.error.as_deref().unwrap_or("unknown"),
                        "Recovery provider effect did not happen; resubmitting"
                    );
                    submissions = next;
                }
            }
        }
        let effect = Effect {
            request_id: uuid::Uuid::new_v4().to_string(),
            state: "submitting".into(),
            receipt: None,
            recorded_at_ms: Some(clock_ms()),
            submissions,
            error: None,
        };
        {
            // Compare-and-set against the effect this decision was made from.
            let (mut tx, _, current) = self.recovery_stage_locked(job, phase).await?;
            if current["provider_effects"][key]["requestId"]
                != op["provider_effects"][key]["requestId"]
            {
                return Err(fenced());
            }
            sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,$2,$3),updated_at=$4 WHERE operation_id=$1")
                .bind(required(&current,"operation_id")?).bind(vec![key]).bind(json!(effect)).bind(clock_ms()).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        let result = match desired {
            DesiredState::Running => provider.wake_receipt(&source, &state).await,
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
            recorded_at_ms: Some(clock_ms()),
            submissions,
            error: result.as_ref().err().map(|e| error_kind(e).to_owned()),
        };
        let saved=sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,$2,$3),updated_at=$4 WHERE operation_id=$1 AND workspace_id=$5 AND generation=$6 AND source_machine_id=$7 AND provider_effects->$8->>'requestId'=$9")
            .bind(required(&op,"operation_id")?).bind(vec![key]).bind(json!(record)).bind(clock_ms()).bind(&job.resource_id).bind(job.generation).bind(&source).bind(key).bind(&effect.request_id).execute(&self.pool).await?;
        if saved.rows_affected() != 1 {
            return Err(fenced());
        }
        if let Err(error) = &result {
            // Name the provider's answer so a stuck effect is diagnosable.
            tracing::warn!(
                key,
                submission = submissions,
                error = error_kind(error),
                "Recovery provider effect unconfirmed"
            );
        }
        result.map_err(|_| ApiError::unavailable("archive-provider-operation-failed"))?;
        Ok(false)
    }
    /// Replace the effect this pass decided from, fenced on its request id.
    async fn recovery_effect_record(
        &self,
        job: &jobs::Job,
        phase: &str,
        key: &str,
        from: &Effect,
        record: Effect,
    ) -> Result<(), ApiError> {
        let (mut tx, _, current) = self.recovery_stage_locked(job, phase).await?;
        if current["provider_effects"][key]["requestId"] != from.request_id {
            return Err(fenced());
        }
        sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,$2,$3),updated_at=$4 WHERE operation_id=$1")
            .bind(required(&current,"operation_id")?).bind(vec![key]).bind(json!(record)).bind(clock_ms()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    /// Stop this recovery job and surface `code` (on the computer and any
    /// Update computer that is capturing through it). A retry re-pends the job.
    ///
    /// A proven-not-applied effect is recorded `not-applied` so the retry
    /// submits afresh; an unconfirmed one keeps its identity and restarts its
    /// wait window so the retry observes it again first.
    async fn recovery_effect_attention(
        &self,
        job: &jobs::Job,
        phase: &str,
        key: &str,
        effect: &Effect,
        code: &'static str,
        not_applied: bool,
    ) -> Result<bool, ApiError> {
        let now = clock_ms();
        let (mut tx, row, current) = self.recovery_stage_locked(job, phase).await?;
        if current["provider_effects"][key]["requestId"] != effect.request_id {
            return Err(fenced());
        }
        let record = Effect {
            state: if not_applied {
                "not-applied".into()
            } else {
                effect.state.clone()
            },
            recorded_at_ms: Some(now),
            ..effect.clone()
        };
        sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,$2,$3),updated_at=$4 WHERE operation_id=$1")
            .bind(required(&current,"operation_id")?).bind(vec![key]).bind(json!(record)).bind(now).execute(&mut *tx).await?;
        let stopped=sqlx::query("UPDATE zuse_rust_jobs SET state='failed',lease_token=NULL,lease_expires_at=NULL,last_error_code=$3,updated_at=$4 WHERE job_id=$1 AND lease_token=$2 AND state='running'")
            .bind(&job.id).bind(&job.lease_token).bind(code).bind(now).execute(&mut *tx).await?;
        if stopped.rows_affected() != 1 {
            return Err(fenced());
        }
        sqlx::query("UPDATE api_cloud_workspaces SET status_code=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1")
            .bind(&job.resource_id).bind(code).bind(now).execute(&mut *tx).await?;
        let recreation = super::super::recreation::capture_intent(&row)
            .then(|| row["request_config"]["rustRecreation"]["operationId"].as_str())
            .flatten()
            .map(str::to_owned);
        tx.commit().await?;
        tracing::warn!(
            code,
            key,
            submission = effect.submissions,
            error = effect.error.as_deref().unwrap_or("unknown"),
            "Recovery provider effect needs attention"
        );
        if let Some(operation) = recreation {
            self.recreation_attention(&operation, code, now).await?;
        }
        Err(ApiError::conflict(code))
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
