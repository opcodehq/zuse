//! Workspace-row-serialized mutable queue. No queued payload is sent to a guest
//! before its final revision is claimed; edits can never race old-text replay.
use super::*;
// Live message-bearing Steer inference uses the control transport lane;
// its lifetime, rather than its delivery ACK, governs conversation controls.
fn conflict() -> ApiError {
    error("cloud_task_queue_changed")
}
fn lease_hash(
    c: &RuntimeControl,
    owner: &str,
    workspace: &str,
    session: &str,
    queue: &str,
    token: &str,
) -> String {
    c.master.digest_api(
        &context(owner, workspace, session, "edit-lease", queue),
        token,
    )
}
fn editable(
    c: &RuntimeControl,
    row: &Value,
    access: &Access,
    workspace: &str,
    session: &str,
    queue: &str,
    token: Option<&str>,
    now: i64,
) -> Result<(), ApiError> {
    if row["state"] != "queued" {
        return Err(error("cloud_task_queue_already_claimed"));
    }
    if row["edit_expires_at"]
        .as_i64()
        .is_some_and(|expiry| expiry > now)
    {
        if row["edit_actor_id"] != access.actor.account_id
            || token.is_none_or(|token| {
                row["edit_lease_hash"]
                    != lease_hash(c, &access.owner_id, workspace, session, queue, token)
            })
        {
            return Err(error("cloud_task_queue_edit_locked"));
        }
    }
    Ok(())
}
async fn row(
    tx: &mut Transaction<'_, Postgres>,
    workspace: &str,
    session: &str,
    queue: &str,
) -> Result<Value, ApiError> {
    sqlx::query_scalar("SELECT to_jsonb(q) FROM zuse_rust_task_command_queue q WHERE workspace_id=$1 AND session_id=$2 AND queue_id=$3 FOR UPDATE")
        .bind(workspace).bind(session).bind(queue).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::not_found("cloud_task_queue_not_found"))
}
impl RuntimeControl {
    pub(super) async fn task_queue_action_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: &Value,
        access: &Access,
        request: &wire::Request,
        now: i64,
    ) -> Result<Option<Value>, ApiError> {
        let id = required(workspace, "workspace_id")?;
        let owner = &access.owner_id;
        let session = &request.session_id;
        match &request.action {
            wire::Action::Queue {
                queue_id,
                message_id,
                input,
                hold_for_turn_end,
            } => {
                let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND session_id=$2 AND state='queued'").bind(id).bind(session).fetch_one(&mut **tx).await?;
                if count >= wire::MAX_QUEUE_ROWS {
                    return Err(error("cloud_task_queue_full"));
                }
                let ciphertext = self
                    .master
                    .seal_api(
                        &context(owner, id, session, "queue", queue_id),
                        &serde_json::to_string(input)
                            .map_err(|_| ApiError::bad_request("cloud_task_input_invalid"))?,
                    )
                    .map_err(|_| ApiError::unavailable("cloud_task_command_encryption_failed"))?;
                let position:i64=sqlx::query_scalar("SELECT COALESCE(MAX(position),0)+1 FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND session_id=$2").bind(id).bind(session).fetch_one(&mut **tx).await?;
                let inserted=sqlx::query("INSERT INTO zuse_rust_task_command_queue(workspace_id,session_id,queue_id,message_id,actor_id,actor,revision,position,state,hold_for_turn_end,ciphertext,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,1,$7,'queued',$8,$9,$10,$10) ON CONFLICT DO NOTHING")
                    .bind(id).bind(session).bind(queue_id).bind(message_id).bind(&access.actor.account_id).bind(authorized_actor(access)).bind(position).bind(hold_for_turn_end).bind(ciphertext).bind(now).execute(&mut **tx).await?;
                if inserted.rows_affected() != 1 {
                    return Err(error("cloud_task_queue_identity_conflict"));
                }
                Ok(Some(
                    json!({"id":queue_id,"messageId":message_id,"revision":1,"state":"queued","durablyAdmitted":true}),
                ))
            }
            wire::Action::Edit {
                queue_id,
                revision,
                input,
                lease_id,
            } => {
                let item = row(tx, id, session, queue_id).await?;
                if item["revision"] != *revision {
                    return Err(conflict());
                }
                editable(
                    self,
                    &item,
                    access,
                    id,
                    session,
                    queue_id,
                    lease_id.as_deref(),
                    now,
                )?;
                let mut input = input.clone();
                if input.run_selection.is_none() {
                    input.run_selection = self
                        .open_queue_input(workspace, session, &item)?
                        .run_selection;
                }
                let ciphertext = self
                    .master
                    .seal_api(
                        &context(owner, id, session, "queue", queue_id),
                        &serde_json::to_string(&input)
                            .map_err(|_| ApiError::bad_request("cloud_task_input_invalid"))?,
                    )
                    .map_err(|_| ApiError::unavailable("cloud_task_command_encryption_failed"))?;
                sqlx::query("UPDATE zuse_rust_task_command_queue SET ciphertext=$4,revision=revision+1,actor_id=$5,actor=$6,edit_lease_hash=NULL,edit_actor_id=NULL,edit_device_id=NULL,edit_instance_id=NULL,edit_expires_at=NULL,updated_at=$7 WHERE workspace_id=$1 AND session_id=$2 AND queue_id=$3")
                    .bind(id).bind(session).bind(queue_id).bind(ciphertext).bind(&access.actor.account_id).bind(authorized_actor(access)).bind(now).execute(&mut **tx).await?;
                Ok(Some(
                    json!({"changed":true,"id":queue_id,"revision":revision+1}),
                ))
            }
            wire::Action::Delete {
                queue_id,
                revision,
                lease_id,
            } => {
                let item = row(tx, id, session, queue_id).await?;
                if item["revision"] != *revision {
                    return Err(conflict());
                }
                editable(
                    self,
                    &item,
                    access,
                    id,
                    session,
                    queue_id,
                    lease_id.as_deref(),
                    now,
                )?;
                sqlx::query("UPDATE zuse_rust_task_command_queue SET state='deleted',revision=revision+1,edit_lease_hash=NULL,edit_actor_id=NULL,edit_device_id=NULL,edit_instance_id=NULL,edit_expires_at=NULL,updated_at=$4 WHERE workspace_id=$1 AND session_id=$2 AND queue_id=$3")
                    .bind(id).bind(session).bind(queue_id).bind(now).execute(&mut **tx).await?;
                Ok(Some(
                    json!({"removed":true,"id":queue_id,"revision":revision+1}),
                ))
            }
            wire::Action::Move {
                queue_id,
                revision,
                to_index,
            } => {
                let item = row(tx, id, session, queue_id).await?;
                if item["revision"] != *revision {
                    return Err(conflict());
                }
                editable(self, &item, access, id, session, queue_id, None, now)?;
                let mut ids:Vec<String>=sqlx::query_scalar("SELECT queue_id FROM zuse_rust_task_command_queue WHERE workspace_id=$1 AND session_id=$2 AND state='queued' ORDER BY position,queue_id LIMIT 129 FOR UPDATE")
                    .bind(id).bind(session).fetch_all(&mut **tx).await?;
                if ids.len() > wire::MAX_QUEUE_ROWS as usize {
                    return Err(error("cloud_task_queue_full"));
                }
                ids.retain(|v| v != queue_id);
                ids.insert((*to_index).min(ids.len()), queue_id.clone());
                for (position, item_id) in ids.into_iter().enumerate() {
                    sqlx::query("UPDATE zuse_rust_task_command_queue SET position=$4,revision=revision+CASE WHEN queue_id=$5 THEN 1 ELSE 0 END,updated_at=$6 WHERE workspace_id=$1 AND session_id=$2 AND queue_id=$3")
                        .bind(id).bind(session).bind(item_id).bind(position as i64).bind(queue_id).bind(now).execute(&mut **tx).await?;
                }
                Ok(Some(
                    json!({"changed":true,"id":queue_id,"revision":revision+1}),
                ))
            }
            wire::Action::BeginEdit {
                queue_id,
                revision,
                editor_device_id,
                editor_instance_id,
            } => {
                let item = row(tx, id, session, queue_id).await?;
                if item["revision"] != *revision {
                    return Err(conflict());
                }
                editable(self, &item, access, id, session, queue_id, None, now)?;
                let token = uuid::Uuid::new_v4().to_string();
                let expiry = now + wire::EDIT_LEASE_MS;
                sqlx::query("UPDATE zuse_rust_task_command_queue SET edit_lease_hash=$4,edit_actor_id=$5,edit_device_id=$6,edit_instance_id=$7,edit_expires_at=$8,updated_at=$9 WHERE workspace_id=$1 AND session_id=$2 AND queue_id=$3")
                    .bind(id).bind(session).bind(queue_id).bind(lease_hash(self,owner,id,session,queue_id,&token)).bind(&access.actor.account_id).bind(editor_device_id).bind(editor_instance_id).bind(expiry).bind(now).execute(&mut **tx).await?;
                Ok(Some(
                    json!({"id":queue_id,"leaseId":token,"revision":revision,"expiresAt":expiry}),
                ))
            }
            wire::Action::RenewEdit { queue_id, lease_id }
            | wire::Action::CancelEdit { queue_id, lease_id } => {
                let item = row(tx, id, session, queue_id).await?;
                // Expiry never silently releases the persisted review gate.
                // Its original editor can explicitly release unchanged text
                // even after expiry, but cannot renew an expired lease.
                if item["state"] != "queued"
                    || item["edit_actor_id"] != access.actor.account_id
                    || item["edit_lease_hash"]
                        != lease_hash(self, owner, id, session, queue_id, lease_id)
                {
                    return Err(error("cloud_task_queue_edit_locked"));
                }
                if matches!(request.action, wire::Action::RenewEdit { .. })
                    && item["edit_expires_at"].as_i64().is_none_or(|v| v <= now)
                {
                    return Err(error("cloud_task_queue_edit_expired"));
                }
                editable(
                    self,
                    &item,
                    access,
                    id,
                    session,
                    queue_id,
                    Some(lease_id),
                    now,
                )?;
                if matches!(request.action, wire::Action::CancelEdit { .. }) {
                    sqlx::query("UPDATE zuse_rust_task_command_queue SET edit_lease_hash=NULL,edit_actor_id=NULL,edit_device_id=NULL,edit_instance_id=NULL,edit_expires_at=NULL,updated_at=$4 WHERE workspace_id=$1 AND session_id=$2 AND queue_id=$3").bind(id).bind(session).bind(queue_id).bind(now).execute(&mut **tx).await?;
                    Ok(Some(json!({"cancelled":true,"id":queue_id})))
                } else {
                    let expiry = now + wire::EDIT_LEASE_MS;
                    sqlx::query("UPDATE zuse_rust_task_command_queue SET edit_expires_at=$4,updated_at=$5 WHERE workspace_id=$1 AND session_id=$2 AND queue_id=$3").bind(id).bind(session).bind(queue_id).bind(expiry).bind(now).execute(&mut **tx).await?;
                    Ok(Some(
                        json!({"id":queue_id,"leaseId":lease_id,"revision":item["revision"],"expiresAt":expiry}),
                    ))
                }
            }
            wire::Action::SendNow { queue_id, revision }
            | wire::Action::SteerNow {
                queue_id, revision, ..
            } => {
                let item = row(tx, id, session, queue_id).await?;
                if item["revision"] != *revision {
                    return Err(conflict());
                }
                if item["state"] != "queued" {
                    return Err(error("cloud_task_queue_already_claimed"));
                }
                if !item["edit_lease_hash"].is_null() {
                    return Err(error("cloud_task_queue_review_required"));
                }
                // An explicit edit can transfer authorship, but merely pressing
                // Send must not borrow another member's prior authorization.
                if item["actor_id"] != access.actor.account_id
                    || item["actor"]
                        != serde_json::to_value(authorized_actor(access)).map_err(|_| conflict())?
                {
                    return Err(error("cloud_task_queue_author_review_required"));
                }
                let input = self.open_queue_input(workspace, session, &item)?;
                self.verify_launch_assets(tx, owner, id, &input.attachments)
                    .await?;
                let message = required(&item, "message_id")?;
                let run = self
                    .queued_run_tx(tx, workspace, session, message, &input)
                    .await?;
                let result = if let wire::Action::SteerNow { based_on, .. } = &request.action {
                    if !input.attachments.is_empty() || !input.computer_files.is_empty() {
                        return Err(error("cloud_task_steer_attachments_unsupported"));
                    }
                    let current:Option<String>=sqlx::query_scalar("SELECT e.turn_id FROM zuse_rust_task_command_execution e JOIN zuse_rust_mailbox_commands c USING(workspace_id,command_id) WHERE e.workspace_id=$1 AND e.session_id=$2 AND (c.lane=$3 OR (c.lane='task-control:'||e.session_id AND c.envelope->>'kind'='zuse.session.steer' AND e.message_id IS NOT NULL AND e.state IN ('starting','running','waiting-input','outcome-unknown'))) AND e.state IN ('running','waiting-input') AND e.turn_id IS NOT NULL ORDER BY c.workspace_sequence LIMIT 1").bind(id).bind(session).bind(format!("task-execution:{session}")).fetch_optional(&mut **tx).await?;
                    let turn = current.ok_or_else(|| error("cloud_task_steer_turn_required"))?;
                    if let Some(selected) = &input.run_selection {
                        let actual = self
                            .active_task_run_selection_tx(tx, workspace, session)
                            .await?;
                        if actual.as_ref() != Some(selected) {
                            return Err(error("cloud_task_steer_selection_changed"));
                        }
                    }
                    if based_on
                        .as_ref()
                        .and_then(|v| v.turn_id.as_ref())
                        .is_some_and(|v| v != &turn)
                    {
                        return Err(error("cloud_task_steer_turn_changed"));
                    }
                    let mut based_on = based_on.clone().unwrap_or_default();
                    based_on.turn_id = Some(turn);
                    let zuse_protocol::session_commands::SessionCommandPayload::Run {
                        request: run,
                        ..
                    } = run
                    else {
                        return Err(conflict());
                    };
                    let payload = zuse_protocol::session_commands::SessionCommandPayload::Steer {
                        prompt: run.prompt,
                        message_id: Some(message.into()),
                    };
                    self.task_seal_submit_tx(
                        tx,
                        workspace,
                        access,
                        &request.command_id,
                        session,
                        message,
                        &payload,
                        &[],
                        Some(based_on),
                        now,
                    )
                    .await?
                } else {
                    let control = format!(
                        "interrupt:{}",
                        self.master.digest_api(
                            &context(owner, id, session, "send-now", queue_id),
                            &request.command_id
                        )
                    );
                    self.task_seal_interrupt_dependency_tx(
                        tx, workspace, access, &control, session, message, now,
                    )
                    .await?;
                    let accepted = self
                        .task_seal_submit_tx(
                            tx,
                            workspace,
                            access,
                            &request.command_id,
                            session,
                            message,
                            &run,
                            &input.attachments,
                            None,
                            now,
                        )
                        .await?;
                    sqlx::query("UPDATE zuse_rust_task_command_execution SET priority=1,wait_for_control_id=$3 WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(&request.command_id).bind(&control).execute(&mut **tx).await?;
                    accepted
                };
                sqlx::query("UPDATE zuse_rust_task_command_queue SET state='claimed',claimed_command_id=$4,updated_at=$5 WHERE workspace_id=$1 AND session_id=$2 AND queue_id=$3").bind(id).bind(session).bind(queue_id).bind(&request.command_id).bind(now).execute(&mut **tx).await?;
                let mut result = result;
                result["id"] = json!(queue_id);
                result["messageId"] = json!(message);
                result["revision"] = json!(revision);
                result["state"] = json!("claimed");
                Ok(Some(result))
            }
            wire::Action::PauseQueue { paused } => {
                sqlx::query("UPDATE zuse_rust_task_command_sessions SET queue_paused=$3 WHERE workspace_id=$1 AND session_id=$2").bind(id).bind(session).bind(paused).execute(&mut **tx).await?;
                sqlx::query("INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,workspace_id,payload,created_at) VALUES($1,'task-command-queue',$2,$3,'{\"commandAdmissionVersion\":2}'::jsonb,$4)")
                    .bind(owner).bind(session).bind(id).bind(now).execute(&mut **tx).await?;
                Ok(Some(json!({"paused":paused})))
            }
            _ => Ok(None),
        }
    }
}
