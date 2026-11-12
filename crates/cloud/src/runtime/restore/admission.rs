use super::*;
fn clock_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
impl RuntimeControl {
    pub(in crate::runtime) async fn restore_locked(
        &self,
        job: &jobs::Job,
    ) -> Result<
        (
            Transaction<'static, Postgres>,
            Value,
            Value,
            zuse_protocol::cloud::RecoveryArchive,
            Value,
        ),
        ApiError,
    > {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        active_job(&mut tx, job).await?;
        if lifecycle::deleted(&row)
            || row["desired_state"] != "ready"
            || generation != job.generation
            || row["request_config"]["rustRestoreOperationId"] != job.payload["operationId"]
        {
            return Err(fenced());
        }
        let op = locked_op(&mut tx, &row).await?;
        let a = archive(&row)?;
        let restore:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_restores r WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 FOR UPDATE").bind(job.payload["operationId"].as_str()).bind(&job.resource_id).bind(generation).fetch_one(&mut *tx).await?;
        if restore["archive_operation_id"] != op["operation_id"]
            || !a.backup_verified()
            || op["verified_at"].is_null()
            || op["verified_archive_sha256"]
                != a.manifest
                    .as_ref()
                    .map(|m| m.archive_sha256.as_str())
                    .unwrap_or("")
            || op["manifest"] != json!(a.manifest)
        {
            return Err(fenced());
        }
        if !restore["machine_id"].is_null() && restore["machine_id"] != row["provider_sandbox_id"] {
            return Err(fenced());
        }
        if !(row["runtime_state"] == "online"
            && storage_enrolled(&row)
            && row["request_config"]["rustLifecycleOperation"]["id"] == restore["operation_id"]
            && row["request_config"]["rustLifecycleOperation"]["phase"] == "ready")
        {
            lifecycle::require_resume_capacity(
                self.staging_candidate.as_deref(),
                &mut tx,
                &row,
                clock_ms(),
            )
            .await?;
        }
        Ok((tx, row, op, a, restore))
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wake {
    request_id: String,
    state: String,
    receipt: Option<crate::providers::contract::ProviderOperation>,
    /// When this state was recorded; older records start their window on
    /// the next observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recorded_at_ms: Option<i64>,
    /// Which submission this is, starting at 1.
    #[serde(default)]
    submissions: u32,
}
impl RuntimeControl {
    /// Wake the restore target. An unanswered wake is observed by identity
    /// and resubmitted (fresh request id) only once the target is provably
    /// still asleep past the settle window; it is bounded like recovery's.
    pub(super) async fn restore_wake(
        &self,
        provider: &dyn RestoreProvider,
        job: &jobs::Job,
        machine: &crate::providers::Machine,
    ) -> Result<bool, ApiError> {
        use super::super::recovery::{
            EFFECT_MAX_SUBMISSIONS, EFFECT_SETTLE_MS, EFFECT_WAIT_LIMIT_MS,
        };
        use crate::providers::contract::{DesiredState, OperationState, ProviderOperation};
        let (tx, _, _, _, restore) = self.restore_locked(job).await?;
        if restore["machine_id"] != machine.id {
            return Err(fenced());
        }
        let value = &restore["provider_effects"]["wake"];
        let prior: Option<Wake> = if value.is_null() {
            None
        } else {
            Some(serde_json::from_value(value.clone()).map_err(|_| fenced())?)
        };
        let asleep = matches!(
            machine.state,
            MachineState::Standby | MachineState::Paused | MachineState::Stopped
        );
        let mut submissions = 1;
        if let Some(wake) = prior.as_ref().filter(|w| w.state != "rejected") {
            let receipt = wake.receipt.clone().unwrap_or(ProviderOperation {
                resource_id: machine.id.clone(),
                operation_id: None,
                desired: DesiredState::Running,
            });
            if receipt.resource_id != machine.id || receipt.desired != DesiredState::Running {
                return Err(fenced());
            }
            if wake.state == "completed" {
                if machine.state != MachineState::Running {
                    return Err(ApiError::conflict("cloud_restore_target_state_changed"));
                }
                tx.commit().await?;
                return Ok(true);
            }
            tx.commit().await?;
            let observed = provider
                .recovery_state(&receipt)
                .await
                .map_err(|_| ApiError::unavailable("cloud_restore_wake_outcome_unknown"))?;
            let (mut tx, _, _, _, current) = self.restore_locked(job).await?;
            if current["provider_effects"]["wake"]["requestId"] != wake.request_id {
                return Err(fenced());
            }
            match observed {
                OperationState::Completed => {
                    let record = Wake {
                        state: "completed".into(),
                        receipt: Some(receipt),
                        recorded_at_ms: Some(clock_ms()),
                        ..wake.clone()
                    };
                    sqlx::query("UPDATE zuse_rust_restores SET provider_effects=jsonb_set(provider_effects,'{wake}',$2),updated_at=$3 WHERE operation_id=$1")
                        .bind(required(&current,"operation_id")?).bind(json!(record)).bind(clock_ms()).execute(&mut *tx).await?;
                    tx.commit().await?;
                    return Ok(true);
                }
                OperationState::Pending | OperationState::Retrying => {
                    let now = clock_ms();
                    let Some(since) = wake.recorded_at_ms else {
                        let record = Wake {
                            recorded_at_ms: Some(now),
                            ..wake.clone()
                        };
                        sqlx::query("UPDATE zuse_rust_restores SET provider_effects=jsonb_set(provider_effects,'{wake}',$2),updated_at=$3 WHERE operation_id=$1")
                            .bind(required(&current,"operation_id")?).bind(json!(record)).bind(now).execute(&mut *tx).await?;
                        tx.commit().await?;
                        return Ok(false);
                    };
                    let unanswered = wake.receipt.is_none()
                        && observed == OperationState::Pending
                        && matches!(wake.state.as_str(), "uncertain" | "submitting");
                    if unanswered && asleep && now - since >= EFFECT_SETTLE_MS {
                        // Provably did not happen.
                        let made = wake.submissions.max(1);
                        if made >= EFFECT_MAX_SUBMISSIONS {
                            return Err(ApiError::conflict("cloud_restore_wake_failed"));
                        }
                        submissions = made + 1;
                        tracing::warn!(
                            submission = submissions,
                            "Restore wake did not happen; resubmitting"
                        );
                    } else if now - since >= EFFECT_WAIT_LIMIT_MS {
                        return Err(ApiError::conflict("cloud_restore_wake_unconfirmed"));
                    } else {
                        tx.commit().await?;
                        return Ok(false);
                    }
                }
                OperationState::Superseded => {
                    return Err(ApiError::conflict("cloud_restore_wake_superseded"));
                }
            }
            tx.commit().await?;
        } else {
            tx.commit().await?;
        }
        if !asleep {
            return Ok(false);
        }
        let wake = Wake {
            request_id: uuid::Uuid::new_v4().to_string(),
            state: "submitting".into(),
            receipt: None,
            recorded_at_ms: Some(clock_ms()),
            submissions,
        };
        {
            // Compare-and-set against the record this decision was made from.
            let (mut tx, _, _, _, current) = self.restore_locked(job).await?;
            if current["provider_effects"]["wake"]["requestId"] != value["requestId"] {
                return Err(fenced());
            }
            sqlx::query("UPDATE zuse_rust_restores SET provider_effects=jsonb_set(provider_effects,'{wake}',$2),updated_at=$3 WHERE operation_id=$1")
                .bind(required(&current,"operation_id")?).bind(json!(wake)).bind(clock_ms()).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        let result = RecoveryProvider::wake_receipt(provider, &machine.id, &machine.state)
            .await
            .and_then(|r| {
                if r.resource_id == machine.id && r.desired == DesiredState::Running {
                    Ok(r)
                } else {
                    Err(ProviderError::InvalidResponse)
                }
            });
        let record = Wake {
            request_id: wake.request_id.clone(),
            state: match &result {
                Ok(_) => "accepted",
                Err(ProviderError::Rejected) => "rejected",
                Err(_) => "uncertain",
            }
            .into(),
            receipt: result.as_ref().ok().cloned(),
            recorded_at_ms: Some(clock_ms()),
            submissions,
        };
        let saved=sqlx::query("UPDATE zuse_rust_restores SET provider_effects=jsonb_set(provider_effects,'{wake}',$2),updated_at=$3 WHERE operation_id=$1 AND workspace_id=$4 AND generation=$5 AND machine_id=$6 AND provider_effects->'wake'->>'requestId'=$7")
            .bind(required(&restore,"operation_id")?).bind(json!(record)).bind(clock_ms()).bind(&job.resource_id).bind(job.generation).bind(&machine.id).bind(&wake.request_id).execute(&self.pool).await?;
        if saved.rows_affected() != 1 {
            return Err(fenced());
        }
        result.map_err(|_| failed())?;
        Ok(false)
    }
}
