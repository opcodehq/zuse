//! Delivery ACKs do not advance this state. Only a generation/disk/fingerprint-
//! bound runtime execution report may do so, and terminal facts are immutable.
use super::*;
fn transition(before: &str, after: wire::ExecutionState) -> bool {
    use wire::ExecutionState::*;
    match before {
        "queued" => matches!(after, Starting | Cancelled | Failed | OutcomeUnknown),
        "starting" => matches!(
            after,
            Starting | Running | WaitingInput | Completed | Cancelled | Failed | OutcomeUnknown
        ),
        "running" => matches!(
            after,
            Running | WaitingInput | Completed | Cancelled | Failed | OutcomeUnknown
        ),
        "waiting-input" => matches!(
            after,
            Running | WaitingInput | Completed | Cancelled | Failed | OutcomeUnknown
        ),
        _ => false,
    }
}
impl RuntimeControl {
    pub async fn task_command_progress(
        &self,
        id: &str,
        credential: &str,
        report: &wire::Progress,
        now: i64,
    ) -> Result<Value, ApiError> {
        if !validation::id(&report.command_id)
            || report.revision < 1
            || !validation::id(&report.storage_incarnation_id)
            || report.turn_id.as_ref().is_some_and(|v| !validation::id(v))
            || report.error_code.as_ref().is_some_and(|v| {
                v.len() > 128
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            })
        {
            return Err(ApiError::bad_request("cloud_task_progress_invalid"));
        }
        let early: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        super::super::mailbox::runtime_auth(&early, credential, now)?;
        self.scope_access(id, None, true).await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, id).await?;
        super::super::mailbox::runtime_auth(&row, credential, now)?;
        if !command_mode(&row)
            || !native_commands_supported(&row)
            || row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
                != report.storage_incarnation_id
            || !storage_enrolled(&row)
        {
            return Err(error("cloud_task_progress_runtime_fenced"));
        }
        let existing:Value=sqlx::query_scalar("SELECT to_jsonb(e) FROM zuse_rust_task_command_execution e WHERE workspace_id=$1 AND command_id=$2 FOR UPDATE").bind(id).bind(&report.command_id).fetch_optional(&mut *tx).await?.ok_or_else(||ApiError::not_found("cloud_task_command_not_found"))?;
        if existing["fingerprint"] != report.fingerprint
            || existing["runtime_generation"] != generation
            || existing["storage_incarnation_id"] != report.storage_incarnation_id
        {
            return Err(error("cloud_task_progress_runtime_fenced"));
        }
        let owner = required(&row, "account_id")?;
        let body = serde_json::to_string(report)
            .map_err(|_| ApiError::bad_request("cloud_task_progress_invalid"))?;
        let digest = self.master.digest_api(
            &format!(
                "task-progress:{owner}:{id}:{}:{generation}",
                report.command_id
            ),
            &body,
        );
        let revision = existing["runtime_revision"].as_i64().unwrap_or(0);
        if report.revision < revision
            && matches!(
                existing["state"].as_str(),
                Some("completed" | "cancelled" | "failed" | "outcome-unknown")
            )
        {
            tx.commit().await?;
            return Ok(
                json!({"commandId":report.command_id,"revision":revision,"state":existing["state"],"terminal":true,"superseded":true}),
            );
        }
        if report.revision <= revision {
            if report.revision != revision || existing["runtime_report_digest"] != digest {
                return Err(error("cloud_task_progress_revision_conflict"));
            }
            tx.commit().await?;
            return Ok(
                json!({"commandId":report.command_id,"revision":revision,"state":existing["state"]}),
            );
        }
        // A negative durable delivery receipt or an interruption fence may
        // terminalize the command before its in-flight progress reports land.
        // Return that authority unchanged; never reopen or infer completion.
        if matches!(
            existing["state"].as_str(),
            Some("completed" | "cancelled" | "failed" | "outcome-unknown")
        ) {
            tx.commit().await?;
            return Ok(
                json!({"commandId":report.command_id,"revision":revision,"state":existing["state"],"terminal":true,"superseded":true}),
            );
        }
        if !transition(existing["state"].as_str().unwrap_or(""), report.state) {
            return Err(error("cloud_task_progress_terminal_or_invalid_transition"));
        }
        let state = serde_json::to_value(report.state)
            .map_err(|_| ApiError::bad_request("cloud_task_progress_invalid"))?;
        sqlx::query("UPDATE zuse_rust_task_command_execution SET state=$3,runtime_revision=$4,runtime_report_digest=$5,turn_id=$6,error_code=$7,updated_at=$8 WHERE workspace_id=$1 AND command_id=$2")
            .bind(id).bind(&report.command_id).bind(state.as_str().unwrap_or("outcome-unknown")).bind(report.revision).bind(digest).bind(&report.turn_id).bind(&report.error_code).bind(now).execute(&mut *tx).await?;
        // Only accepted new runtime revisions count; duplicate/stale reports
        // returned above. Terminal completion always starts a full idle window.
        self.record_activity_tx(&mut tx, &row, now, report.state.terminal())
            .await?;
        // The computer fetches every server-held file a run binds before its
        // turn starts, so these states prove delivery. A command that ends
        // without starting only stops holding its files.
        let release = match report.state {
            wire::ExecutionState::Running
            | wire::ExecutionState::WaitingInput
            | wire::ExecutionState::Completed => {
                self.settle_command_assets_tx(&mut tx, id, &report.command_id, true, now)
                    .await?
            }
            state if state.terminal() => {
                self.settle_command_assets_tx(&mut tx, id, &report.command_id, false, now)
                    .await?
            }
            _ => vec![],
        };
        if report.state.terminal() {
            self.enqueue_task_drain_tx(&mut tx, &row, required(&existing, "session_id")?, now)
                .await?;
        }
        tx.commit().await?;
        self.notify_task_drain();
        // Object deletes run after commit; the sweep retries a failure.
        if !release.is_empty()
            && let Err(error) = self.release_assets(id, &release, now).await
        {
            tracing::warn!(code = error.code, "Attachment object release deferred");
        }
        Ok(json!({"commandId":report.command_id,"revision":report.revision,"state":state}))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delivery_admission_cannot_complete_a_turn_or_reopen_a_terminal_outcome() {
        assert!(!transition("queued", wire::ExecutionState::Completed));
        assert!(transition("starting", wire::ExecutionState::Completed));
        assert!(!transition("completed", wire::ExecutionState::Running));
        assert!(!transition(
            "outcome-unknown",
            wire::ExecutionState::Completed
        ));
        assert!(!transition("running", wire::ExecutionState::Starting));
    }
}
