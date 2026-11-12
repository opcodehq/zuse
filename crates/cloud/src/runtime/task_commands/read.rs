//! Compute-independent queue and admission projections. These are distinct from
//! guest checkpoints: no runtime generation, stream epoch or cursor is invented.
use super::*;
impl RuntimeControl {
    pub(super) async fn task_read_workspace_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: &str,
        access: &Access,
    ) -> Result<Value, ApiError> {
        let row:Value=sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 AND account_id=$2")
            .bind(id).bind(&access.owner_id).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::not_found("cloud_workspace_not_found"))?;
        access.workspace_permission(&access.owner_id, &row["request_config"])?;
        if !command_mode(&row)
            || matches!(row["state"].as_str(), Some("deleting" | "deleted"))
            || row["desired_state"] == "deleted"
        {
            return Err(ApiError::not_found("cloud_task_command_not_found"));
        }
        Ok(row)
    }
    pub async fn task_command_queue(
        &self,
        id: &str,
        access: &Access,
        session: &str,
    ) -> Result<Value, ApiError> {
        if !validation::id(session) {
            return Err(ApiError::bad_request("cloud_task_session_invalid"));
        }
        let fresh = self
            .scope_access(id, Some(&access.actor.account_id), false)
            .await?;
        let authorized = fresh.as_ref().map_or(access, |(access, _)| access);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let workspace = self.task_read_workspace_tx(&mut tx, id, authorized).await?;
        if fresh
            .as_ref()
            .is_some_and(|(_, policy)| workspace["request_config"]["sharingPolicy"] != *policy)
        {
            return Err(ApiError::conflict("workspace_sharing_changed"));
        }
        let paused:bool=sqlx::query_scalar("SELECT queue_paused FROM zuse_rust_task_command_sessions WHERE workspace_id=$1 AND session_id=$2 AND state='active'")
            .bind(id).bind(session).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("cloud_task_session_not_registered"))?;
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(q) FROM zuse_rust_task_command_queue q WHERE workspace_id=$1 AND session_id=$2 AND state='queued' ORDER BY position,queue_id LIMIT 129")
            .bind(id).bind(session).fetch_all(&mut *tx).await?;
        if rows.len() > wire::MAX_QUEUE_ROWS as usize {
            return Err(ApiError::unavailable("cloud_task_queue_projection_invalid"));
        }
        let mut queue = Vec::with_capacity(rows.len());
        for row in rows {
            let key = required(&row, "queue_id")?;
            let text = self
                .master
                .open_api(
                    &context(&access.owner_id, id, session, "queue", key),
                    required(&row, "ciphertext")?,
                )
                .map_err(|_| ApiError::unavailable("cloud_task_queue_unavailable"))?;
            let input: wire::Input = serde_json::from_slice(&text)
                .map_err(|_| ApiError::unavailable("cloud_task_queue_unavailable"))?;
            queue.push(json!({"id":key,"messageId":row["message_id"],"revision":row["revision"],"position":row["position"],"input":input,"text":input.text,"attachments":input.attachments,"computerFiles":input.computer_files,"fileRefs":input.file_refs,"skillRefs":input.skill_refs,"annotations":input.annotations,"asGoal":input.as_goal,"holdForTurnEnd":row["hold_for_turn_end"],"createdAt":row["created_at"],"editing":if row["edit_lease_hash"].is_string(){json!({"actorId":row["edit_actor_id"],"expiresAt":row["edit_expires_at"],"reviewRequired":true})}else{Value::Null}}));
        }
        let queue_error:Option<String>=sqlx::query_scalar("SELECT last_error FROM zuse_rust_task_command_drains WHERE workspace_id=$1 AND session_id=$2").bind(id).bind(session).fetch_optional(&mut *tx).await?.flatten();
        tx.commit().await?;
        Ok(json!({"sessionId":session,"items":queue,"paused":paused,"queueError":queue_error}))
    }
    pub async fn task_command_status(
        &self,
        id: &str,
        access: &Access,
        command: &str,
    ) -> Result<Value, ApiError> {
        if !validation::id(command) {
            return Err(ApiError::bad_request("cloud_task_command_identity_invalid"));
        }
        let fresh = self
            .scope_access(id, Some(&access.actor.account_id), false)
            .await?;
        let authorized = fresh.as_ref().map_or(access, |(access, _)| access);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let workspace = self.task_read_workspace_tx(&mut tx, id, authorized).await?;
        if fresh
            .as_ref()
            .is_some_and(|(_, policy)| workspace["request_config"]["sharingPolicy"] != *policy)
        {
            return Err(ApiError::conflict("workspace_sharing_changed"));
        }
        // Do not return encrypted private command bodies, edit tokens or the
        // original acceptance object (which can contain another editor's lease).
        let result:Value=sqlx::query_scalar("SELECT jsonb_build_object('commandId',a.command_id,'executionCommandId',e.command_id,'sessionId',a.session_id,'durablyAdmitted',true,'createdAt',a.created_at,'deliveryState',m.state,'executionState',e.state,'errorCode',e.error_code,'updatedAt',e.updated_at) FROM zuse_rust_task_command_actions a LEFT JOIN zuse_rust_task_command_queue q ON q.workspace_id=a.workspace_id AND q.session_id=a.session_id AND q.queue_id=a.acceptance->>'id' LEFT JOIN zuse_rust_mailbox_commands m ON m.workspace_id=a.workspace_id AND m.command_id=coalesce(q.claimed_command_id,a.command_id) LEFT JOIN zuse_rust_task_command_execution e ON e.workspace_id=a.workspace_id AND e.command_id=coalesce(q.claimed_command_id,a.command_id) WHERE a.workspace_id=$1 AND a.command_id=$2 UNION ALL SELECT jsonb_build_object('commandId',e.command_id,'executionCommandId',e.command_id,'sessionId',e.session_id,'durablyAdmitted',true,'createdAt',e.created_at,'deliveryState',m.state,'executionState',e.state,'errorCode',e.error_code,'updatedAt',e.updated_at) FROM zuse_rust_task_command_execution e LEFT JOIN zuse_rust_mailbox_commands m USING(workspace_id,command_id) WHERE e.workspace_id=$1 AND e.command_id=$2 AND NOT EXISTS(SELECT 1 FROM zuse_rust_task_command_actions a WHERE a.workspace_id=e.workspace_id AND a.command_id=e.command_id)")
            .bind(id).bind(command).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("cloud_task_command_not_found"))?;
        tx.commit().await?;
        Ok(result)
    }
    pub(in crate::runtime) async fn task_command_admissions_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        session: &str,
    ) -> Result<Vec<Value>, ApiError> {
        if !command_mode(row) {
            return Ok(vec![]);
        }
        let id = required(row, "workspace_id")?;
        let owner = required(row, "account_id")?;
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(e) FROM ((SELECT * FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND session_id=$2 AND message_id IS NOT NULL ORDER BY created_at DESC,command_id DESC LIMIT 64) UNION (SELECT * FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND session_id=$2 AND state='outcome-unknown' AND NOT unknown_reviewed ORDER BY created_at,command_id LIMIT 128))e ORDER BY created_at,command_id")
            .bind(id).bind(session).fetch_all(&mut **tx).await?;
        let mut result = Vec::with_capacity(rows.len());
        for item in rows {
            let cid = required(&item, "command_id")?;
            let text = self
                .master
                .open_api(
                    &context(owner, id, session, "presentation", cid),
                    required(&item, "presentation_ciphertext")?,
                )
                .map_err(|_| ApiError::unavailable("cloud_task_admission_unavailable"))?;
            let mut value: Value = serde_json::from_slice(&text)
                .map_err(|_| ApiError::unavailable("cloud_task_admission_unavailable"))?;
            value["commandId"] = json!(cid);
            value["messageId"] = item["message_id"].clone();
            value["createdAt"] = item["created_at"].clone();
            value["executionState"] = item["state"].clone();
            value["sessionId"] = json!(session);
            value["unknownReviewed"] = item["unknown_reviewed"].clone();
            value["errorCode"] = item["error_code"].clone();
            result.push(value);
        }
        Ok(result)
    }
}
