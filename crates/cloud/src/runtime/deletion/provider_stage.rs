//! Provider reads, deletion and receipt observation happen outside database transactions.
use super::*;

impl RuntimeControl {
    async fn delete_locked(
        &self,
        job: &jobs::Job,
    ) -> Result<(Transaction<'static, Postgres>, Value, Value), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        recovery::active_job(&mut tx, job).await?;
        if generation != job.generation || row["desired_state"] != "deleted" {
            return Err(fenced());
        }
        let deletion:Value=sqlx::query_scalar("SELECT to_jsonb(d) FROM zuse_rust_deletions d WHERE workspace_id=$1 AND operation_id=$2 AND generation=$3 FOR UPDATE")
            .bind(&job.resource_id).bind(job.payload["operationId"].as_str()).bind(job.generation).fetch_one(&mut *tx).await?;
        Ok((tx, row, deletion))
    }

    async fn delete_targets_saved(
        &self,
        job: &jobs::Job,
        targets: &[Target],
        next: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let (mut tx, _, deletion) = self.delete_locked(job).await?;
        if deletion["phase"] != "providers" {
            return Err(fenced());
        }
        sqlx::query("UPDATE zuse_rust_deletions SET targets=$2,phase=$3 WHERE workspace_id=$1")
            .bind(&job.resource_id)
            .bind(json!(targets))
            .bind(next)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE api_cloud_workspaces SET status_code=$2,request_config=jsonb_set(request_config,'{rustDeleteOperation}',$3),revision=revision+1,updated_at=GREATEST(updated_at+1,$4) WHERE workspace_id=$1")
            .bind(&job.resource_id).bind(if next=="objects" {"delete-purging-data"} else {"delete-provider-cleanup"})
            .bind(json!({"id":deletion["operation_id"],"phase":next})).bind(now).execute(&mut *tx).await?;
        recovery::defer(&mut tx, job, false, now).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn admit_delete_target(
        &self,
        job: &jobs::Job,
        targets: &[Target],
        id: &str,
    ) -> Result<(), ApiError> {
        let (mut tx, row, deletion) = self.delete_locked(job).await?;
        if deletion["phase"] != "providers" {
            return Err(fenced());
        }
        let shared:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE workspace_id<>$1 AND provider=$2 AND provider_sandbox_id=$3 AND state<>'deleted')")
            .bind(&job.resource_id).bind(required(&row,"provider")?).bind(id).fetch_one(&mut *tx).await?;
        let persistent:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_machines m WHERE to_jsonb(m)->>'provider_sandbox_id'=$1 AND state NOT IN ('deleted','destroyed'))")
            .bind(id).fetch_one(&mut *tx).await?;
        if shared || persistent {
            return Err(ApiError::conflict("cloud_delete_source_shared"));
        }
        sqlx::query("UPDATE zuse_rust_deletions SET targets=$2 WHERE workspace_id=$1")
            .bind(&job.resource_id)
            .bind(json!(targets))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn save_delete_receipt(
        &self,
        job: &jobs::Job,
        id: &str,
        result: &Result<ProviderOperation, ProviderError>,
    ) -> Result<(), ApiError> {
        let update = json!({"delete_submitted":!matches!(result,Err(ProviderError::Rejected)),"delete_receipt":result.as_ref().ok()});
        // Preserve the original receipt even after job lease expiry. Merge only its
        // exact target rather than overwriting another worker's observed targets.
        sqlx::query("UPDATE zuse_rust_deletions SET targets=(SELECT COALESCE(jsonb_agg(CASE WHEN item->>'id'=$4 THEN item||$5 ELSE item END ORDER BY ordinal),'[]') FROM jsonb_array_elements(targets) WITH ORDINALITY AS t(item,ordinal)) WHERE workspace_id=$1 AND operation_id=$2 AND generation=$3")
            .bind(&job.resource_id).bind(job.payload["operationId"].as_str()).bind(job.generation).bind(id).bind(update).execute(&self.pool).await?;
        Ok(())
    }

