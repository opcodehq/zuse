use super::*;
use zuse_cloud_runtime::mailbox::CommandKey;
use zuse_protocol::{
    cloud_commands::{CloudSessionCommand, CommandEnvelope},
    cloud_runtime_updates as updates,
    session_commands::SessionCommandPayload,
};
/// How a client's message is admitted.
#[derive(Clone, Copy, Debug, Default)]
pub struct Submit {
    /// The client shows server-queued messages: a run that has to wait for
    /// its computer (runtime update, Update computer) is queued, not refused.
    /// See [`updates::QUEUE_WHILE_WAITING`].
    pub queue_while_waiting: bool,
}
impl RuntimeControl {
    pub(super) async fn initialize_task_command_policy_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        intent: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        if !command_mode(row) {
            return Err(error("server_command_admission_update_required"));
        }
        let id = required(row, "workspace_id")?;
        let owner = required(row, "account_id")?;
        let session = required(row, "initial_session_id")?;
        // Creation holds the managed-base admission fence and has already
        // validated this exact release/candidate. Snapshot its real installed
        // catalogue; future turns never select an arbitrary uninstalled model.
        let release_id = required(&row["request_config"], "managedReleaseId")?;
        let record:Value=sqlx::query_scalar("SELECT CASE WHEN $3 THEN candidate_manifest ELSE runtime_manifest END FROM cloud_managed_bases WHERE provider=$1 AND release_id=$2").bind(required(row,"provider")?).bind(release_id).bind(row["request_config"]["managedCandidateSha256"].is_string()).fetch_optional(&mut **tx).await?.ok_or_else(||error("cloud_task_managed_policy_required"))?;
        let agent = required(&row["request_config"], "agent")?;
        let native = record["harnesses"]
            .as_array()
            .and_then(|v| v.iter().find(|v| v["agent"] == agent))
            .ok_or_else(|| error("cloud_task_model_catalog_missing"))?;
        let catalog: zuse_protocol::cloud_catalog::CloudAgentModels =
            serde_json::from_value(native["catalog"].clone())
                .map_err(|_| error("cloud_task_model_catalog_missing"))?;
        let policy = Policy {
            agent: required(&row["request_config"], "agent")?.into(),
            connection_id: required(&row["request_config"], "agentConnectionId")?.into(),
            connection_owner_id: required(&row["request_config"], "agentConnectionOwnerId")?.into(),
            runtime_mode: serde_json::from_value(intent["runtimeMode"].clone())
                .map_err(|_| error("cloud_task_policy_invalid"))?,
            initial_model: required(intent, "model")?.into(),
            reasoning: serde_json::from_value(intent["reasoning"].clone())
                .map_err(|_| error("cloud_task_policy_invalid"))?,
            model_options: intent["modelOptions"]
                .as_object()
                .cloned()
                .unwrap_or_default(),
            catalog,
            catalogs: agents::installed_catalogs(&record)?,
        };
        let ciphertext = self
            .master
            .seal_api(
                &context(owner, id, "", "policy", "1"),
                &serde_json::to_string(&policy).map_err(|_| error("cloud_task_policy_invalid"))?,
            )
            .map_err(|_| ApiError::unavailable("cloud_task_command_encryption_failed"))?;
        sqlx::query("INSERT INTO zuse_rust_task_command_policies(workspace_id,owner_id,policy_revision,ciphertext,created_at,updated_at) VALUES($1,$2,1,$3,$4,$4)").bind(id).bind(owner).bind(ciphertext).bind(now).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO zuse_rust_task_command_sessions(workspace_id,session_id,thread_id,state,created_at) VALUES($1,$2,$3,'active',$4)").bind(id).bind(session).bind(required(row,"chat_id")?).bind(now).execute(&mut **tx).await?;
        Ok(())
    }
    pub(in crate::runtime) async fn task_policy_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
    ) -> Result<(Policy, i64), ApiError> {
        let id = required(row, "workspace_id")?;
        let owner = required(row, "account_id")?;
        let (revision,cipher):(i64,String)=sqlx::query_as("SELECT policy_revision,ciphertext FROM zuse_rust_task_command_policies WHERE workspace_id=$1 AND owner_id=$2").bind(id).bind(owner).fetch_optional(&mut **tx).await?.ok_or_else(||error("cloud_task_policy_missing"))?;
        let text = self
            .master
            .open_api(
                &context(owner, id, "", "policy", &revision.to_string()),
                &cipher,
            )
            .map_err(|_| ApiError::unavailable("cloud_task_policy_unavailable"))?;
        let policy: Policy = serde_json::from_slice(&text)
            .map_err(|_| ApiError::unavailable("cloud_task_policy_unavailable"))?;
        if row["request_config"]["agentConnectionId"] != policy.connection_id
            || row["request_config"]["agentConnectionOwnerId"] != policy.connection_owner_id
            || row["request_config"]["agent"] != policy.agent
        {
            return Err(error("cloud_task_policy_changed"));
        }
        Ok((policy, revision))
    }
    /// Re-seal the task policy with a new access mode, keeping its revision
    /// (the runtime binds commands to the revision, not the mode).
    async fn set_task_runtime_mode_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        policy: &Policy,
        revision: i64,
        mode: zuse_protocol::cloud::CloudRuntimeMode,
        now: i64,
    ) -> Result<(), ApiError> {
        let id = required(row, "workspace_id")?;
        let owner = required(row, "account_id")?;
        let mut updated = policy.clone();
        updated.runtime_mode = mode;
        let ciphertext = self
            .master
            .seal_api(
                &context(owner, id, "", "policy", &revision.to_string()),
                &serde_json::to_string(&updated).map_err(|_| error("cloud_task_policy_invalid"))?,
            )
            .map_err(|_| ApiError::unavailable("cloud_task_command_encryption_failed"))?;
        let changed = sqlx::query("UPDATE zuse_rust_task_command_policies SET ciphertext=$3,updated_at=$4 WHERE workspace_id=$1 AND owner_id=$2 AND policy_revision=$5")
            .bind(id).bind(owner).bind(ciphertext).bind(now).bind(revision)
            .execute(&mut **tx).await?;
        if changed.rows_affected() != 1 {
            return Err(error("cloud_task_policy_changed"));
        }
        Ok(())
    }
    pub async fn task_command_submit(
        &self,
        id: &str,
        access: &Access,
        request: wire::Request,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.task_command_submit_with(id, access, request, Submit::default(), now)
            .await
    }
    pub async fn task_command_submit_with(
        &self,
        id: &str,
        access: &Access,
        request: wire::Request,
        options: Submit,
        now: i64,
    ) -> Result<Value, ApiError> {
        if !self.task_command_delivery_available().await? {
            return Err(ApiError::unavailable("cloud_task_command_schema_not_ready"));
        }
        validation::request(&request)?;
        // External membership lookup is complete before the workspace lock.
        let fresh = self
            .scope_access(id, Some(&access.actor.account_id), true)
            .await?;
        let mut tx = self.pool.begin().await?;
        let (row, _, _) = locked_workspace_with(&mut tx, id, WorkspaceLock::Command).await?;
        if let Some((_, sharing_policy)) = &fresh
            && row["request_config"]["sharingPolicy"] != *sharing_policy
        {
            return Err(ApiError::conflict("workspace_sharing_changed"));
        }
        let authorized = fresh.as_ref().map_or(access, |(access, _)| access);
        self.authorize_task_command_row(&row, authorized, true)?;
        let result = self
            .task_command_submit_tx(&mut tx, &row, authorized, &request, options, now)
            .await?;
        tx.commit().await?;
        self.notify_task_drain();
        if request.wake == wire::WakePolicy::ExplicitResume {
            super::super::lifecycle::wakeup::send();
        }
        Ok(result)
    }
    pub(super) fn authorize_task_command_row(
        &self,
        row: &Value,
        access: &Access,
        edit: bool,
    ) -> Result<(), ApiError> {
        if row["account_id"] != access.owner_id {
            return Err(ApiError::not_found("cloud_workspace_not_found"));
        }
        let permission = access.workspace_permission(&access.owner_id, &row["request_config"])?;
        if edit && permission != crate::access::Permission::Edit {
            return Err(ApiError::forbidden("workspace_access_denied"));
        }
        if !command_mode(row) {
            return Err(error("server_command_admission_update_required"));
        }
        if blocked(row)
            || matches!(row["desired_state"].as_str(), Some("deleted" | "archived"))
            || row["request_config"]["cloudMailboxLifecyclePending"]["action"] == "archive"
        {
            return Err(error("cloud_workspace_unavailable"));
        }
        Ok(())
    }
    pub(in crate::runtime) async fn task_command_submit_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        access: &Access,
        request: &wire::Request,
        options: Submit,
        now: i64,
    ) -> Result<Value, ApiError> {
        validation::request(request)?;
        self.authorize_task_command_row(row, access, true)?;
        let id = required(row, "workspace_id")?;
        let owner = &access.owner_id;
        let session_ok:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_task_command_sessions WHERE workspace_id=$1 AND session_id=$2 AND state='active')").bind(id).bind(&request.session_id).fetch_one(&mut **tx).await?;
        if !session_ok && !matches!(request.action, wire::Action::CreateSession { .. }) {
            return Err(error("cloud_task_session_not_registered"));
        }
        let serialized = zeroize::Zeroizing::new(
            serde_json::to_string(request)
                .map_err(|_| ApiError::bad_request("cloud_task_command_invalid"))?,
        );
        let digest = self.master.digest_api(
            &context(
                owner,
                id,
                &request.session_id,
                "action",
                &request.command_id,
            ),
            &serialized,
        );
        if let Some(existing)=sqlx::query("SELECT actor_id,request_digest,acceptance FROM zuse_rust_task_command_actions WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(&request.command_id).fetch_optional(&mut **tx).await? {
            if existing.get::<String,_>("actor_id")!=access.actor.account_id||existing.get::<String,_>("request_digest")!=digest {return Err(error("cloud_task_command_idempotency_conflict"));}
            return Ok(existing.get("acceptance"));
        }
        // New user actions reset inactivity in the admission transaction. The
        // idempotent early return above never extends it from a replay.
        self.record_activity_tx(tx, row, now, true).await?;
        if let wire::Action::CreateSession { title } = &request.action {
            if session_ok {
                return Err(error("cloud_task_session_identity_conflict"));
            }
            let metadata = self
                .master
                .seal_api(
                    &context(owner, id, &request.session_id, "session", "metadata"),
                    &json!({"title":title}).to_string(),
                )
                .map_err(|_| ApiError::unavailable("cloud_task_command_encryption_failed"))?;
            sqlx::query("INSERT INTO zuse_rust_task_command_sessions(workspace_id,session_id,thread_id,state,metadata_ciphertext,created_at) VALUES($1,$2,$3,'active',$4,$5)").bind(id).bind(&request.session_id).bind(required(row,"chat_id")?).bind(metadata).bind(now).execute(&mut **tx).await?;
            sqlx::query("INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,workspace_id,payload,created_at) VALUES($1,'task-command-progress',$2,$3,'{\"commandAdmissionVersion\":2,\"sessionsChanged\":true}'::jsonb,$4)").bind(owner).bind(&request.session_id).bind(id).bind(now).execute(&mut **tx).await?;
            let acceptance = json!({"commandId":request.command_id,"durablyAdmitted":true,"sessionId":request.session_id,"threadId":row["chat_id"],"createdAt":now});
            self.enqueue_task_drain_tx(tx, row, &request.session_id, now)
                .await?;
            self.save_task_action_tx(tx, id, access, request, &digest, &acceptance, now)
                .await?;
            return Ok(acceptance);
        }
        // A run that only has to wait for its computer is queued instead of
        // refused when the client shows server-queued messages.
        let waiting_run = options.queue_while_waiting
            && matches!(
                &request.action,
                wire::Action::Submit {
                    payload: SessionCommandPayload::Run { .. },
                    ..
                }
            );
        let recreating = super::super::recreation::leases_blocked(row);
        if recreating && !waiting_run {
            return Err(error(updates::RECREATING));
        }
        if !recreating && super::super::lifecycle::valid_memory_recovery(row) {
            return Err(error("codex_memory_transport_recovery_required"));
        }
        if !recreating && super::super::repair::leases_blocked(row) {
            return Err(error("runtime_repair_attention_required"));
        }
        // Every referenced asset is committed under this exact owner/workspace;
        // verification reads encrypted manifest rows, never R2/network under lock.
        match &request.action {
            wire::Action::Submit { attachments, .. } => {
                self.verify_launch_assets(tx, owner, id, attachments)
                    .await?
            }
            wire::Action::Queue { input, .. } | wire::Action::Edit { input, .. } => {
                self.verify_launch_assets(tx, owner, id, &input.attachments)
                    .await?
            }
            _ => {}
        }
        let mut acceptance = if recreating {
            self.queue_waiting_run_tx(tx, row, access, request, updates::RECREATING, now)
                .await?
        } else if let Some(value) = self
            .task_queue_action_tx(tx, row, access, request, now)
            .await?
        {
            value
        } else {
            match &request.action {
                wire::Action::Submit {
                    message_id,
                    payload,
                    attachments,
                    based_on,
                } if waiting_run => {
                    // A savepoint, so a refusal for the computer's runtime
                    // leaves nothing behind but the queued message.
                    let mut attempt = sqlx::Acquire::begin(&mut **tx).await?;
                    match self
                        .task_seal_submit_tx(
                            &mut attempt,
                            row,
                            access,
                            &request.command_id,
                            &request.session_id,
                            message_id,
                            payload,
                            attachments,
                            based_on.clone(),
                            now,
                        )
                        .await
                    {
                        Ok(value) => {
                            attempt.commit().await?;
                            value
                        }
                        Err(refused)
                            if refused.code == updates::RUNTIME_UPDATING
                                || refused.code == updates::RUNTIME_UPDATE_UNAVAILABLE =>
                        {
                            attempt.rollback().await?;
                            self.queue_waiting_run_tx(tx, row, access, request, refused.code, now)
                                .await?
                        }
                        Err(refused) => return Err(refused),
                    }
                }
                wire::Action::Submit {
                    message_id,
                    payload,
                    attachments,
                    based_on,
                } => {
                    self.task_seal_submit_tx(
                        tx,
                        row,
                        access,
                        &request.command_id,
                        &request.session_id,
                        message_id,
                        payload,
                        attachments,
                        based_on.clone(),
                        now,
                    )
                    .await?
                }
                wire::Action::ReviewUnknown {
                    interrupted_command_id,
                } => {
                    let affected=sqlx::query("UPDATE zuse_rust_task_command_execution SET unknown_reviewed=true,updated_at=$4 WHERE workspace_id=$1 AND session_id=$2 AND command_id=$3 AND state='outcome-unknown'").bind(id).bind(&request.session_id).bind(interrupted_command_id).bind(now).execute(&mut **tx).await?;
                    if affected.rows_affected() != 1 {
                        return Err(error("cloud_task_unknown_outcome_not_found"));
                    }
                    json!({"reviewed":true,"interruptedCommandId":interrupted_command_id})
                }
                _ => return Err(error("cloud_task_queue_delivery_update_required")),
            }
        };
        // Explicit wake is handled only after message admission, in this same
        // transaction. Until its lifecycle integration is available it fails
        // closed and rolls back, never quietly waking or dropping the message.
        // A recreation brings the computer back up itself.
        if request.wake == wire::WakePolicy::ExplicitResume && !recreating {
            acceptance["wake"] = self
                .task_command_wake_tx(tx, row, &request.command_id, now)
                .await?;
        }
        acceptance["commandId"] = json!(request.command_id);
        acceptance["durablyAdmitted"] = json!(true);
        self.enqueue_task_drain_tx(tx, row, &request.session_id, now)
            .await?;
        if let Some(reason) = acceptance["waitingFor"].as_str() {
            // Every device sees why it waits before the first drain attempt.
            sqlx::query("UPDATE zuse_rust_task_command_drains SET last_error=$3 WHERE workspace_id=$1 AND session_id=$2")
                .bind(id).bind(&request.session_id).bind(reason).execute(&mut **tx).await?;
        }
        self.save_task_action_tx(tx, id, access, request, &digest, &acceptance, now)
            .await?;
        Ok(acceptance)
    }
    /// Save a run that has to wait for its computer as a queued message with
    /// the same text, files and agent settings. Its files stay where they
    /// are (on the computer, or held in object storage until it is sent);
    /// the drain sends it once the computer can take it.
    async fn queue_waiting_run_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        access: &Access,
        request: &wire::Request,
        reason: &'static str,
        now: i64,
    ) -> Result<Value, ApiError> {
        let wire::Action::Submit {
            message_id,
            payload: SessionCommandPayload::Run { request: run, .. },
            attachments,
            ..
        } = &request.action
        else {
            return Err(error(reason));
        };
        let input = wire::Input {
            run_selection: Some(wire::RunSelection {
                harness: run
                    .harness
                    .ok_or_else(|| error("cloud_task_agent_required"))?,
                model: run
                    .model
                    .clone()
                    .ok_or_else(|| error("cloud_task_model_required"))?,
                reasoning: run.reasoning,
                model_options: run.model_options.clone(),
                sandbox: run.sandbox,
            }),
            text: run.prompt.clone(),
            attachments: attachments.clone(),
            computer_files: run.attachments.clone(),
            file_refs: vec![],
            skill_refs: vec![],
            annotations: vec![],
            as_goal: false,
        };
        validation::input(&input)?;
        let queued = wire::Request {
            command_id: request.command_id.clone(),
            session_id: request.session_id.clone(),
            // The queue id is the message id, so the sent message's bubble
            // becomes its queued row.
            action: wire::Action::Queue {
                queue_id: message_id.clone(),
                message_id: message_id.clone(),
                input,
                hold_for_turn_end: true,
            },
            wake: request.wake,
        };
        let mut acceptance = self
            .task_queue_action_tx(tx, row, access, &queued, now)
            .await?
            .ok_or_else(|| error(reason))?;
        acceptance["waitingFor"] = json!(reason);
        Ok(acceptance)
    }
    async fn save_task_action_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: &str,
        access: &Access,
        request: &wire::Request,
        digest: &str,
        acceptance: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        sqlx::query("INSERT INTO zuse_rust_task_command_actions(workspace_id,command_id,session_id,actor_id,request_digest,action_kind,acceptance,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(id).bind(&request.command_id).bind(&request.session_id).bind(&access.actor.account_id).bind(digest).bind(serde_json::to_value(&request.action).ok().and_then(|v|v["type"].as_str().map(str::to_owned)).unwrap_or_default()).bind(&acceptance).bind(now).execute(&mut **tx).await?;
        Ok(())
    }
    pub(super) async fn task_seal_submit_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        access: &Access,
        command: &str,
        session: &str,
        message: &str,
        payload: &SessionCommandPayload,
        assets: &[zuse_protocol::cloud_assets::Asset],
        based_on: Option<zuse_protocol::session_commands::CommandBasedOn>,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.task_seal_submit_inner_tx(
            tx, row, access, command, session, message, payload, assets, based_on, now, true,
        )
        .await
    }
    pub(super) async fn task_seal_interrupt_dependency_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        access: &Access,
        command: &str,
        session: &str,
        message: &str,
        now: i64,
    ) -> Result<Value, ApiError> {
        self.task_seal_submit_inner_tx(
            tx,
            row,
            access,
            command,
            session,
            message,
            &SessionCommandPayload::Interrupt {},
            &[],
            None,
            now,
            false,
        )
        .await
    }
    pub(super) async fn task_seal_submit_inner_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        access: &Access,
        command: &str,
        session: &str,
        message: &str,
        payload: &SessionCommandPayload,
        assets: &[zuse_protocol::cloud_assets::Asset],
        based_on: Option<zuse_protocol::session_commands::CommandBasedOn>,
        now: i64,
        pause_queue_on_interrupt: bool,
    ) -> Result<Value, ApiError> {
        // The stored presentation identity and actual guest user entry must
        // agree, including when a caller omitted an optional steer MID.
        let canonical_payload = match payload {
            SessionCommandPayload::Run { message_id, .. } if message_id != message => {
                return Err(error("cloud_task_message_identity_conflict"));
            }
            SessionCommandPayload::Steer { prompt, message_id } => {
                if message_id.as_deref().is_some_and(|id| id != message) {
                    return Err(error("cloud_task_message_identity_conflict"));
                }
                SessionCommandPayload::Steer {
                    prompt: prompt.clone(),
                    message_id: Some(message.into()),
                }
            }
            _ => payload.clone(),
        };
        let payload = &canonical_payload;
        let id = required(row, "workspace_id")?;
        let owner = &access.owner_id;
        let (policy, revision) = self.task_policy_tx(tx, row).await?;
        let run = matches!(payload, SessionCommandPayload::Run { .. });
        if let SessionCommandPayload::Run { request, .. } = payload {
            let harness = request
                .harness
                .as_ref()
                .ok_or_else(|| error("cloud_task_agent_required"))?;
            let catalog = policy.catalog_for(harness)?;
            let model = catalog
                .resolve_model(
                    request
                        .model
                        .as_deref()
                        .ok_or_else(|| error("cloud_task_model_required"))?,
                )
                .ok_or_else(|| error("cloud_task_model_not_installed"))?;
            if request
                .reasoning
                .is_some_and(|r| !model.reasoning_levels.contains(&r))
            {
                return Err(error("cloud_task_model_reasoning_invalid"));
            }
            for (name, value) in &request.model_options {
                let choice = value
                    .as_str()
                    .ok_or_else(|| error("cloud_task_model_option_invalid"))?;
                if !model.options.iter().any(|option| {
                    option.id == *name && option.choices.iter().any(|v| v.id == choice)
                }) {
                    return Err(error("cloud_task_model_option_invalid"));
                }
            }
            // An older guest runtime fails on a run carrying computer file
            // paths, leaving the command's outcome unknown and the chat
            // locked; refuse it here, before anything is enqueued, and
            // update the runtime in place when a release can. Checked after
            // the model, so a message that can never run is refused outright
            // rather than left waiting for an update.
            if !request.attachments.is_empty()
                && !crate::runtime::runtime_capable(row, wire::DIRECT_UPLOADS_CAPABILITY)
            {
                return Err(self
                    .demand_runtime_update(tx, row, wire::DIRECT_UPLOADS_CAPABILITY, now)
                    .await);
            }
            let (sandbox, auto) = match policy.runtime_mode {
                zuse_protocol::cloud::CloudRuntimeMode::FullAccess => {
                    (zuse_protocol::SandboxLevel::DangerFullAccess, true)
                }
                zuse_protocol::cloud::CloudRuntimeMode::Auto => {
                    (zuse_protocol::SandboxLevel::Auto, false)
                }
                _ => (zuse_protocol::SandboxLevel::WorkspaceWrite, false),
            };
            if request.sandbox != sandbox || request.auto_approve != auto {
                // Access is chosen per message: a different, valid mode
                // becomes the task's policy from this message on.
                let mode = requested_runtime_mode(request.sandbox, request.auto_approve)
                    .ok_or_else(|| error("cloud_task_permission_policy_invalid"))?;
                // An unchanged mode is admitted as before, so plain messages
                // keep working on computers that predate per-message access.
                if !crate::runtime::runtime_capable(row, wire::MESSAGE_ACCESS_MODE_CAPABILITY) {
                    return Err(self
                        .demand_runtime_update(tx, row, wire::MESSAGE_ACCESS_MODE_CAPABILITY, now)
                        .await);
                }
                self.set_task_runtime_mode_tx(tx, row, &policy, revision, mode, now)
                    .await?;
            }
            let requested =
                agents::agent(harness).ok_or_else(|| error("cloud_task_agent_not_installed"))?;
            let initial = if policy.agent == "claude-code" {
                "claude"
            } else {
                &policy.agent
            };
            if requested != initial {
                self.bind_task_agent_tx(tx, row, requested).await?;
            }
        } else if !assets.is_empty() {
            return Err(ApiError::bad_request(
                "cloud_task_control_attachments_invalid",
            ));
        }
        if run {
            let requires_review:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND session_id=$2 AND state='outcome-unknown' AND NOT unknown_reviewed)").bind(id).bind(session).fetch_one(&mut **tx).await?;
            if requires_review {
                return Err(error("cloud_task_unknown_outcome_review_required"));
            }
        }
        if let SessionCommandPayload::Run { request, .. } = payload {
            let config = json!({"harness":request.harness,"model":request.model,"reasoning":request.reasoning,"modelOptions":request.model_options,"sandbox":request.sandbox});
            let previous: Option<String> = sqlx::query_scalar("SELECT metadata_ciphertext FROM zuse_rust_task_command_sessions WHERE workspace_id=$1 AND session_id=$2")
                .bind(id).bind(session).fetch_one(&mut **tx).await?;
            let metadata_context = context(owner, id, session, "session", "metadata");
            let mut updated = if let Some(previous) = previous {
                let text = self
                    .master
                    .open_api(&metadata_context, &previous)
                    .map_err(|_| ApiError::unavailable("cloud_task_session_unavailable"))?;
                serde_json::from_slice::<Value>(&text)
                    .map_err(|_| ApiError::unavailable("cloud_task_session_unavailable"))?
            } else {
                json!({})
            };
            if !updated.is_object() {
                return Err(ApiError::unavailable("cloud_task_session_unavailable"));
            }
            updated["config"] = config;
            let metadata = self
                .master
                .seal_api(&metadata_context, &updated.to_string())
                .map_err(|_| ApiError::unavailable("cloud_task_command_encryption_failed"))?;
            sqlx::query("UPDATE zuse_rust_task_command_sessions SET metadata_ciphertext=$3 WHERE workspace_id=$1 AND session_id=$2").bind(id).bind(session).bind(metadata).execute(&mut **tx).await?;
        }
        let interrupt = if matches!(payload, SessionCommandPayload::Interrupt {}) {
            self.bind_task_interrupt_tx(tx, row, session, based_on.as_ref(), now)
                .await?
        } else {
            None
        };
        if matches!(payload, SessionCommandPayload::Interrupt {}) && pause_queue_on_interrupt {
            let changed=sqlx::query("UPDATE zuse_rust_task_command_sessions SET queue_paused=true WHERE workspace_id=$1 AND session_id=$2 AND NOT queue_paused").bind(id).bind(session).execute(&mut **tx).await?;
            if changed.rows_affected() > 0 {
                sqlx::query("INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,workspace_id,payload,created_at) VALUES($1,'task-command-queue',$2,$3,'{\"commandAdmissionVersion\":2,\"paused\":true}'::jsonb,$4)").bind(owner).bind(session).bind(id).bind(now).execute(&mut **tx).await?;
            }
        }
        let native = CloudSessionCommand {
            command_id: command.into(),
            session_id: session.into(),
            payload: payload.clone(),
            based_on,
            expires_at: None,
            runtime_binding: Some(wire::RuntimeBinding {
                version: wire::VERSION,
                policy_revision: revision,
                interrupt_command_id: interrupt.as_ref().map(|target| target.command_id.clone()),
                interrupt_message_id: interrupt.as_ref().map(|target| target.message_id.clone()),
                cwd_relative: match payload {
                    SessionCommandPayload::Run { request, .. } => request.cwd.clone(),
                    _ => ".".into(),
                },
                attachments: assets.to_vec(),
                session: if !run || session == row["initial_session_id"] {
                    None
                } else {
                    let registration=sqlx::query("SELECT thread_id,metadata_ciphertext,created_at FROM zuse_rust_task_command_sessions WHERE workspace_id=$1 AND session_id=$2 AND state='active'")
                        .bind(id).bind(session).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::not_found("cloud_task_session_not_registered"))?;
                    let thread: String = registration.get("thread_id");
                    if row["chat_id"] != thread {
                        return Err(error("cloud_task_session_scope_changed"));
                    }
                    let title = if let Some(cipher) =
                        registration.get::<Option<String>, _>("metadata_ciphertext")
                    {
                        let text = self
                            .master
                            .open_api(&context(owner, id, session, "session", "metadata"), &cipher)
                            .map_err(|_| ApiError::unavailable("cloud_task_session_unavailable"))?;
                        let value: Value = serde_json::from_slice(&text)
                            .map_err(|_| ApiError::unavailable("cloud_task_session_unavailable"))?;
                        value["title"].as_str().map(str::to_owned)
                    } else {
                        None
                    };
                    Some(wire::RuntimeSession {
                        session_id: session.into(),
                        thread_id: thread,
                        title,
                        created_at: registration.get("created_at"),
                    })
                },
            }),
        };
        let key = self
            .master
            .open_transcript_key(owner, id, required(row, "wrapped_transcript_key")?)
            .map_err(|_| ApiError::unavailable("cloud_command_key_unavailable"))?;
        let key = CommandKey::parse(&key)
            .map_err(|_| ApiError::unavailable("cloud_command_key_unavailable"))?;
        let bytes = zeroize::Zeroizing::new(
            serde_json::to_vec(&native)
                .map_err(|_| ApiError::bad_request("cloud_task_command_invalid"))?,
        );
        if bytes.len() > 131_072 {
            return Err(ApiError::bad_request("cloud_task_command_too_large"));
        }
        let mut envelope = CommandEnvelope {
            protocol_version: zuse_protocol::cloud_commands::PROTOCOL_VERSION,
            workspace_id: id.into(),
            session_id: session.into(),
            command_id: command.into(),
            kind: zuse_protocol::cloud_commands::session_command_kind(payload.kind()).into(),
            fingerprint: key.fingerprint(&bytes),
            schema_version: 1,
            key_version: 1,
            destruction_fence: super::super::mailbox::destruction_fence(row),
            created_at: now,
            iv: String::new(),
            ciphertext: String::new(),
            dependencies: vec![],
            actor: authorized_actor(access),
        };
        let (iv, ciphertext) = key
            .encrypt(&envelope, &bytes)
            .map_err(|_| ApiError::unavailable("cloud_command_encryption_failed"))?;
        envelope.iv = iv;
        envelope.ciphertext = ciphertext;
        let mut acceptance = self
            .insert_task_envelope_tx(tx, row, &envelope, run, now)
            .await?;
        if matches!(payload, SessionCommandPayload::Interrupt {}) {
            acceptance["interruptCommandId"] =
                json!(interrupt.as_ref().map(|v| v.command_id.as_str()));
            acceptance["interruptMessageId"] =
                json!(interrupt.as_ref().map(|v| v.message_id.as_str()));
            acceptance["cancelledBeforeDispatch"] = json!(
                interrupt
                    .as_ref()
                    .is_some_and(|v| v.cancelled_before_dispatch)
            );
            acceptance["noCurrentRun"] = json!(interrupt.is_none());
            if pause_queue_on_interrupt {
                acceptance["queuePaused"] = json!(true);
            }
        }
        let presentation = match payload {
            SessionCommandPayload::Run { request, .. } => {
                json!({"messageId":message,"text":request.prompt,"attachments":assets})
            }
            SessionCommandPayload::Steer { prompt, .. } => {
                let selected = self.active_task_run_selection_tx(tx, row, session).await?;
                json!({"messageId":message,"text":prompt,"runSelection":selected})
            }
            _ => json!({}),
        };
        let cipher = self
            .master
            .seal_api(
                &context(owner, id, session, "presentation", command),
                &presentation.to_string(),
            )
            .map_err(|_| ApiError::unavailable("cloud_command_encryption_failed"))?;
        sqlx::query("INSERT INTO zuse_rust_task_command_execution(workspace_id,command_id,session_id,message_id,fingerprint,state,presentation_ciphertext,created_at,updated_at) VALUES($1,$2,$3,$4,$5,'queued',$6,$7,$7)").bind(id).bind(command).bind(session).bind(if run||matches!(payload,SessionCommandPayload::Steer{..}){Some(message)}else{None}).bind(&envelope.fingerprint).bind(cipher).bind(now).execute(&mut **tx).await?;
        // Server-held files stay in object storage until this command (or
        // another that binds them) reports the computer has them.
        self.hold_assets_tx(
            tx,
            id,
            super::super::assets::Holder::Command { session, command },
            assets,
            now,
        )
        .await?;
        Ok(acceptance)
    }
    async fn insert_task_envelope_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        envelope: &CommandEnvelope,
        run: bool,
        now: i64,
    ) -> Result<Value, ApiError> {
        let id = required(row, "workspace_id")?;
        if !envelope.eligible() {
            return Err(ApiError::bad_request("cloud_command_not_eligible"));
        }
        sqlx::query(
            "INSERT INTO zuse_rust_mailboxes(workspace_id) VALUES($1) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .execute(&mut **tx)
        .await?;
        let (count,bytes):(i64,i64)=sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(envelope_bytes),0)::bigint FROM zuse_rust_mailbox_commands WHERE workspace_id=$1 AND state NOT IN ('applied','rejected','expired','cancelled','outcome-unknown')").bind(id).fetch_one(&mut **tx).await?;
        let value = serde_json::to_value(envelope)
            .map_err(|_| ApiError::bad_request("cloud_command_not_eligible"))?;
        let size = serde_json::to_vec(&value)
            .map_err(|_| ApiError::bad_request("cloud_command_not_eligible"))?
            .len() as i64;
        if count >= zuse_protocol::cloud_commands::MAX_NONTERMINAL
            || bytes + size > zuse_protocol::cloud_commands::MAX_MAILBOX_BYTES
        {
            return Err(error("cloud_mailbox_full"));
        }
        let (sequence,revision):(i64,i64)=sqlx::query_as("UPDATE zuse_rust_mailboxes SET sequence=sequence+1,revision=revision+1 WHERE workspace_id=$1 RETURNING sequence,revision").bind(id).fetch_one(&mut **tx).await?;
        let waiting = row["state"] != "ready"
            || row["runtime_state"] != "online"
            || row["desired_state"] != "ready";
        let state = if waiting {
            "waiting-for-runtime"
        } else {
            "accepted"
        };
        sqlx::query("INSERT INTO zuse_rust_mailbox_commands(workspace_id,command_id,workspace_sequence,revision,lane,fingerprint,envelope,envelope_bytes,state,accepted_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10)").bind(id).bind(&envelope.command_id).bind(sequence).bind(revision).bind(format!("task-{}:{}",if run{"execution"}else{"control"},envelope.session_id)).bind(&envelope.fingerprint).bind(value).bind(size).bind(state).bind(now).execute(&mut **tx).await?;
        Ok(
            json!({"commandId":envelope.command_id,"workspaceSequence":sequence,"revision":revision,"acceptedAt":now,"state":state,"deliveryState":state,"executionState":"queued"}),
        )
    }
}

