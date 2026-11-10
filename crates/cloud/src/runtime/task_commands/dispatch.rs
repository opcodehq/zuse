//! Delivery admission is not a completed model turn. The API retains one
//! execution lane per conversation until factual terminal progress or review.
use super::*;
/// Appended only for negotiated v2 rows; legacy schemas must never parse a
/// reference to these optional tables merely because a SQL boolean is false.
// Steer stays on the control transport lane so it can join a live Run. Once
// its inference starts it owns the same conversation exclusion as a Run;
// queued/message-less controls must never become inference barriers.
pub(in crate::runtime) const EXECUTION_GUARD: &str = r#" AND (c.lane NOT LIKE 'task-execution:%' OR (
 EXISTS(SELECT 1 FROM zuse_rust_task_command_execution current WHERE current.workspace_id=c.workspace_id AND current.command_id=c.command_id AND current.state IN ('queued','starting','running','waiting-input') AND (current.wait_for_control_id IS NULL OR EXISTS(SELECT 1 FROM zuse_rust_mailbox_commands control WHERE control.workspace_id=c.workspace_id AND control.command_id=current.wait_for_control_id AND control.state='applied')))
 AND NOT EXISTS(SELECT 1 FROM zuse_rust_mailbox_commands prior JOIN zuse_rust_task_command_execution e ON e.workspace_id=prior.workspace_id AND e.command_id=prior.command_id JOIN zuse_rust_task_command_execution current ON current.workspace_id=c.workspace_id AND current.command_id=c.command_id WHERE prior.workspace_id=c.workspace_id AND (prior.lane=c.lane OR (prior.lane='task-control:'||current.session_id AND e.session_id=current.session_id AND prior.envelope->>'kind'='zuse.session.steer' AND e.message_id IS NOT NULL AND e.state IN ('starting','running','waiting-input','outcome-unknown'))) AND prior.command_id<>c.command_id AND (
 (e.state='outcome-unknown' AND NOT e.unknown_reviewed) OR e.state IN ('starting','running','waiting-input') OR
 (e.state='queued' AND (prior.ever_leased OR prior.state IN ('leased','lease-expired-awaiting-fence','applied'))) OR
 (e.state='queued' AND (e.priority>current.priority OR (e.priority=current.priority AND prior.workspace_sequence<c.workspace_sequence)))
 ))))"#;

