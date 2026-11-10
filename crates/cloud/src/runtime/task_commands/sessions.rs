//! API-registered conversations render before a guest document/checkpoint exists.
use super::*;
impl RuntimeControl {
    fn task_registered_chat(
        &self,
        row: &Value,
        policy: &Policy,
        session: &Value,
    ) -> Result<Value, ApiError> {
        let id = required(row, "workspace_id")?;
        let owner = required(row, "account_id")?;
        let sid = required(session, "session_id")?;
        if session["thread_id"] != row["chat_id"] {
            return Err(error("cloud_task_session_identity_conflict"));
        }
        let metadata = if let Some(cipher) = session["metadata_ciphertext"].as_str() {
            let plain = self
                .master
                .open_api(&context(owner, id, sid, "session", "metadata"), cipher)
                .map_err(|_| ApiError::unavailable("cloud_task_session_unavailable"))?;
            serde_json::from_slice::<Value>(&plain)
                .map_err(|_| ApiError::unavailable("cloud_task_session_unavailable"))?
        } else {
            json!({})
        };
        let sandbox = match policy.runtime_mode {
            zuse_protocol::cloud::CloudRuntimeMode::FullAccess => "danger-full-access",
            zuse_protocol::cloud::CloudRuntimeMode::Auto => "auto",
            _ => "workspace-write",
        };
        let config=metadata.get("config").cloned().unwrap_or_else(||json!({"harness":if policy.agent=="claude"{"claude-code"}else{&policy.agent},"model":policy.initial_model,"reasoning":policy.reasoning,"modelOptions":policy.model_options,"sandbox":sandbox}));
        let at = session["created_at"]
            .as_i64()
            .ok_or_else(|| error("cloud_task_session_invalid"))?;
        let at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at)
            .ok_or_else(|| error("cloud_task_session_invalid"))?;
        let chat:zuse_protocol::Chat=serde_json::from_value(json!({"id":sid,"threadId":row["chat_id"],"deviceId":format!("cloud:{id}"),"title":metadata["title"],"archived":session["state"]=="archived","cwd":".","branch":row["branch"],"createdAt":at,"config":config})).map_err(|_|error("cloud_task_session_invalid"))?;
        serde_json::to_value(chat).map_err(|_| error("cloud_task_session_invalid"))
    }
    pub(in crate::runtime) async fn task_registered_session_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        session: &str,
    ) -> Result<Option<Value>, ApiError> {
        if !command_mode(row) {
            return Ok(None);
        }
        let entry:Value=sqlx::query_scalar("SELECT to_jsonb(s) FROM zuse_rust_task_command_sessions s WHERE workspace_id=$1 AND session_id=$2").bind(required(row,"workspace_id")?).bind(session).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::not_found("cloud_task_session_not_registered"))?;
        let (policy, _) = self.task_policy_tx(tx, row).await?;
        let chat = self.task_registered_chat(row, &policy, &entry)?;
        Ok(Some(
            json!({"sessionId":session,"threadId":row["chat_id"],"createdAt":entry["created_at"],"queuePaused":entry["queue_paused"],"chat":chat}),
        ))
    }
    pub(in crate::runtime) async fn task_registered_chats_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
    ) -> Result<Vec<Value>, ApiError> {
        if !command_mode(row) {
            return Ok(vec![]);
        }
        let (policy, _) = self.task_policy_tx(tx, row).await?;
        let entries:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(s) FROM zuse_rust_task_command_sessions s WHERE workspace_id=$1 AND state='active' ORDER BY created_at DESC,session_id DESC LIMIT 64").bind(required(row,"workspace_id")?).fetch_all(&mut **tx).await?;
        entries
            .iter()
            .map(|entry| self.task_registered_chat(row, &policy, entry))
            .collect()
    }
    pub async fn task_command_sessions(
        &self,
        id: &str,
        access: &Access,
        after: Option<(i64, String)>,
    ) -> Result<Value, ApiError> {
        if after
            .as_ref()
            .is_some_and(|(at, sid)| *at < 0 || !validation::id(sid))
        {
            return Err(ApiError::bad_request("cloud_task_session_cursor_invalid"));
        }
        let fresh = self
            .scope_access(id, Some(&access.actor.account_id), false)
            .await?;
        let authorized = fresh.as_ref().map_or(access, |(access, _)| access);
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut *tx)
            .await?;
        let row = self.task_read_workspace_tx(&mut tx, id, authorized).await?;
        if fresh
            .as_ref()
            .is_some_and(|(_, policy)| row["request_config"]["sharingPolicy"] != *policy)
        {
            return Err(ApiError::conflict("workspace_sharing_changed"));
        }
        let (policy, _) = self.task_policy_tx(&mut tx, &row).await?;
        let rows:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(s) FROM zuse_rust_task_command_sessions s WHERE workspace_id=$1 AND state='active' AND ($2::bigint IS NULL OR (created_at,session_id)<($2,$3)) ORDER BY created_at DESC,session_id DESC LIMIT 65").bind(id).bind(after.as_ref().map(|a|a.0)).bind(after.as_ref().map(|a|a.1.as_str())).fetch_all(&mut *tx).await?;
        let more = rows.len() > 64;
        let rows = &rows[..rows.len().min(64)];
        let chats = rows
            .iter()
            .map(|entry| self.task_registered_chat(&row, &policy, entry))
            .collect::<Result<Vec<_>, _>>()?;
        let next = if more {
            rows.last()
                .map(|last| json!({"createdAt":last["created_at"],"sessionId":last["session_id"]}))
        } else {
            None
        };
        tx.commit().await?;
        Ok(json!({"workspaceId":id,"items":chats,"nextCursor":next}))
    }
}
