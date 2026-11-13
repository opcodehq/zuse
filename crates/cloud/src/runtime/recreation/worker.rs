//! Bounded reconciliation dispatches the existing durable lifecycle/recovery/
//! restore operations. Provider effects remain in their original receipt ledgers.
use super::*;
impl RuntimeControl {
    pub async fn recreation_once(&self, now: i64) -> Result<bool, ApiError> {
        let ready = self.recreation_schema_present().await?;
        if !ready {
            return Ok(false);
        }
        let operations:Vec<String>=sqlx::query_scalar("SELECT operation_id FROM zuse_rust_computer_recreations WHERE phase NOT IN ('ready','cancelled','attention-required') AND due_at<=$1 ORDER BY due_at,operation_id LIMIT 8").bind(now).fetch_all(&self.pool).await?;
        for operation in &operations {
            if let Err(error) = self.recreation_step(operation, now).await {
                self.recreation_attention(operation, error.code, now)
                    .await?;
            }
        }
        Ok(!operations.is_empty())
    }
    pub(in crate::runtime) async fn recreation_attention(
        &self,
        operation: &str,
        error: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        let workspace: Option<String> = sqlx::query_scalar(
            "SELECT workspace_id FROM zuse_rust_computer_recreations WHERE operation_id=$1",
        )
        .bind(operation)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(workspace) = workspace else {
            return Ok(());
        };
        let (row, _, _) = locked_workspace(&mut tx, &workspace).await?;
        let changed=sqlx::query("UPDATE zuse_rust_computer_recreations SET retry_phase=CASE WHEN phase='attention-required' THEN retry_phase ELSE phase END,phase='attention-required',error_code=$2,updated_at=GREATEST(updated_at+1,$3) WHERE operation_id=$1 AND phase NOT IN ('ready','cancelled')").bind(operation).bind(error).bind(now).execute(&mut *tx).await?;
        if changed.rows_affected() == 0 {
            // A delayed failing delivery must not close execution again after
            // another worker completed/cancelled this exact operation.
            tx.commit().await?;
            return Ok(());
        }
        if row["request_config"]["rustRecreation"]["operationId"] == operation
            && !lifecycle::deleted(&row)
        {
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRecreation,phase}','\"attention-required\"'::jsonb),status_code=$3,revision=revision+1,updated_at=GREATEST(updated_at+1,$2) WHERE workspace_id=$1").bind(&workspace).bind(now).bind(error).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    async fn recreation_step(&self, operation: &str, now: i64) -> Result<(), ApiError> {
        let workspace:Option<String>=sqlx::query_scalar("SELECT workspace_id FROM zuse_rust_computer_recreations WHERE operation_id=$1 AND phase NOT IN ('ready','cancelled','attention-required')").bind(operation).fetch_optional(&self.pool).await?;
        let Some(workspace) = workspace else {
            return Ok(());
        };
        let actor: Option<String> = sqlx::query_scalar(
            "SELECT actor_id FROM zuse_rust_computer_recreations WHERE operation_id=$1",
        )
        .bind(operation)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(actor) = actor {
            self.scope_access(&workspace, Some(&actor), true).await?;
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR UPDATE").bind(&workspace).fetch_optional(&mut *tx).await?;
        let (row, generation, _) = locked_workspace(&mut tx, &workspace).await?;
        let op:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_computer_recreations r WHERE operation_id=$1 FOR UPDATE").bind(operation).fetch_one(&mut *tx).await?;
        if matches!(
            op["phase"].as_str(),
            Some("ready" | "cancelled" | "attention-required")
        ) {
            return Ok(());
        }
        if lifecycle::deleted(&row) {
            sqlx::query("UPDATE zuse_rust_computer_recreations SET phase='cancelled',updated_at=$2 WHERE operation_id=$1").bind(operation).bind(now).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(());
        }
        let restoring = matches!(
            op["phase"].as_str(),
            Some("restoring" | "waiting-for-runtime")
        );
        let expected = if restoring {
            op["restore_generation"].as_i64()
        } else {
            op["source_generation"].as_i64()
        };
        if Some(generation) != expected
            || row["account_id"] != op["owner_id"]
            || row["provider"] != op["provider_id"]
            // The source release until the restore is queued; after that an
            // "Update computer" recreation is pinned to its target release.
            // A staging-candidate target also binds the exact candidate.
            || (row["managed_release_id"] != op["managed_release_id"]
                && !(restoring
                    && super::target_release(&row)
                        .is_some_and(|target| row["managed_release_id"] == target)
                    && super::target_candidate(&row).is_none_or(|candidate| {
                        row["request_config"]["managedCandidateSha256"] == candidate
                    })))
            || row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
                != op["storage_incarnation_id"]
            || row["request_config"]["rustRecreation"]["operationId"] != operation
        {
            return Err(fenced());
        }
        if !restoring
            && !row["provider_sandbox_id"].is_null()
            && row["provider_sandbox_id"] != op["source_machine_id"]
        {
            return Err(fenced());
        }
        let failed:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE resource_id=$1 AND state='failed' AND payload->>'operationId'=ANY($2))").bind(&workspace).bind(vec![op["wake_operation_id"].as_str().unwrap_or("").to_owned(),op["pause_operation_id"].as_str().unwrap_or("").to_owned(),op["archive_operation_id"].as_str().unwrap_or("").to_owned(),op["restore_operation_id"].as_str().unwrap_or("").to_owned()]).fetch_one(&mut *tx).await?;
        if failed {
            return Err(ApiError::conflict("computer_recreation_stage_failed"));
        }
        sqlx::query("UPDATE zuse_rust_computer_recreations SET due_at=$2 WHERE operation_id=$1")
            .bind(operation)
            .bind(now + 1000)
            .execute(&mut *tx)
            .await?;
        match op["phase"].as_str() {
            Some("waking") => {
                if row["state"] != "ready"
                    || row["runtime_state"] != "online"
                    || row["request_config"]["rustLifecycleOperation"]["id"]
                        != op["wake_operation_id"]
                    || row["request_config"]["rustLifecycleOperation"]["phase"] != "ready"
                {
                    tx.commit().await?;
                    return Ok(());
                }
                // A Ready ACK can precede the original wake worker's terminal
                // receipt. Keep its current pointer until that worker finishes;
                // otherwise switching to fullPause would fence the old job.
                let wake_state:Option<String>=sqlx::query_scalar("SELECT state FROM zuse_rust_jobs WHERE kind=$1 AND resource_id=$2 AND resource_generation=$3 AND payload->>'operationId'=$4")
                    .bind(lifecycle::JOB_KIND).bind(&workspace).bind(generation).bind(op["wake_operation_id"].as_str()).fetch_optional(&mut *tx).await?;
                match wake_state.as_deref() {
                    Some("completed") => {}
                    Some("pending" | "running") => {
                        sqlx::query("UPDATE zuse_rust_jobs SET due_at=LEAST(due_at,$2) WHERE kind=$1 AND resource_id=$3 AND resource_generation=$4 AND payload->>'operationId'=$5 AND state='pending'")
                            .bind(lifecycle::JOB_KIND).bind(now).bind(&workspace).bind(generation).bind(op["wake_operation_id"].as_str()).execute(&mut *tx).await?;
                        tx.commit().await?;
                        lifecycle::wakeup::send();
                        return Ok(());
                    }
                    _ => {
                        return Err(ApiError::conflict(
                            "computer_recreation_wake_receipt_required",
                        ));
                    }
                }
                let pause = required(&op, "pause_operation_id")?;
                let mut config = row["request_config"].clone();
                config["rustLifecycleOperation"] =
                    json!({"id":pause,"action":"pause","phase":"quiescing"});
                sqlx::query("UPDATE api_cloud_workspaces SET state='pausing',desired_state='paused',request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3),next_action_at=$3 WHERE workspace_id=$1").bind(&workspace).bind(config).bind(now).execute(&mut *tx).await?;
                let job = jobs::enqueue(
                    &mut tx,
                    &format!("lifecycle:{pause}"),
                    lifecycle::JOB_KIND,
                    &workspace,
                    generation,
                    &json!({"operationId":pause}),
                    now,
                )
                .await?;
                sqlx::query("UPDATE zuse_rust_jobs SET max_attempts=120 WHERE job_id=$1")
                    .bind(job)
                    .execute(&mut *tx)
                    .await?;
                transition(&mut tx, &row, &op, "quiescing", now).await?;
            }

            Some("quiescing") => {
                if row["state"] != "paused"
                    || row["desired_state"] != "paused"
                    || row["request_config"]["rustLifecycleOperation"]["id"]
                        != op["pause_operation_id"]
                    || row["request_config"]["rustLifecycleOperation"]["phase"] != "paused"
                {
                    tx.commit().await?;
                    return Ok(());
                }
                let artifact = self.recreation_source(&mut tx, &row, now).await?;
                if artifact != op["source_artifact_id"] {
                    return Err(fenced());
                }
                transition(&mut tx, &row, &op, "capturing", now).await?;
                tx.commit().await?;
                self.begin_recovery(&workspace, now).await?;
                return Ok(());
            }
            Some("capturing") => {
                let archive = match recovery::archive(&row) {
                    Ok(archive) => archive,
                    Err(_) => {
                        tx.commit().await?;
                        self.begin_recovery(&workspace, now).await?;
                        return Ok(());
                    }
                };
                if archive.source_sandbox_id != op["source_machine_id"]
                    || (!op["archive_operation_id"].is_null()
                        && op["archive_operation_id"] != archive.id)
                {
                    return Err(fenced());
                }
                sqlx::query("UPDATE zuse_rust_computer_recreations SET archive_operation_id=COALESCE(archive_operation_id,$2) WHERE operation_id=$1").bind(operation).bind(&archive.id).execute(&mut *tx).await?;
                if archive.backup_verified() {
                    let digest = &archive.manifest.as_ref().ok_or_else(fenced)?.archive_sha256;
                    sqlx::query("UPDATE zuse_rust_computer_recreations SET verified_archive_sha256=$2 WHERE operation_id=$1").bind(operation).bind(digest).execute(&mut *tx).await?;
                    transition(&mut tx, &row, &op, "retiring", now).await?;
                }
            }
            Some("retiring") => {
                let archive = recovery::archive(&row)?;
                if archive.id != op["archive_operation_id"]
                    || !archive.backup_verified()
                    || archive.manifest.as_ref().is_none_or(|manifest| {
                        op["verified_archive_sha256"] != manifest.archive_sha256
                    })
                {
                    return Err(fenced());
                }
                if row["provider_sandbox_id"].is_null() && archive.phase == ArchivePhase::Retired {
                    let artifact = self.recreation_source(&mut tx, &row, now).await?;
                    if artifact != op["source_artifact_id"] {
                        return Err(fenced());
                    }
                    // "Update computer": the original is gone and the verified
                    // capture is about to land on the target base, so repin
                    // the computer there in the same transaction. Restore then
                    // resolves (and boot-verifies) the new base like any other.
                    // A staging-candidate computer moves onto the exact
                    // candidate pin chosen at admission, still current.
                    let row = match super::target_release(&row) {
                        Some(target)
                            if row["managed_release_id"] != target
                                || super::target_candidate(&row).is_some_and(|candidate| {
                                    row["request_config"]["managedCandidateSha256"] != candidate
                                }) =>
                        {
                            let release = self.chosen_target(&mut tx, &row, target, now).await?;
                            if release.artifact_id != artifact {
                                return Err(fenced());
                            }
                            let mut config = row["request_config"].clone();
                            super::repin_config(&mut config, &release.settings)?;
                            sqlx::query("UPDATE api_cloud_workspaces SET managed_release_id=$2,request_config=$3,revision=revision+1,updated_at=GREATEST(updated_at+1,$4) WHERE workspace_id=$1").bind(&workspace).bind(&release.release_id).bind(config).bind(now).execute(&mut *tx).await?;
                            locked_workspace(&mut tx, &workspace).await?.0
                        }
                        _ => row,
                    };
                    self.queue_restore_tx(
                        &mut tx,
                        &row,
                        &format!("recreate:{operation}:restore"),
                        now,
                    )
                    .await?;
                    let (current, next_generation, _) =
                        locked_workspace(&mut tx, &workspace).await?;
                    if next_generation != generation.checked_add(1).ok_or_else(fenced)? {
                        return Err(fenced());
                    }
                    sqlx::query("UPDATE zuse_rust_computer_recreations SET restore_operation_id=$2,restore_generation=$3 WHERE operation_id=$1").bind(operation).bind(current["request_config"]["rustRestoreOperationId"].as_str()).bind(next_generation).execute(&mut *tx).await?;
                    transition(&mut tx, &current, &op, "restoring", now).await?;
                }
            }
            Some("restoring" | "waiting-for-runtime") => {
                if row["request_config"]["rustRestoreOperationId"] != op["restore_operation_id"] {
                    return Err(fenced());
                }
                let restore:Value=sqlx::query_scalar("SELECT to_jsonb(r) FROM zuse_rust_restores r WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 FOR SHARE").bind(op["restore_operation_id"].as_str()).bind(&workspace).bind(generation).fetch_one(&mut *tx).await?;
                if !row["provider_sandbox_id"].is_null()
                    && row["provider_sandbox_id"] != restore["machine_id"]
                {
                    return Err(fenced());
                }
                if restore["phase"] == "ready"
                    && row["runtime_state"] == "online"
                    && storage_enrolled(&row)
                    && row["request_config"]["rustLifecycleOperation"]["phase"] == "ready"
                {
                    transition(&mut tx, &row, &op, "ready", now).await?;
                } else if matches!(restore["phase"].as_str(), Some("restored" | "launching"))
                    && op["phase"] != "waiting-for-runtime"
                {
                    transition(&mut tx, &row, &op, "waiting-for-runtime", now).await?;
                }
            }
            _ => return Err(fenced()),
        }
        tx.commit().await?;
        Ok(())
    }
}
pub(super) async fn transition(
    tx: &mut Transaction<'_, Postgres>,
    row: &Value,
    op: &Value,
    next: &str,
    now: i64,
) -> Result<(), ApiError> {
    sqlx::query("UPDATE zuse_rust_computer_recreations SET phase=$2,retry_phase=NULL,error_code=NULL,due_at=$3,updated_at=GREATEST(updated_at+1,$3) WHERE operation_id=$1").bind(required(op,"operation_id")?).bind(next).bind(now).execute(&mut **tx).await?;
    // jsonb_set operates on CURRENT config, preserving restore generation/grants
    // written in this transaction rather than replaying a stale source snapshot.
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=jsonb_set(request_config,'{rustRecreation,phase}',to_jsonb($2::text)),status_code=$3,revision=revision+1,updated_at=GREATEST(updated_at+1,$4) WHERE workspace_id=$1 AND request_config->'rustRecreation'->>'operationId'=$5").bind(required(row,"workspace_id")?).bind(next).bind(format!("computer-recreate-{next}")).bind(now).bind(required(op,"operation_id")?).execute(&mut **tx).await?;
    Ok(())
}
