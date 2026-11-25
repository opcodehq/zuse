//! Settles lifecycle operations left unfinished on a computer that is already
//! paused, with nothing left to finish them.
//!
//! Archive, Resume and a message's wake all require the current lifecycle
//! operation to have completed (`cloud_lifecycle_quiescence_required`,
//! `cloud_lifecycle_in_progress`). A path that marked the computer paused
//! without completing its operation (an older server that gave up on a resume
//! after its attempts ran out, for example) left that operation `waking` or
//! `launching` forever, and the user could neither archive nor use the
//! computer. Once such an operation has had no progress for its deadline and
//! no job of any kind is active on the computer, this sweep settles it the
//! way a failed resume is settled (`settle_failed_resume`): a pause takes
//! over the computer, sleeps its machine through the ordinary retained
//! lifecycle and keeps a failure code visible. Operations that own the
//! machine identity (recreation, repair, memory recovery) are left to them,
//! a computer whose provider no longer has its machine is left for Recreate,
//! and one with no machine or retained disk has nothing to sleep.
use super::*;

/// How often the worker looks for unfinished operations on paused computers.
pub const SWEEP_MS: i64 = 60_000;
/// No progress for this long, with no active job, means the operation will
/// not finish: the same bound a failing resume has.
pub const DEADLINE_MS: i64 = RESUME_FAILURE_DEADLINE_MS;
/// Status when the operation's last job left no resume failure code.
pub const CODE: &str = "cloud_lifecycle_timed_out";
/// Computers settled per sweep.
const BATCH: i64 = 20;

/// Paused computers whose current operation is unfinished, idle since `$1`,
/// with no active job, at most $2.
const CANDIDATES: &str = "SELECT w.workspace_id FROM api_cloud_workspaces w JOIN zuse_rust_control_owners c ON c.resource_kind='workspace' AND c.resource_id=w.workspace_id AND c.backend='rust' AND c.generation=COALESCE((w.request_config->>'runtimeGeneration')::bigint,1) JOIN zuse_rust_lifecycle_operations o ON o.workspace_id=w.workspace_id AND o.operation_id=w.request_config#>>'{rustLifecycleOperation,id}' WHERE w.state='paused' AND w.desired_state='paused' AND w.status_code IS DISTINCT FROM 'cloud_retained_machine_missing' AND w.provider_sandbox_id IS NOT NULL AND w.request_config#>>'{rustRuntimeStorage,storageIncarnationId}' IS NOT NULL AND o.phase NOT IN ('paused','ready') AND o.updated_at<=$1 AND NOT EXISTS(SELECT 1 FROM zuse_rust_jobs j WHERE j.resource_id=w.workspace_id AND j.state IN ('pending','running')) ORDER BY o.updated_at,w.workspace_id LIMIT $2";

/// Whether the locked computer and its current operation still need settling.
fn unfinished(row: &Value, op: &Value, cutoff: i64) -> bool {
    let config = &row["request_config"];
    row["state"] == "paused"
        && row["desired_state"] == "paused"
        && row["status_code"] != MACHINE_MISSING
        && settle_sleep(row).is_some()
        && !deleted(row)
        && !super::super::recreation::leases_blocked(row)
        && !super::super::repair::leases_blocked(row)
        && config["rustMemoryTransportRecovery"]["operationId"] != op["operation_id"]
        && op["operation_id"] == config["rustLifecycleOperation"]["id"]
        && !matches!(op["phase"].as_str(), Some("paused" | "ready"))
        && op["updated_at"].as_i64().is_some_and(|at| at <= cutoff)
}

impl RuntimeControl {
    /// Settle every unfinished operation on a paused computer past its
    /// deadline (at most one batch). Returns how many computers were settled.
    pub async fn settle_stale_lifecycle(&self, now: i64) -> Result<usize, ApiError> {
        let cutoff = now.saturating_sub(DEADLINE_MS);
        let ids: Vec<String> = sqlx::query_scalar(CANDIDATES)
            .bind(cutoff)
            .bind(BATCH)
            .fetch_all(&self.pool)
            .await?;
        let mut settled = 0;
        for id in ids {
            if self.settle_stale_one(&id, cutoff, now).await? {
                settled += 1;
            }
        }
        if settled > 0 {
            wakeup::send();
        }
        Ok(settled)
    }
    async fn settle_stale_one(&self, id: &str, cutoff: i64, now: i64) -> Result<bool, ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locking::workspace(&mut tx, id, true).await?;
        let op: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(o) FROM zuse_rust_lifecycle_operations o WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 FOR UPDATE")
            .bind(row["request_config"]["rustLifecycleOperation"]["id"].as_str()).bind(id).bind(generation)
            .fetch_optional(&mut *tx).await?;
        let Some(op) = op else {
            return Ok(false);
        };
        // Recheck under the lock: a job admitted since the scan owns it.
        let active: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE resource_id=$1 AND state IN ('pending','running'))")
            .bind(id).fetch_one(&mut *tx).await?;
        if active || !unfinished(&row, &op, cutoff) {
            return Ok(false);
        }
        let operation = op["operation_id"].as_str().unwrap_or_default();
        let last: Option<String> = sqlx::query_scalar("SELECT last_error_code FROM zuse_rust_jobs WHERE kind=$1 AND resource_id=$2 AND payload->>'operationId'=$3 ORDER BY updated_at DESC LIMIT 1")
            .bind(JOB_KIND).bind(id).bind(operation).fetch_optional(&mut *tx).await?.flatten();
        let code = last
            .filter(|code| resume_failure_terminal(code))
            .unwrap_or_else(|| CODE.to_owned());
        let resume = op["action"] == "resume";
        let sleeping = settle_to_sleep(
            &mut tx,
            &row,
            generation,
            resume.then_some(operation),
            &format!("stale-settle:{operation}"),
            &code,
            now,
        )
        .await?;
        tx.commit().await?;
        tracing::warn!(
            workspace = %id,
            operation,
            action = op["action"].as_str().unwrap_or(""),
            phase = op["phase"].as_str().unwrap_or(""),
            code,
            sleeping,
            "Unfinished lifecycle operation on a paused computer settled; a normal path left it behind"
        );
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
