//! Explicit command wake shares the command transaction. No network/provider
//! operation runs under locks, no new allocation and no automatic Git changes.
use super::*;
impl RuntimeControl {
    /// Caller retains control-owner SHARE and workspace command lock and has
    /// checked fresh actor/sharing authority before admitting this command.
    pub(in crate::runtime) async fn task_command_wake_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        command: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        let id = required(row, "workspace_id")?;
        if deleted(row)
            || super::super::repair::leases_blocked(row)
            || super::super::recreation::leases_blocked(row)
        {
            return Err(ApiError::conflict("cloud_workspace_unavailable"));
        }
        // Already allocated/starting work is never reallocated by a follow-up.
        if row["desired_state"] == "ready"
            && matches!(
                row["state"].as_str(),
                Some("ready" | "queued" | "provisioning" | "starting")
            )
        {
            require_resume_capacity(self.staging_candidate.as_deref(), tx, row, now).await?;
            return Ok(json!({"state":if row["state"]=="ready"{"ready"}else{"starting"}}));
        }
        if row["state"] != "paused" || row["desired_state"] != "paused" {
            return Err(ApiError::conflict("cloud_lifecycle_transition_unavailable"));
        }
        let config = &row["request_config"];
        if !storage_enrolled(row)
            || !config["rustStorageEnrollment"]["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(|v| v == CAPABILITY))
        {
            return Err(ApiError::conflict(
                "cloud_lifecycle_runtime_update_required",
            ));
        }
        if config["rustNativeTranscriptEnabled"] == true {
            if self.transcript_objects.is_none() {
                return Err(ApiError::unavailable("cloud_transcript_store_unavailable"));
            }
            if !config["rustStorageEnrollment"]["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(|v| v == "rust-transcript-projection-v1"))
            {
                return Err(ApiError::conflict(
                    "cloud_lifecycle_runtime_update_required",
                ));
            }
        }
        let previous = current(tx, row).await?;
        if previous["phase"] != "paused" {
            return Err(ApiError::conflict("cloud_lifecycle_quiescence_required"));
        }
        require_resume_capacity(self.staging_candidate.as_deref(), tx, row, now).await?;
        let generation = config["runtimeGeneration"]
            .as_i64()
            .filter(|g| *g > 0)
            .ok_or_else(|| ApiError::conflict("cloud_lifecycle_generation_invalid"))?;
        let operation = uuid::Uuid::new_v4().to_string();
        let wake_key = format!("task-wake:{}", hash(command));
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'resume','waking',$7,$7)")
            .bind(&operation).bind(id).bind(&wake_key).bind(generation).bind(required(row,"provider_sandbox_id")?).bind(required(&config["rustRuntimeStorage"],"storageIncarnationId")?).bind(now).execute(&mut **tx).await?;
        let mut updated = config.clone();
        progress(&mut updated, &operation, Action::Resume, "waking");
        sqlx::query("UPDATE api_cloud_workspaces SET state='provisioning',desired_state='ready',status_code='resume-waking',request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3),next_action_at=$3 WHERE workspace_id=$1")
            .bind(id).bind(updated).bind(now).execute(&mut **tx).await?;
        let job = jobs::enqueue(
            tx,
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
            .execute(&mut **tx)
            .await?;
        Ok(json!({"state":"waking","operationId":operation}))
    }
}
