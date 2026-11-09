//! Admit idle sleep through the same retained lifecycle as an explicit Sleep.
//! Provider effects still require guest quiescence and the durable worker lease.
use super::*;

fn eligible(row: &Value) -> bool {
    let config = &row["request_config"];
    row["state"] == "ready"
        && row["desired_state"] == "ready"
        && row["runtime_state"] == "online"
        && task_commands::command_mode(row)
        && native_commands_supported(row)
        && storage_enrolled(row)
        && config["rustStorageEnrollment"]["capabilities"]
            .as_array()
            .is_some_and(|caps| caps.iter().any(|cap| cap == CAPABILITY))
        && config["runtimeReleaseChangeRequested"] != true
        && !matches!(
            config["runtimeActivation"]["phase"].as_str(),
            Some("preparing" | "launching" | "confirming" | "rolling-back")
        )
        && config["rustLifecycleOperation"]["phase"]
            .as_str()
            .is_none_or(|phase| phase == "ready")
        && !super::super::recreation::leases_blocked(row)
}

impl RuntimeControl {
    /// Admit at most one due computer per worker round. Empty scans are paced
    /// by the worker; an open event subscription is not user activity.
    pub async fn idle_sleep_once(&self, now: i64) -> Result<bool, ApiError> {
        if self.creation_accounts.is_empty() || !self.task_command_delivery_available().await? {
            return Ok(false);
        }
        // Coalesced terminal writes must each receive at least the full idle
        // interval. Older guests already have durable command timestamps, so
        // upgrading the API cannot mistake a recent completed turn for idle.
        let cutoff = now
            .saturating_sub(self.idle_ms)
            .saturating_sub(crate::runtime::activity::COALESCE_MS);
        let candidate: Option<String> = sqlx::query_scalar(include_str!("idle_candidates.sql"))
            .bind(&self.creation_accounts)
            .bind(cutoff)
            .bind(now)
            .bind(zuse_protocol::cloud_task_commands::MODE)
            .fetch_optional(&self.pool)
            .await?;
        let Some(id) = candidate else {
            return Ok(false);
        };
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locking::workspace(&mut tx, &id, true).await?;
        if !eligible(&row)
            || !self
                .creation_accounts
                .iter()
                .any(|owner| row["account_id"] == *owner)
            || row["last_activity_at"]
                .as_i64()
                .is_none_or(|at| at > cutoff)
            || row["next_action_at"].as_i64().is_none_or(|at| at > now)
        {
            return Ok(false);
        }
        let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspace_launch_intents WHERE workspace_id=$1) OR EXISTS(SELECT 1 FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND (state IN('queued','starting','running','waiting-input','outcome-unknown') OR updated_at>$2))")
            .bind(&id).bind(cutoff).fetch_one(&mut *tx).await?;
        if busy {
            return Ok(false);
        }
        let operation = uuid::Uuid::new_v4().to_string();
        let mut config = row["request_config"].clone();
        progress(&mut config, &operation, Action::Pause, "quiescing");
        config["rustLifecycleOperation"]["reason"] = json!("idle-timeout");
        let command = format!(
            "idle:{generation}:{}",
            row["next_action_at"].as_i64().unwrap_or(0)
        );
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'pause','quiescing',$7,$7)")
            .bind(&operation).bind(&id).bind(command).bind(generation).bind(required(&row,"provider_sandbox_id")?)
            .bind(required(&config["rustRuntimeStorage"],"storageIncarnationId")?).bind(now).execute(&mut *tx).await?;
        sqlx::query("UPDATE api_cloud_workspaces SET state='pausing',desired_state='paused',status_code='pause-quiescing',request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3),next_action_at=$3 WHERE workspace_id=$1")
            .bind(&id).bind(config).bind(now).execute(&mut *tx).await?;
        let job = jobs::enqueue(
            &mut tx,
            &format!("lifecycle:{operation}"),
            JOB_KIND,
            &id,
            generation,
            &json!({"operationId":operation}),
            now,
        )
        .await?;
        sqlx::query("UPDATE zuse_rust_jobs SET max_attempts=120 WHERE job_id=$1")
            .bind(job)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        wakeup::send();
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