/// The task access mode a run request asks for; only the pairings the app
/// sends are valid (full access always auto-approves, the others never do).
fn requested_runtime_mode(
    sandbox: zuse_protocol::SandboxLevel,
    auto_approve: bool,
) -> Option<zuse_protocol::cloud::CloudRuntimeMode> {
    use zuse_protocol::{SandboxLevel as S, cloud::CloudRuntimeMode as M};
    match (sandbox, auto_approve) {
        (S::DangerFullAccess, true) => Some(M::FullAccess),
        (S::Auto, false) => Some(M::Auto),
        (S::WorkspaceWrite, false) => Some(M::ApprovalRequired),
        _ => None,
    }
}

#[cfg(test)]
mod access_mode_tests {
    use super::requested_runtime_mode;
    use zuse_protocol::{SandboxLevel as S, cloud::CloudRuntimeMode as M};

    #[test]
    fn a_message_may_choose_any_access_mode_the_app_offers() {
        assert!(matches!(
            requested_runtime_mode(S::DangerFullAccess, true),
            Some(M::FullAccess)
        ));
        assert!(matches!(
            requested_runtime_mode(S::Auto, false),
            Some(M::Auto)
        ));
        assert!(matches!(
            requested_runtime_mode(S::WorkspaceWrite, false),
            Some(M::ApprovalRequired)
        ));
        // Auto-approval only ever comes with full access.
        assert!(requested_runtime_mode(S::DangerFullAccess, false).is_none());
        assert!(requested_runtime_mode(S::WorkspaceWrite, true).is_none());
    }
}
