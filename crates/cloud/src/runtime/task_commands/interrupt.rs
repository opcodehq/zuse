//! Stop binds an already admitted Run while holding the same workspace lock as
//! runtime leasing. It cannot cancel future queue rows or a later native turn.
use super::*;
// Live message-bearing Steer inference uses the control transport lane;
// its lifetime, rather than its delivery ACK, governs conversation controls.
pub(super) struct BoundInterrupt {
    pub command_id: String,
    pub message_id: String,
    pub cancelled_before_dispatch: bool,
}
impl RuntimeControl {
    pub(super) async fn bind_task_interrupt_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        session: &str,
        based_on: Option<&zuse_protocol::session_commands::CommandBasedOn>,
        now: i64,
    ) -> Result<Option<BoundInterrupt>, ApiError> {
        let id = required(row, "workspace_id")?;
        let turn = based_on.and_then(|b| b.turn_id.as_deref());
        let target:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(e)||jsonb_build_object('ever_leased',c.ever_leased,'delivery_state',c.state) FROM zuse_rust_task_command_execution e JOIN zuse_rust_mailbox_commands c USING(workspace_id,command_id) WHERE e.workspace_id=$1 AND e.session_id=$2 AND (c.lane=$4 OR (c.lane='task-control:'||e.session_id AND c.envelope->>'kind'='zuse.session.steer' AND e.message_id IS NOT NULL AND e.state IN ('starting','running','waiting-input','outcome-unknown'))) AND (e.state IN ('queued','starting','running','waiting-input') OR (e.state='outcome-unknown' AND NOT e.unknown_reviewed)) AND ($3::text IS NULL OR e.turn_id=$3) ORDER BY CASE WHEN c.ever_leased OR e.state IN ('starting','running','waiting-input','outcome-unknown') THEN 0 ELSE 1 END,e.priority DESC,c.workspace_sequence LIMIT 1 FOR UPDATE OF e,c")
            .bind(id).bind(session).bind(turn).bind(format!("task-execution:{session}")).fetch_optional(&mut **tx).await?;
        let Some(target) = target else {
            if turn.is_some() {
                return Err(error("cloud_task_interrupt_target_changed"));
            }
            return Ok(None);
        };
        let command = required(&target, "command_id")?.to_owned();
        let message = required(&target, "message_id")?.to_owned();
        let prelease = target["ever_leased"] == false
            && target["state"] == "queued"
            && matches!(
                target["delivery_state"].as_str(),
                Some("accepted" | "waiting-for-runtime" | "blocked")
            );
        if prelease {
            // The workspace lock excludes the real runtime lease transaction;
            // compare both receipts as a final proof that effects never began.
            let changed=sqlx::query("UPDATE zuse_rust_task_command_execution e SET state='cancelled',error_code='cancelled-before-dispatch',updated_at=$3 WHERE e.workspace_id=$1 AND e.command_id=$2 AND e.state='queued' AND EXISTS(SELECT 1 FROM zuse_rust_mailbox_commands c WHERE c.workspace_id=e.workspace_id AND c.command_id=e.command_id AND NOT c.ever_leased AND c.state IN ('accepted','waiting-for-runtime','blocked'))").bind(id).bind(&command).bind(now).execute(&mut **tx).await?;
            if changed.rows_affected() != 1 {
                return Err(error("cloud_task_interrupt_target_changed"));
            }
            super::super::mailbox::change(tx,id,&command,"state='cancelled',category='cancelled-before-dispatch',lease_token=NULL,lease_deadline=NULL",now).await?;
        }
        Ok(Some(BoundInterrupt {
            command_id: command,
            message_id: message,
            cancelled_before_dispatch: prelease,
        }))
    }
}
