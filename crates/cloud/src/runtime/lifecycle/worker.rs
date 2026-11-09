//! Short database admissions surround provider I/O; the original receipt survives lease loss.
use super::*;
use serde::{Deserialize, Serialize};
use sqlx::{Connection, PgConnection};
mod connection;
mod initial_boot;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wake_tests;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Effect {
    request_id: String,
    state: String,
    receipt: Option<ProviderOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    submitted_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    received_at_ms: Option<i64>,
}
fn effect_error() -> ApiError {
    ApiError::conflict("cloud_lifecycle_provider_receipt_invalid")
}

impl RuntimeControl {
    async fn lifecycle_locked(
        &self,
        job: &jobs::Job,
    ) -> Result<(Transaction<'static, Postgres>, Value, Value, Directive), ApiError> {
        self.lifecycle_locked_with(None, job).await
    }
    async fn lifecycle_locked_with<'c>(
        &self,
        connection: Option<&'c mut PgConnection>,
        job: &jobs::Job,
    ) -> Result<(Transaction<'c, Postgres>, Value, Value, Directive), ApiError> {
        let personal_only = connection.is_some();
        let mut tx = match connection {
            Some(connection) => connection.begin().await?,
            None => self.pool.begin().await?,
        };
        let (owner, row, op, active): (Option<i64>, Option<Value>, Option<Value>, bool) =
            sqlx::query_as(super::locking::JOB_SNAPSHOT)
                .bind(&job.resource_id)
                .bind(&job.id)
                .bind(&job.lease_token)
                .bind(job.generation)
                .bind(clock_ms())
                .bind(JOB_KIND)
                .fetch_one(&mut *tx)
                .await?;
        let (row, generation, _) = super::locking::validate_workspace(owner, row)?;
        if personal_only
            && row["account_id"]
                .as_str()
                .is_none_or(|owner| owner.starts_with("organization:"))
        {
            return Err(ApiError::conflict("cloud_lifecycle_fenced"));
        }
        if personal_only
            && (super::super::repair::leases_blocked(&row)
                || super::super::recreation::leases_blocked(&row))
        {
            return Err(ApiError::conflict("cloud_lifecycle_fenced"));
        }
        let op = op.ok_or_else(|| ApiError::conflict("cloud_lifecycle_operation_missing"))?;
        validate_current(&row, &op)?;
        if memory_recovery_for(&row, &op).is_some() {
            return Err(ApiError::conflict(
                "codex_memory_transport_recovery_required",
            ));
        }
        let d = directive(&op)?;
        if !active
            || generation != job.generation
            || job.payload["operationId"] != d.operation_id
            || (d.action == Action::Resume && row["desired_state"] != "ready")
        {
            return Err(ApiError::conflict("cloud_lifecycle_fenced"));
        }
        Ok((tx, row, op, d))
    }

    /// No network call may occur while this transaction is alive.
    async fn lifecycle_snapshot(
        &self,
        job: &jobs::Job,
    ) -> Result<(Value, Value, Directive), ApiError> {
        self.lifecycle_snapshot_with(None, job).await
    }
    async fn lifecycle_snapshot_with(
        &self,
        connection: Option<&mut PgConnection>,
        job: &jobs::Job,
    ) -> Result<(Value, Value, Directive), ApiError> {
        let (mut tx, row, op, d) = self.lifecycle_locked_with(connection, job).await?;
        if d.action == Action::Resume {
            require_resume_capacity(self.staging_candidate.as_deref(), &mut tx, &row, clock_ms())
                .await?;
        }
        tx.commit().await?;
        Ok((row, op, d))
    }

    async fn lifecycle_save(
        &self,
        job: &jobs::Job,
        observed_phase: &str,
        next: Option<&str>,
        pause_done: bool,
        new_receipt: bool,
    ) -> Result<(), ApiError> {
        self.lifecycle_save_with(None, job, observed_phase, next, pause_done, new_receipt)
            .await
    }
    async fn lifecycle_save_with(
        &self,
        connection: Option<&mut PgConnection>,
        job: &jobs::Job,
        observed_phase: &str,
        next: Option<&str>,
        pause_done: bool,
        new_receipt: bool,
    ) -> Result<(), ApiError> {
        let (mut tx, row, op, d) = self.lifecycle_locked_with(connection, job).await?;
        let mut complete = matches!(d.phase.as_str(), "paused" | "ready");
        let immediate =
            new_receipt || d.phase != observed_phase || next.is_some_and(|next| next != d.phase);
        if !complete {
            if pause_done {
                if !matches!(d.action, Action::Pause | Action::Archive) || d.phase == "quiescing" {
                    return Err(effect_error());
                }
                phase(&mut tx, &row, &op, "paused", clock_ms()).await?;
                sqlx::query("UPDATE api_cloud_workspaces SET state=$2,runtime_state='offline',status_code=$3,next_action_at=COALESCE(archive_delete_at,9007199254740991) WHERE workspace_id=$1")
                    .bind(&job.resource_id).bind(if d.action==Action::Archive {"archived"} else {"paused"})
                    .bind(if d.action==Action::Archive {"archived-retained"} else {"paused"}).execute(&mut *tx).await?;
                complete = true;
            } else if let Some(next) = next {
                if (next == "launching" && d.action != Action::Resume)
                    || (next == "pausing" && !matches!(d.action, Action::Pause | Action::Archive))
                {
                    return Err(effect_error());
                }
                phase(&mut tx, &row, &op, next, clock_ms()).await?;
            }
        }
        if !complete && job.attempts >= 120 {
            return Err(ApiError::unavailable("cloud_lifecycle_timed_out"));
        }
        let at = clock_ms();
        let saved=sqlx::query("UPDATE zuse_rust_jobs SET state=$3,due_at=$4,lease_token=NULL,lease_expires_at=NULL,last_error_code=NULL,updated_at=$5 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$5")
            .bind(&job.id).bind(&job.lease_token).bind(if complete {"completed"} else {"pending"}).bind(if immediate || complete {at}else{at+2000}).bind(at).execute(&mut *tx).await?;
        if saved.rows_affected() != 1 {
            return Err(ApiError::conflict("cloud_lifecycle_fenced"));
        }
        tx.commit().await?;
        if immediate && !complete {
            super::wakeup::send();
        }
        Ok(())
    }

    /// Persist intent before a mutation. The shared effects guard excludes another
    /// local worker; the database lease/generation remains the authority fence.
    async fn lifecycle_effect(
        &self,
        job: &jobs::Job,
        key: &str,
    ) -> Result<(Effect, bool), ApiError> {
        self.lifecycle_effect_with(None, job, key).await
    }
    async fn lifecycle_effect_with(
        &self,
        connection: Option<&mut PgConnection>,
        job: &jobs::Job,
        key: &str,
    ) -> Result<(Effect, bool), ApiError> {
        let (mut tx, row, op, d) = self.lifecycle_locked_with(connection, job).await?;
        if d.action == Action::Resume {
            require_resume_capacity(self.staging_candidate.as_deref(), &mut tx, &row, clock_ms())
                .await?;
        }
        if matches!(d.phase.as_str(), "paused" | "ready" | "quiescing") {
            return Err(ApiError::conflict("cloud_lifecycle_fenced"));
        }
        let prior = &op["provider_effects"][key];
        if !prior.is_null() {
            let effect = serde_json::from_value(prior.clone()).map_err(|_| effect_error())?;
            tx.commit().await?;
            return Ok((effect, false));
        }
        let effect = Effect {
            request_id: uuid::Uuid::new_v4().to_string(),
            state: "submitting".into(),
            receipt: None,
            submitted_at_ms: Some(clock_ms()),
            received_at_ms: None,
        };
        sqlx::query("UPDATE zuse_rust_lifecycle_operations SET provider_effects=jsonb_set(provider_effects,ARRAY[$2],$3),updated_at=$4 WHERE operation_id=$1")
            .bind(&d.operation_id).bind(key).bind(json!(effect)).bind(clock_ms()).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok((effect, true))
    }

    /// Write the receipt even if a concurrent delete/lease expiry fenced this job.
    /// It belongs to the immutable original operation, never a replacement resource.
    async fn lifecycle_receipt(
        &self,
        job: &jobs::Job,
        key: &str,
        effect: &Effect,
        result: &Result<ProviderOperation, ProviderError>,
    ) -> Result<(), ApiError> {
        self.lifecycle_receipt_with(None, job, key, effect, result)
            .await
    }
    async fn lifecycle_receipt_with(
        &self,
        connection: Option<&mut PgConnection>,
        job: &jobs::Job,
        key: &str,
        effect: &Effect,
        result: &Result<ProviderOperation, ProviderError>,
    ) -> Result<(), ApiError> {
        let record = Effect {
            request_id: effect.request_id.clone(),
            state: match result {
                Ok(_) => "accepted",
                Err(ProviderError::Rejected) => "rejected",
                Err(_) => "uncertain",
            }
            .into(),
            receipt: result.as_ref().ok().cloned(),
            submitted_at_ms: effect.submitted_at_ms,
            received_at_ms: Some(clock_ms()),
        };
        let query=sqlx::query("UPDATE zuse_rust_lifecycle_operations SET provider_effects=jsonb_set(provider_effects,ARRAY[$2],$3),updated_at=$4 WHERE operation_id=$1 AND workspace_id=$5 AND generation=$6 AND provider_effects->$2->>'requestId'=$7")
            .bind(job.payload["operationId"].as_str()).bind(key).bind(json!(record)).bind(clock_ms())
            .bind(&job.resource_id).bind(job.generation).bind(&effect.request_id);
        let saved = match connection {
            Some(connection) => query.execute(connection).await?,
            None => query.execute(&self.pool).await?,
        };
        if saved.rows_affected() != 1 {
            return Err(effect_error());
        }
        Ok(())
    }

    async fn reconcile_lifecycle_effect(
        &self,
        provider: &dyn RetainedProvider,
        resource: &str,
        desired: DesiredState,
        effect: &Effect,
        memory: bool,
    ) -> Result<OperationState, ApiError> {
        if effect.state == "rejected" {
            return Err(ApiError::unavailable("cloud_provider_rejected"));
        }
        let receipt = effect.receipt.clone().unwrap_or(ProviderOperation {
            resource_id: resource.into(),
            operation_id: None,
            desired,
        });
        if receipt.resource_id != resource || receipt.desired != desired {
            return Err(effect_error());
        }
        let observed = if memory {
            provider.memory_receipt_state(&receipt).await
        } else {
            provider.receipt_state(&receipt).await
        };
        match observed {
            Ok(OperationState::Superseded) => {
                Err(ApiError::conflict("cloud_provider_operation_superseded"))
            }
            Ok(state) => Ok(state),
            Err(_) if effect.receipt.is_none() => {
                Err(ApiError::unavailable("cloud_provider_outcome_unknown"))
            }
            Err(error) => Err(provider_error(error)),
        }
    }

    pub(super) async fn lifecycle_step(
        &self,
        provider: &dyn RetainedProvider,
        job: &jobs::Job,
        now: i64,
    ) -> Result<(), ApiError> {
        let _effects = effects::Guard::acquire(&job.resource_id)?;
        let mut connection = self.personal_memory_connection(job).await?;
        if connection.is_none() {
            self.worker_job_scope_access(job, "lifecycle", now).await?;
        }
        let (row, op, d) = self
            .lifecycle_snapshot_with(connection.as_mut().map(|c| &mut **c), job)
            .await?;
        if row["provider"] != provider.provider_id() {
            return Err(ApiError::conflict("cloud_provider_identity_mismatch"));
        }
        if matches!(d.phase.as_str(), "paused" | "ready") {
            return self
                .lifecycle_save_with(
                    connection.as_mut().map(|c| &mut **c),
                    job,
                    &d.phase,
                    None,
                    false,
                    false,
                )
                .await;
        }
        if d.phase == "quiescing" {
            if now - op["created_at"].as_i64().unwrap_or(now) > 120000 {
                return Err(ApiError::unavailable("cloud_runtime_quiesce_failed"));
            }
            return self
                .lifecycle_save_with(
                    connection.as_mut().map(|c| &mut **c),
                    job,
                    &d.phase,
                    None,
                    false,
                    false,
                )
                .await;
        }
        let wake = d.action == Action::Resume;
        let immediate_memory_wake = wake
            && connection.is_some()
            && matches!(row["provider"].as_str(), Some("boxd" | "e2b"));
        let memory = memory_parked(&row, &d);
        let key = if wake { "wake" } else { "sleep" };
        let desired = if wake {
            DesiredState::Running
        } else {
            DesiredState::Sleeping
        };
        // Accepted or uncertain mutations are observed by original identity, never replayed.
        if !op["provider_effects"][key].is_null() {
            let effect: Effect = serde_json::from_value(op["provider_effects"][key].clone())
                .map_err(|_| effect_error())?;
            if self
                .reconcile_lifecycle_effect(
                    provider,
                    &d.source_machine_id,
                    desired,
                    &effect,
                    memory,
                )
                .await?
                != OperationState::Completed
            {
                return self
                    .lifecycle_save_with(
                        connection.as_mut().map(|c| &mut **c),
                        job,
                        &d.phase,
                        None,
                        false,
                        false,
                    )
                    .await;
            }
        } else {
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
            let already = if wake {
                machine.state == MachineState::Running
            } else {
                matches!(
                    machine.state,
                    MachineState::Standby | MachineState::Paused | MachineState::Stopped
                )
            };
            if already && !wake {
                // Some providers expose a stopped-looking resource before its
                // snapshot operation has completed. Let the adapter prove it;
                // absence of our own submission is not a completion receipt.
                let receipt = ProviderOperation {
                    resource_id: machine.id.clone(),
                    operation_id: None,
                    desired,
                };
                let observed = if memory {
                    provider.memory_receipt_state(&receipt).await
                } else {
                    provider.receipt_state(&receipt).await
                }
                .map_err(|_| ApiError::unavailable("cloud_provider_outcome_unknown"))?;
                if observed == OperationState::Superseded {
                    return Err(ApiError::conflict("cloud_provider_operation_superseded"));
                }
                if observed != OperationState::Completed {
                    return self
                        .lifecycle_save_with(
                            connection.as_mut().map(|c| &mut **c),
                            job,
                            &d.phase,
                            None,
                            false,
                            false,
                        )
                        .await;
                }
            }
            if !already {
                if matches!(machine.state, MachineState::Starting) && wake {
                    return self
                        .lifecycle_save_with(
                            connection.as_mut().map(|c| &mut **c),
                            job,
                            &d.phase,
                            None,
                            false,
                            false,
                        )
                        .await;
                }
                if !matches!(
                    machine.state,
                    MachineState::Running
                        | MachineState::Standby
                        | MachineState::Paused
                        | MachineState::Stopped
                ) {
                    return Err(ApiError::unavailable(
                        "cloud_retained_machine_state_unknown",
                    ));
                }
                let (effect, fresh) = self
                    .lifecycle_effect_with(connection.as_mut().map(|c| &mut **c), job, key)
                    .await?;
                if fresh {
                    let result = if wake {
                        provider.wake_receipt(&machine.id, &machine.state).await
                    } else if memory {
                        provider.memory_sleep_receipt(&machine.id).await
                    } else {
                        provider.sleep_receipt(&machine.id).await
                    };
                    // Invalid receipts are retained as uncertain, not used to target another resource.
                    let result = result.and_then(|r| {
                        if r.resource_id == machine.id && r.desired == desired {
                            Ok(r)
                        } else {
                            Err(ProviderError::InvalidResponse)
                        }
                    });
                    self.lifecycle_receipt_with(
                        connection.as_mut().map(|c| &mut **c),
                        job,
                        key,
                        &effect,
                        &result,
                    )
                    .await?;
                    let receipt = result.map_err(provider_error)?;
                    if immediate_memory_wake {
                        // The accepted original receipt is durable before this
                        // read. A completed RAM wake can retain this lease and
                        // reach the ordinary post-I/O launcher fence directly.
                        let accepted = Effect {
                            request_id: effect.request_id.clone(),
                            state: "accepted".into(),
                            receipt: Some(receipt),
                            submitted_at_ms: effect.submitted_at_ms,
                            received_at_ms: None,
                        };
                        if self
                            .reconcile_lifecycle_effect(
                                provider,
                                &d.source_machine_id,
                                desired,
                                &accepted,
                                false,
                            )
                            .await?
                            == OperationState::Completed
                        {
                            // No requeue/reclaim between accepted wake and launch.
                        } else {
                            return self
                                .lifecycle_save_with(
                                    connection.as_mut().map(|c| &mut **c),
                                    job,
                                    &d.phase,
                                    None,
                                    false,
                                    false,
                                )
                                .await;
                        }
                    } else {
                        return self
                            .lifecycle_save_with(
                                connection.as_mut().map(|c| &mut **c),
                                job,
                                &d.phase,
                                if wake { None } else { Some("pausing") },
                                false,
                                true,
                            )
                            .await;
                    }
                } else {
                    return self
                        .lifecycle_save_with(
                            connection.as_mut().map(|c| &mut **c),
                            job,
                            &d.phase,
                            if wake { None } else { Some("pausing") },
                            false,
                            false,
                        )
                        .await;
                }
            }
        }
        if !wake {
            return self
                .lifecycle_save_with(
                    connection.as_mut().map(|c| &mut **c),
                    job,
                    &d.phase,
                    None,
                    true,
                    false,
                )
                .await;
        }
        // Runtime launch is independently idempotent under the guest's lifetime lock.
        // Revalidate after all provider reads and before dispatching that launcher.
        if !provider
            .retained_launch_ready(&d.source_machine_id)
            .await
            .map_err(provider_error)?
        {
            return self
                .lifecycle_save_with(
                    connection.as_mut().map(|c| &mut **c),
                    job,
                    &d.phase,
                    None,
                    false,
                    false,
                )
                .await;
        }
        let (row, _, mut current) = self
            .lifecycle_snapshot_with(connection.as_mut().map(|c| &mut **c), job)
            .await?;
        if current.phase != "ready" {
            if let Some(request) = initial_boot::request(&row, &current)? {
                if connection.is_none() {
                    self.worker_job_scope_access(job, "lifecycle", clock_ms())
                        .await?;
                    let (fresh, _, directive) = self.lifecycle_snapshot(job).await?;
                    if initial_boot::request(&fresh, &directive)? != Some(request.clone())
                        || fresh["request_config"]["managedReleaseId"]
                            != row["request_config"]["managedReleaseId"]
                    {
                        return Err(ApiError::conflict("cloud_lifecycle_fenced"));
                    }
                    if directive.phase == "ready" {
                        return self.lifecycle_save(job, &d.phase, None, false, false).await;
                    }
                }
                let ready = provider
                    .bootstrap_retained_runtime(&request)
                    .await
                    .map_err(provider_error)?;
                // Recheck the exact operation, lease, scope/capacity and image pins
                // after the start helper, before ordinary retained handoff.
                if connection.is_none() {
                    self.worker_job_scope_access(job, "lifecycle", clock_ms())
                        .await?;
                }
                let (fresh, _, directive) = self
                    .lifecycle_snapshot_with(connection.as_mut().map(|c| &mut **c), job)
                    .await?;
                if initial_boot::request(&fresh, &directive)? != Some(request)
                    || fresh["request_config"]["managedReleaseId"]
                        != row["request_config"]["managedReleaseId"]
                {
                    return Err(ApiError::conflict("cloud_lifecycle_fenced"));
                }
                current = directive;
                if !ready {
                    return self
                        .lifecycle_save_with(
                            connection.as_mut().map(|c| &mut **c),
                            job,
                            &d.phase,
                            None,
                            false,
                            false,
                        )
                        .await;
                }
            }
        }
        if current.phase == "ready" {
            return self
                .lifecycle_save_with(
                    connection.as_mut().map(|c| &mut **c),
                    job,
                    &d.phase,
                    None,
                    false,
                    false,
                )
                .await;
        }
        provider
            .launch_retained(&current, &job.resource_id, &self.signer.issuer())
            .await
            .map_err(provider_error)?;
        self.lifecycle_save_with(
            connection.as_mut().map(|c| &mut **c),
            job,
            &d.phase,
            Some("launching"),
            false,
            false,
        )
        .await
    }
}

/// The exact authenticated park authorizes RAM sleep, not merely a provider feature flag.
fn memory_parked(row: &Value, d: &Directive) -> bool {
    let c = &row["request_config"];
    let p = &c["rustMemoryParked"];
    d.action == Action::Pause
        && !d.full_shutdown
        && c["rustMemorySleepVersion"] == 1
        && p["version"] == 1
        && p["operationId"] == d.operation_id
        && p["sourceMachineId"] == d.source_machine_id
        && p["generation"] == d.generation
        && p["storageIncarnationId"] == d.storage_incarnation_id
        && matches!(row["provider"].as_str(), Some("boxd" | "e2b"))
        && c["rustStorageEnrollment"]["capabilities"]
            .as_array()
            .is_some_and(|v| {
                v.iter()
                    .any(|v| v == zuse_protocol::cloud_lifecycle::MEMORY_CAPABILITY)
            })
}
