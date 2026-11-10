//! Read immutable active execution evidence; a later admitted chat config is not live state.
use super::*;
use zuse_cloud_runtime::mailbox::CommandKey;
use zuse_protocol::{cloud_commands::CloudSessionCommand, session_commands::SessionCommandPayload};

pub(super) fn from_run(run: &zuse_protocol::RunRequest) -> Option<wire::RunSelection> {
    Some(wire::RunSelection {
        harness: run.harness?,
        model: run.model.clone()?,
        reasoning: run.reasoning,
        model_options: run.model_options.clone(),
        sandbox: run.sandbox,
    })
}
impl RuntimeControl {
    pub(super) async fn active_task_run_selection_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: &Value,
        session: &str,
    ) -> Result<Option<wire::RunSelection>, ApiError> {
        let id = required(workspace, "workspace_id")?;
        let active: Option<Value> = sqlx::query_scalar("SELECT jsonb_build_object('commandId',e.command_id,'presentation',e.presentation_ciphertext,'envelope',c.envelope) FROM zuse_rust_task_command_execution e JOIN zuse_rust_mailbox_commands c USING(workspace_id,command_id) WHERE e.workspace_id=$1 AND e.session_id=$2 AND (c.lane=$3 OR (c.lane='task-control:'||e.session_id AND c.envelope->>'kind'='zuse.session.steer' AND e.message_id IS NOT NULL)) AND e.state IN ('running','waiting-input') AND e.turn_id IS NOT NULL ORDER BY c.workspace_sequence LIMIT 1")
            .bind(id).bind(session).bind(format!("task-execution:{session}")).fetch_optional(&mut **tx).await?;
        let Some(active) = active else {
            return Ok(None);
        };
        let envelope = serde_json::from_value(active["envelope"].clone())
            .map_err(|_| error("cloud_task_active_selection_unavailable"))?;
        let key = self
            .master
            .open_transcript_key(
                required(workspace, "account_id")?,
                id,
                required(workspace, "wrapped_transcript_key")?,
            )
            .map_err(|_| error("cloud_task_active_selection_unavailable"))?;
        let key = CommandKey::parse(&key)
            .map_err(|_| error("cloud_task_active_selection_unavailable"))?;
        let command: CloudSessionCommand = serde_json::from_slice(
            &key.decrypt(&envelope)
                .map_err(|_| error("cloud_task_active_selection_unavailable"))?,
        )
        .map_err(|_| error("cloud_task_active_selection_unavailable"))?;
        if let SessionCommandPayload::Run { request, .. } = command.payload {
            return Ok(from_run(&request));
        }
        // New Steer executions retain the exact prior live choice in encrypted
        // presentation metadata. Older executions remain unknown, never guessed.
        let plain = self
            .master
            .open_api(
                &context(
                    required(workspace, "account_id")?,
                    id,
                    session,
                    "presentation",
                    required(&active, "commandId")?,
                ),
                required(&active, "presentation")?,
            )
            .map_err(|_| error("cloud_task_active_selection_unavailable"))?;
        let value: Value = serde_json::from_slice(&plain)
            .map_err(|_| error("cloud_task_active_selection_unavailable"))?;
        serde_json::from_value(value.get("runSelection").cloned().unwrap_or(Value::Null))
            .map_err(|_| error("cloud_task_active_selection_unavailable"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_queue_input_has_no_selection_and_serializes_without_new_field() {
        let input: wire::Input = serde_json::from_value(json!({"text":"legacy queue"})).unwrap();
        assert!(input.run_selection.is_none());
        assert!(
            serde_json::to_value(input)
                .unwrap()
                .get("runSelection")
                .is_none()
        );
    }
    #[test]
    fn queued_selection_roundtrips_real_types_and_rejects_arbitrary_run_fields() {
        let input = json!({"text":"selected","runSelection":{"harness":"grok","model":"grok-test","reasoning":"high","modelOptions":{"speed":"fast"},"sandbox":"workspace-write"}});
        let decoded: wire::Input = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(decoded).unwrap()["runSelection"],
            input["runSelection"]
        );
        let mut bad = input;
        bad["runSelection"]["autoApprove"] = json!(true);
        assert!(serde_json::from_value::<wire::Input>(bad).is_err());
    }
}
