//! Claim the final queued revision only after reauthorizing its original author.
//! Jobs survive missed in-process hints; no object or identity HTTP holds SQL locks.
use super::*;
// Live message-bearing Steer inference uses the control transport lane;
// its lifetime, rather than its delivery ACK, governs conversation controls.
use zuse_protocol::session_commands::SessionCommandPayload;
impl RuntimeControl {
    pub(in crate::runtime) async fn enqueue_task_drain_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        session: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        if !command_mode(row) {
            return Ok(());
        }
        sqlx::query("INSERT INTO zuse_rust_task_command_drains(workspace_id,session_id,revision,next_attempt_at) VALUES($1,$2,1,$3) ON CONFLICT(workspace_id,session_id) DO UPDATE SET revision=zuse_rust_task_command_drains.revision+1,next_attempt_at=EXCLUDED.next_attempt_at,last_error=NULL")
            .bind(required(row,"workspace_id")?).bind(session).bind(now).execute(&mut **tx).await?;
        Ok(())
    }
    /// The computer can take more now (its runtime update committed, or a
    /// recreation finished): every conversation with queued messages tries
    /// again at once instead of at its next retry.
    pub(in crate::runtime) async fn resume_waiting_queues_tx(
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        if !command_mode(row) {
            return Ok(());
        }
        sqlx::query("INSERT INTO zuse_rust_task_command_drains(workspace_id,session_id,revision,next_attempt_at) SELECT DISTINCT workspace_id,session_id,1,$2::bigint FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND state='queued' ON CONFLICT(workspace_id,session_id) DO UPDATE SET revision=zuse_rust_task_command_drains.revision+1,next_attempt_at=EXCLUDED.next_attempt_at")
            .bind(required(row, "workspace_id")?).bind(now).execute(&mut **tx).await?;
        Ok(())
    }
    pub(in crate::runtime) fn notify_task_drain(&self) {
        let _ = self.mailbox_changes.send(());
        super::super::lifecycle::wakeup::send();
    }
    pub(super) async fn queued_run_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        session: &str,
        message: &str,
        input: &wire::Input,
    ) -> Result<SessionCommandPayload, ApiError> {
        validation::input(input)?;
        if input.as_goal {
            return Err(error("cloud_task_queue_goal_execution_unavailable"));
        }
        if input.skill_refs.iter().any(|s| !validation::relative(s)) {
            return Err(error("cloud_task_queue_relative_reference_required"));
        }
        let chat = self
            .task_registered_session_tx(tx, row, session)
            .await?
            .ok_or_else(|| error("cloud_task_session_not_registered"))?;
        let saved_config = &chat["chat"]["config"];
        let selected_config = input
            .run_selection
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|_| error("cloud_task_queue_selection_invalid"))?;
        let config = selected_config.as_ref().unwrap_or(saved_config);
        let (policy, _) = self.task_policy_tx(tx, row).await?;
        let (default_sandbox, _) = match policy.runtime_mode {
            zuse_protocol::cloud::CloudRuntimeMode::FullAccess => ("danger-full-access", true),
            zuse_protocol::cloud::CloudRuntimeMode::Auto => ("auto", false),
            _ => ("workspace-write", false),
        };
        let sandbox = input
            .run_selection
            .as_ref()
            .map(|v| serde_json::to_value(v.sandbox).unwrap_or(Value::Null))
            .unwrap_or(json!(default_sandbox));
        let auto = sandbox == "danger-full-access";
        let mut prompt = input.text.clone();
        if !input.file_refs.is_empty()
            || !input.skill_refs.is_empty()
            || !input.annotations.is_empty()
        {
            prompt.push_str(
                "\n\nUser-supplied task context (paths are relative to this checkout):\n",
            );
            prompt.push_str(&json!({"fileRefs":input.file_refs,"skillRefs":input.skill_refs,"annotations":input.annotations}).to_string());
        }
        if prompt.len() > wire::MAX_TEXT_BYTES {
            return Err(error("cloud_task_input_too_large"));
        }
        // Files uploaded straight to the computer run as their paths there;
        // the computer checks each is one of its own uploads.
        serde_json::from_value(json!({"kind":"run","messageId":message,"request":{"prompt":prompt,"harness":config["harness"],"model":config["model"],"reasoning":config["reasoning"],"modelOptions":config.get("modelOptions").cloned().unwrap_or(json!({})),"cwd":".","sandbox":sandbox,"autoApprove":auto,"resume":null,"worktree":null,"mcp":null,"attachments":input.computer_files}})).map_err(|_|error("cloud_task_session_config_invalid"))
    }
    pub(super) fn open_queue_input(
        &self,
        workspace: &Value,
        session: &str,
        item: &Value,
    ) -> Result<wire::Input, ApiError> {
        let text = self
            .master
            .open_api(
                &context(
                    required(workspace, "account_id")?,
                    required(workspace, "workspace_id")?,
                    session,
                    "queue",
                    required(item, "queue_id")?,
                ),
                required(item, "ciphertext")?,
            )
            .map_err(|_| ApiError::unavailable("cloud_task_queue_unavailable"))?;
        serde_json::from_slice(&text)
            .map_err(|_| ApiError::unavailable("cloud_task_queue_unavailable"))
    }
    /// One bounded ordinary-worker action. The cached catalog probe keeps old
    /// schemas from parsing optional-table queries without disabling ongoing
    /// delivery when the new-task creation rollout is paused.
    pub async fn work_task_drain_once(&self, now: i64) -> Result<bool, ApiError> {
        if !self.task_command_delivery_available().await? {
            return Ok(false);
        }
        let job:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(d) FROM zuse_rust_task_command_drains d WHERE next_attempt_at<=$1 ORDER BY next_attempt_at,workspace_id,session_id LIMIT 1").bind(now).fetch_optional(&self.pool).await?;
        let Some(job) = job else { return Ok(false) };
        self.run_task_drain_job(job, now).await
    }
    /// [`Self::work_task_drain_once`] for one computer's due job only.
    #[cfg(test)]
    pub(in crate::runtime) async fn work_task_drain_for(
        &self,
        workspace: &str,
        now: i64,
    ) -> Result<bool, ApiError> {
        let job:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(d) FROM zuse_rust_task_command_drains d WHERE workspace_id=$1 AND next_attempt_at<=$2 ORDER BY next_attempt_at,session_id LIMIT 1").bind(workspace).bind(now).fetch_optional(&self.pool).await?;
        let Some(job) = job else { return Ok(false) };
        self.run_task_drain_job(job, now).await
    }
    async fn run_task_drain_job(&self, job: Value, now: i64) -> Result<bool, ApiError> {
        let result = self.promote_task_queue(&job, now).await;
        if let Err(error) = result {
            sqlx::query("UPDATE zuse_rust_task_command_drains SET next_attempt_at=$4,last_error=$5 WHERE workspace_id=$1 AND session_id=$2 AND revision=$3").bind(required(&job,"workspace_id")?).bind(required(&job,"session_id")?).bind(job["revision"].as_i64().unwrap_or(0)).bind(now+30_000).bind(error.code).execute(&self.pool).await?;
            return Ok(true);
        }
        Ok(true)
    }
    /// Immediate best effort dispatch is bounded independently of provider work.
    /// Losing a process or semaphore slot leaves the committed job for workers.
    pub(crate) fn dispatch_task_drain(
        self: &std::sync::Arc<Self>,
        workspace: &str,
        session: Option<&str>,
    ) {
        if self.task_command_schema_ready.get() == Some(&false) {
            return;
        }
        static SLOTS: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
            std::sync::OnceLock::new();
        let Ok(permit) = SLOTS
            .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(32)))
            .clone()
            .try_acquire_owned()
        else {
            return;
        };
        let control = self.clone();
        let workspace = workspace.to_owned();
        let session = session.map(str::to_owned);
        tokio::spawn(async move {
            let _permit = permit;
            let now = chrono::Utc::now().timestamp_millis();
            let result=async{
                if !control.task_command_delivery_available().await?{return Ok(());}
                let job:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(d) FROM zuse_rust_task_command_drains d WHERE workspace_id=$1 AND ($2::text IS NULL OR session_id=$2) AND next_attempt_at<=$3 ORDER BY next_attempt_at,session_id LIMIT 1").bind(workspace).bind(session).bind(now).fetch_optional(&control.pool).await?;
                if let Some(job)=job{control.run_task_drain_job(job,now).await?;}
                Ok::<(),ApiError>(())
            }.await;
            if let Err(error) = result {
                tracing::warn!(
                    code = error.code,
                    "Direct task drain deferred to durable worker"
                );
            }
        });
    }
    async fn promote_task_queue(&self, job: &Value, now: i64) -> Result<(), ApiError> {
        let id = required(job, "workspace_id")?;
        let session = required(job, "session_id")?;
        let preview:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(q) FROM zuse_rust_task_command_queue q WHERE workspace_id=$1 AND session_id=$2 AND state='queued' ORDER BY position,queue_id LIMIT 1").bind(id).bind(session).fetch_optional(&self.pool).await?;
        let Some(preview) = preview else {
            sqlx::query("DELETE FROM zuse_rust_task_command_drains WHERE workspace_id=$1 AND session_id=$2 AND revision=$3").bind(id).bind(session).bind(job["revision"].as_i64().unwrap_or(0)).execute(&self.pool).await?;
            return Ok(());
        };
        let actor = required(&preview, "actor_id")?;
        let fresh = self.scope_access(id, Some(actor), true).await?;
        let mut tx = self.pool.begin().await?;
        let (workspace, _, _) = locked_workspace_with(&mut tx, id, WorkspaceLock::Command).await?;
        let personal = Access {
            actor: crate::auth::Principal {
                account_id: actor.into(),
            },
            owner_id: required(&workspace, "account_id")?.into(),
            scope: crate::access::Scope::Personal,
            membership: None,
        };
        if fresh.is_none()
            && (personal.owner_id != actor || personal.owner_id.starts_with("organization:"))
        {
            return Err(error("cloud_task_queue_author_changed"));
        }
        let access = fresh.as_ref().map_or(&personal, |(a, _)| a);
        self.authorize_task_command_row(&workspace, access, true)?;
        if super::super::recreation::leases_blocked(&workspace) {
            // Kept queued; sent once the recreation is ready.
            return Err(error(zuse_protocol::cloud_runtime_updates::RECREATING));
        }
        if super::super::repair::leases_blocked(&workspace)
            || super::super::lifecycle::valid_memory_recovery(&workspace)
        {
            return Err(error("cloud_task_queue_runtime_recovery_required"));
        }

        if fresh
            .as_ref()
            .is_some_and(|(_, policy)| workspace["request_config"]["sharingPolicy"] != *policy)
        {
            return Err(error("workspace_sharing_changed"));
        }
        let current:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(q) FROM zuse_rust_task_command_queue q WHERE workspace_id=$1 AND session_id=$2 AND state='queued' ORDER BY position,queue_id LIMIT 1 FOR UPDATE").bind(id).bind(session).fetch_optional(&mut *tx).await?;
        let Some(item) = current else {
            tx.rollback().await?;
            return Ok(());
        };
        if item["queue_id"] != preview["queue_id"]
            || item["revision"] != preview["revision"]
            || item["actor_id"] != preview["actor_id"]
            || item["actor"]
                != serde_json::to_value(authorized_actor(access))
                    .map_err(|_| error("cloud_task_queue_author_changed"))?
        {
            return Err(error("cloud_task_queue_author_changed"));
        }
        // A stale edit lease is still a review gate. Paused/sleeping machines
        // retain their queue without creating a lifecycle or allocation action.
        let paused:bool=sqlx::query_scalar("SELECT queue_paused FROM zuse_rust_task_command_sessions WHERE workspace_id=$1 AND session_id=$2 AND state='active'").bind(id).bind(session).fetch_one(&mut *tx).await?;
        let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_task_command_execution e JOIN zuse_rust_mailbox_commands c USING(workspace_id,command_id) WHERE e.workspace_id=$1 AND e.session_id=$2 AND (c.lane LIKE 'task-execution:%' OR (c.lane='task-control:'||e.session_id AND c.envelope->>'kind'='zuse.session.steer' AND e.message_id IS NOT NULL AND e.state IN ('starting','running','waiting-input','outcome-unknown'))) AND (e.state IN ('queued','starting','running','waiting-input') OR (e.state='outcome-unknown' AND NOT e.unknown_reviewed)))").bind(id).bind(session).fetch_one(&mut *tx).await?;
        if paused
            || busy
            || !item["edit_lease_hash"].is_null()
            || workspace["desired_state"] != "ready"
        {
            sqlx::query("UPDATE zuse_rust_task_command_drains SET next_attempt_at=$4 WHERE workspace_id=$1 AND session_id=$2 AND revision=$3").bind(id).bind(session).bind(job["revision"].as_i64().unwrap_or(0)).bind(now+30_000).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(());
        }
        let input = self.open_queue_input(&workspace, session, &item)?;
        self.verify_launch_assets(
            &mut tx,
            required(&workspace, "account_id")?,
            id,
            &input.attachments,
        )
        .await?;
        let message = required(&item, "message_id")?;
        let command = format!(
            "queue:{}",
            self.master.digest_api(
                &context(
                    required(&workspace, "account_id")?,
                    id,
                    session,
                    "queue-revision",
                    required(&item, "queue_id")?
                ),
                &item["revision"].as_i64().unwrap_or(0).to_string()
            )
        );
        let payload = self
            .queued_run_tx(&mut tx, &workspace, session, message, &input)
            .await?;
        self.task_seal_submit_tx(
            &mut tx,
            &workspace,
            access,
            &command,
            session,
            message,
            &payload,
            &input.attachments,
            None,
            now,
        )
        .await?;
        sqlx::query("UPDATE zuse_rust_task_command_queue SET state='claimed',claimed_command_id=$4,updated_at=$5 WHERE workspace_id=$1 AND session_id=$2 AND queue_id=$3 AND state='queued'").bind(id).bind(session).bind(required(&item,"queue_id")?).bind(&command).bind(now).execute(&mut *tx).await?;
        // Sent: whatever it was waiting for is over.
        sqlx::query("UPDATE zuse_rust_task_command_drains SET next_attempt_at=$4,last_error=NULL WHERE workspace_id=$1 AND session_id=$2 AND revision=$3").bind(id).bind(session).bind(job["revision"].as_i64().unwrap_or(0)).bind(now+30_000).execute(&mut *tx).await?;
        tx.commit().await?;
        self.notify_task_drain();
        Ok(())
    }
}
