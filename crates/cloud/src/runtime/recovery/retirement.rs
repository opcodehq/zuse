//! A verified archive precedes provider retirement; provider calls never hold a database lock.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Retirement {
    request_id: String,
    state: String,
    receipt: Option<ProviderOperation>,
}
impl RuntimeControl {
    async fn retirement_locked(
        &self,
        job: &jobs::Job,
    ) -> Result<
        (
            Transaction<'static, Postgres>,
            Value,
            Value,
            RecoveryArchive,
        ),
        ApiError,
    > {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        active_job(&mut tx, job).await?;
        let op = locked_op(&mut tx, &row).await?;
        let a = archive(&row)?;
        if generation != job.generation
            || op["operation_id"] != job.payload["operationId"]
            || op["phase"] != "retiring"
            || op["verified_at"].is_null()
            || !a.backup_verified()
            || a.manifest.as_ref().is_none_or(|m| {
                op["verified_archive_sha256"] != m.archive_sha256 || op["manifest"] != json!(m)
            })
        {
            return Err(fenced());
        }
        let shared:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE workspace_id<>$1 AND provider=$2 AND provider_sandbox_id=$3 AND state<>'deleted')")
            .bind(&job.resource_id).bind(required(&row,"provider")?).bind(&a.source_sandbox_id).fetch_one(&mut *tx).await?;
        let persistent:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_machines m WHERE to_jsonb(m)->>'provider_sandbox_id'=$1 AND state NOT IN ('deleted','destroyed'))")
            .bind(&a.source_sandbox_id).fetch_one(&mut *tx).await?;
        if shared || persistent {
            return Err(ApiError::conflict("archive-source-shared"));
        }
        self.recreation_pre_retirement(&mut tx, &row, &a, clock_ms())
            .await?;
        Ok((tx, row, op, a))
    }

    async fn retirement_defer(&self, job: &jobs::Job) -> Result<(), ApiError> {
        let (mut tx, _, _, _) = self.retirement_locked(job).await?;
        defer(&mut tx, job, false, clock_ms()).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn retirement_complete(&self, job: &jobs::Job) -> Result<(), ApiError> {
        let (mut tx, row, op, mut a) = self.retirement_locked(job).await?;
        let now = clock_ms();
        observe(&mut tx, &row, &op, None, now).await?;
        a.phase = ArchivePhase::Retired;
        sqlx::query("UPDATE zuse_rust_recovery_operations SET phase='retired',updated_at=$2 WHERE operation_id=$1")
            .bind(&a.id).bind(now).execute(&mut *tx).await?;
        save(&mut tx, &row, &a, "archive-retired", now).await?;
        sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id=NULL,runtime_state='offline',runtime_credential_hash=NULL,runtime_boot_token_hash=NULL,runtime_boot_token_expires_at=NULL,state=CASE WHEN desired_state='archived' THEN 'archived' ELSE 'paused' END,next_action_at=9007199254740991 WHERE workspace_id=$1")
            .bind(&job.resource_id).execute(&mut *tx).await?;
        defer(
            &mut tx,
            job,
            !row["request_config"]["rustRestoreRequested"].is_string(),
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub(super) async fn recovery_retirement(
        &self,
        provider: &dyn RecoveryProvider,
        job: &jobs::Job,
        _now: i64,
    ) -> Result<(), ApiError> {
        // The caller keeps the execution guard across this whole stage. Every
        // database transaction below is committed before provider I/O.
        let (tx, row, op, a) = self.retirement_locked(job).await?;
        if row["provider"] != provider.provider_id() {
            return Err(ApiError::conflict("cloud_provider_identity_mismatch"));
        }
        tx.commit().await?;
        let prior = &op["provider_effects"]["retirement"];
        let prior: Option<Retirement> = if prior.is_null() {
            None
        } else {
            Some(serde_json::from_value(prior.clone()).map_err(|_| fenced())?)
        };
        if let Some(effect) = prior.as_ref().filter(|e| e.state != "rejected") {
            let receipt = effect.receipt.clone().unwrap_or(ProviderOperation {
                resource_id: a.source_sandbox_id.clone(),
                operation_id: None,
                desired: DesiredState::Deleted,
            });
            if receipt.resource_id != a.source_sandbox_id
                || receipt.desired != DesiredState::Deleted
            {
                return Err(fenced());
            }
            match provider
                .retirement_state(&receipt)
                .await
                .map_err(|_| ApiError::unavailable("archive-retirement-outcome-unknown"))?
            {
                OperationState::Completed => return self.retirement_complete(job).await,
                OperationState::Pending | OperationState::Retrying => {
                    return self.retirement_defer(job).await;
                }
                OperationState::Superseded => {
                    return Err(ApiError::conflict("archive-retirement-superseded"));
                }
            }
        }
        let machine = provider
            .inspect(&a.source_sandbox_id)
            .await
            .map_err(|_| failed())?;
        let Some(machine) = machine else {
            return self.retirement_complete(job).await;
        };
        if machine.id != a.source_sandbox_id {
            return Err(fenced());
        }
        let (mut tx, _, op, current) = self.retirement_locked(job).await?;
        if current.source_sandbox_id != machine.id {
            return Err(fenced());
        }
        let value = &op["provider_effects"]["retirement"];
        if !value.is_null() && value["state"] != "rejected" {
            // Another admitted attempt already owns this mutation. Observe it
            // next delivery instead of dispatching a second retirement.
            defer(&mut tx, job, false, clock_ms()).await?;
            tx.commit().await?;
            return Ok(());
        }
        let effect = Retirement {
            request_id: uuid::Uuid::new_v4().to_string(),
            state: "submitting".into(),
            receipt: None,
        };
        sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,'{retirement}',$2),updated_at=$3 WHERE operation_id=$1")
            .bind(&current.id).bind(json!(effect)).bind(clock_ms()).execute(&mut *tx).await?;
        tx.commit().await?;
        let result = provider.retire_receipt(&machine.id).await.and_then(|r| {
            if r.resource_id == machine.id && r.desired == DesiredState::Deleted {
                Ok(r)
            } else {
                Err(ProviderError::InvalidResponse)
            }
        });
        let record = Retirement {
            request_id: effect.request_id.clone(),
            state: match &result {
                Ok(_) => "accepted",
                Err(ProviderError::Rejected) => "rejected",
                Err(_) => "uncertain",
            }
            .into(),
            receipt: result.as_ref().ok().cloned(),
        };
        // The receipt belongs to the immutable original source, even if this
        // delivery expired or a concurrent explicit delete fenced its worker.
        let saved=sqlx::query("UPDATE zuse_rust_recovery_operations SET provider_effects=jsonb_set(provider_effects,'{retirement}',$2),updated_at=$3 WHERE operation_id=$1 AND workspace_id=$4 AND generation=$5 AND source_machine_id=$6 AND provider_effects->'retirement'->>'requestId'=$7")
            .bind(&current.id).bind(json!(record)).bind(clock_ms()).bind(&job.resource_id).bind(job.generation).bind(&machine.id).bind(&effect.request_id).execute(&self.pool).await?;
        if saved.rows_affected() != 1 {
            return Err(fenced());
        }
        result.map_err(|_| ApiError::unavailable("archive-retirement-failed"))?;
        self.retirement_defer(job).await
    }
}