impl RuntimeControl {
    pub(in crate::runtime) async fn task_fence_previous_executions_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: &str,
        generation: i64,
        disk: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        let unknown:Vec<String>=sqlx::query_scalar("UPDATE zuse_rust_task_command_execution SET state='outcome-unknown',error_code='runtime-execution-replaced',unknown_reviewed=false,updated_at=$4 WHERE workspace_id=$1 AND runtime_generation IS NOT NULL AND (runtime_generation<>$2 OR storage_incarnation_id<>$3) AND state IN ('queued','starting','running','waiting-input') RETURNING command_id").bind(id).bind(generation).bind(disk).bind(now).fetch_all(&mut **tx).await?;
        for command in unknown {
            // Applied still means its original delivery was admitted. Keep
            // that truthful receipt separate from the now-uncertain execution.
            sqlx::query("UPDATE zuse_rust_mailbox_commands SET state='outcome-unknown',category='runtime-execution-replaced',lease_token=NULL,lease_deadline=NULL,updated_at=$3 WHERE workspace_id=$1 AND command_id=$2 AND state NOT IN ('applied','rejected','expired','cancelled','outcome-unknown')").bind(id).bind(command).bind(now).execute(&mut **tx).await?;
        }
        Ok(())
    }
    pub(in crate::runtime) async fn task_delivery_fact_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: &str,
        ack: &zuse_protocol::cloud_commands::RuntimeAcknowledgment,
        now: i64,
    ) -> Result<(), ApiError> {
        use zuse_protocol::cloud_commands::AcknowledgmentState;
        let (state, code) = match ack.state {
            AcknowledgmentState::Applied => return Ok(()), // admission is not completion
            AcknowledgmentState::Rejected => ("failed", "command-admission-rejected"),
            AcknowledgmentState::Expired => ("cancelled", "command-expired"),
            AcknowledgmentState::OutcomeUnknown => ("outcome-unknown", "delivery-outcome-unknown"),
        };
        sqlx::query("UPDATE zuse_rust_task_command_execution SET state=$3,error_code=$4,unknown_reviewed=false,updated_at=$5 WHERE workspace_id=$1 AND command_id=$2 AND fingerprint=$6 AND state IN ('queued','starting','running','waiting-input')").bind(id).bind(&ack.command_id).bind(state).bind(code).bind(now).bind(&ack.fingerprint).execute(&mut **tx).await?;
        // Only never-dispatched dependent Runs are proven not executed. A
        // failed Interrupt never fabricates a terminal outcome for its target.
        let dependents:Vec<String>=sqlx::query_scalar("UPDATE zuse_rust_task_command_execution e SET state='cancelled',error_code='interrupt-admission-unconfirmed',updated_at=$3 WHERE e.workspace_id=$1 AND e.wait_for_control_id=$2 AND e.state='queued' AND EXISTS(SELECT 1 FROM zuse_rust_mailbox_commands c WHERE c.workspace_id=e.workspace_id AND c.command_id=e.command_id AND NOT c.ever_leased AND c.state IN ('accepted','waiting-for-runtime')) RETURNING command_id").bind(id).bind(&ack.command_id).bind(now).fetch_all(&mut **tx).await?;
        for command in dependents {
            super::super::mailbox::change(tx,id,&command,"state='cancelled',category='interrupt-admission-unconfirmed',lease_token=NULL,lease_deadline=NULL",now).await?;
        }
        let workspace: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
        let session:Option<String>=sqlx::query_scalar("SELECT session_id FROM zuse_rust_task_command_execution WHERE workspace_id=$1 AND command_id=$2").bind(id).bind(&ack.command_id).fetch_optional(&mut **tx).await?;
        if let Some(session) = session {
            self.enqueue_task_drain_tx(tx, &workspace, &session, now)
                .await?;
        }

        Ok(())
    }
    pub(in crate::runtime) async fn task_lease_dispatch_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: &str,
        command: &str,
        token: &str,
        deadline: i64,
        generation: i64,
        sandbox: &str,
        disk: &str,
        now: i64,
    ) -> Result<(), ApiError> {
        // Retain one revision per command and the exact execution binding
        // predicate. The caller still owns the workspace lock and transaction;
        // a failed execution fence rolls back both the lease and revision.
        let (leased,bound):(i64,i64)=sqlx::query_as("WITH revision AS (UPDATE zuse_rust_mailboxes SET revision=revision+1 WHERE workspace_id=$1 RETURNING revision), leased AS (UPDATE zuse_rust_mailbox_commands SET state='leased',ever_leased=true,lease_token=$3,lease_deadline=$4,runtime_generation=$5,provider_sandbox_id=$6,storage_incarnation_id=$7,revision=revision.revision,updated_at=$8 FROM revision WHERE workspace_id=$1 AND command_id=$2 RETURNING command_id), bound AS (UPDATE zuse_rust_task_command_execution e SET runtime_generation=$5,storage_incarnation_id=$7,updated_at=$8 FROM leased l WHERE e.workspace_id=$1 AND e.command_id=l.command_id AND e.state='queued' AND ((e.runtime_generation IS NULL AND e.storage_incarnation_id IS NULL) OR (e.runtime_generation=$5 AND e.storage_incarnation_id=$7)) RETURNING e.command_id) SELECT (SELECT count(*) FROM leased),(SELECT count(*) FROM bound)")
            .bind(id).bind(command).bind(token).bind(deadline).bind(generation).bind(sandbox).bind(disk).bind(now).fetch_one(&mut **tx).await?;
        if leased != 1 || bound != 1 {
            return Err(error("cloud_task_dispatch_execution_fenced"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