    pub(super) async fn delete_provider_stage(
        &self,
        provider: &dyn DeletionProvider,
        job: &jobs::Job,
        mut targets: Vec<Target>,
        now: i64,
    ) -> Result<(), ApiError> {
        if let Some(index) = targets.iter().position(|t| !t.done) {
            let target = &targets[index];
            if target.delete_submitted {
                let id = target.id.as_deref().ok_or_else(fenced)?;
                let receipt = target.delete_receipt.clone().unwrap_or(ProviderOperation {
                    resource_id: id.into(),
                    operation_id: None,
                    desired: DesiredState::Deleted,
                });
                if receipt.resource_id != id || receipt.desired != DesiredState::Deleted {
                    return Err(fenced());
                }
                match provider
                    .receipt_state(&receipt)
                    .await
                    .map_err(|_| failed())?
                {
                    OperationState::Completed => targets[index].done = true,
                    OperationState::Pending | OperationState::Retrying => {}
                    OperationState::Superseded => {
                        return Err(ApiError::conflict("cloud_provider_operation_superseded"));
                    }
                }
            } else {
                let machine = provider
                    .find_target(target.id.as_deref(), target.name.as_deref())
                    .await
                    .map_err(|_| failed())?;
                match machine {
                    Some(machine) => {
                        if target.id.as_ref().is_some_and(|id| id != &machine.id)
                            || target
                                .name
                                .as_ref()
                                .is_some_and(|name| name != &machine.name)
                        {
                            return Err(fenced());
                        }
                        targets[index].id = Some(machine.id.clone());
                        targets[index].delete_submitted = true;
                        // Commit the exact ID, sharing checks and submission before deletion.
                        self.admit_delete_target(job, &targets, &machine.id).await?;
                        let result = provider.destroy_receipt(&machine.id).await.and_then(|r| {
                            if r.resource_id == machine.id && r.desired == DesiredState::Deleted {
                                Ok(r)
                            } else {
                                Err(ProviderError::InvalidResponse)
                            }
                        });
                        self.save_delete_receipt(job, &machine.id, &result).await?;
                        let receipt = result.map_err(|_| failed())?;
                        targets[index].delete_receipt = Some(receipt);
                    }
                    None if target.uncertain && target.id.is_none() => {
                        return Err(ApiError::unavailable(
                            "cloud_delete_allocation_outcome_unknown",
                        ));
                    }
                    None => targets[index].done = true,
                }
            }
        }
        let next = if targets.iter().all(|t| t.done) {
            "objects"
        } else {
            "providers"
        };
        self.delete_targets_saved(job, &targets, next, now).await
    }

    pub(super) async fn delete_objects_stage(
        &self,
        job: &jobs::Job,
        now: i64,
    ) -> Result<(), ApiError> {
        let objects = self.transcript_objects.as_ref().ok_or_else(failed)?;
        let prefix = format!(
            "workspaces/{}/",
            crate::objects::encode(&job.resource_id, false)
        );
        let purged = objects.purge_prefix(&prefix).await?;
        let (mut tx, _, deletion) = self.delete_locked(job).await?;
        if deletion["phase"] != "objects" {
            return Err(fenced());
        }
        if purged {
            sqlx::query("UPDATE zuse_rust_deletions SET phase='metadata' WHERE workspace_id=$1")
                .bind(&job.resource_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("UPDATE api_cloud_workspaces SET status_code='delete-clearing-metadata',request_config=jsonb_set(request_config,'{rustDeleteOperation}',$2),revision=revision+1 WHERE workspace_id=$1")
                .bind(&job.resource_id).bind(json!({"id":deletion["operation_id"],"phase":"metadata"})).execute(&mut *tx).await?;
        }
        recovery::defer(&mut tx, job, false, now).await?;
        tx.commit().await?;
        Ok(())
    }
}
