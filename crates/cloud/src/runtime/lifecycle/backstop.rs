//! Max-quiet-awake backstop: no computer stays awake for longer than
//! [`RuntimeControl::max_quiet_awake_ms`] without activity, whatever its
//! command, lifecycle, launch, credential or runtime state says.
//!
//! The ordinary idle sleep (idle_candidates.sql) admits only computers that
//! satisfy a long list of eligibility conditions. Each condition protects
//! something, but a computer that falls outside any of them for good (a turn
//! that can never finish, a lifecycle job that gave up, a runtime that went
//! away while the row still says online) used to stay awake and billed until
//! someone noticed. The backstop runs in the same deadline-driven worker,
//! looks only at awake.sql, and sleeps such a computer through the ordinary
//! retained lifecycle. It supersedes a stuck lifecycle operation rather than
//! waiting for it, and skips guest quiescence when no runtime is alive to
//! answer. A turn whose runtime is provably alive is never interrupted, and
//! operations that own the machine identity (recreation, repair, memory
//! recovery) are reported instead of raced. Every firing logs one WARN: it
//! means a normal path failed.
use super::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Quiet time beyond the idle timeout before the backstop acts. One hour is
/// thirty times the 120s guest quiescence bound, four runtime credential
/// lifetimes, and longer than any healthy provisioning, setup, resume or
/// sleep; it is also the provider running limit `keep_running` already treats
/// as the longest a forgotten computer may run. A missed sleep therefore costs
/// at most about an hour of machine time instead of days.
pub const BACKSTOP_GRACE_MS: i64 = 3_600_000;

/// How long after its runtime was last known alive (its credential's end) a
/// computer still recording a turn sleeps. The runtime renews two minutes
/// before that end, so by then it has missed its renewal for five minutes: the
/// turn can't be running, and the computer would otherwise stay up, billed,
/// for a full quiet bound with a turn that never finishes.
pub const LOST_RUNTIME_GRACE_MS: i64 = 180_000;

/// When the backstop may act on a computer (`$2` the max quiet bound): the
/// quiet bound, or soon after a lost runtime (see awake.sql's `lost_at`).
fn due() -> String {
    format!(
        "CASE WHEN lost_at IS NULL THEN quiet_since+$2 ELSE LEAST(quiet_since+$2,lost_at+{LOST_RUNTIME_GRACE_MS}) END"
    )
}

/// Status a superseded or force-slept computer shows until it is resumed.
pub const BACKSTOP_REASON: &str = "idle-backstop";

static HELD: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/// Record holds and log only transitions: once when a computer starts being
/// held awake past the backstop bound (or its reason changes), once when it
/// stops. Returns the workspaces that newly started being held.
pub(super) fn held_transitions(
    held: &mut HashMap<String, String>,
    rows: Vec<(String, i64, String)>,
    now: i64,
) -> Vec<String> {
    let mut started = Vec::new();
    let mut current = HashMap::new();
    for (id, quiet_since, hold) in rows {
        if held.get(&id) != Some(&hold) {
            tracing::warn!(
                workspace = %id,
                hold = %hold,
                quiet_ms = now.saturating_sub(quiet_since),
                "Cloud computer held awake past the max-quiet-awake bound"
            );
            started.push(id.clone());
        }
        current.insert(id, hold);
    }
    for id in held.keys().filter(|id| !current.contains_key(*id)) {
        tracing::info!(workspace = %id, "Cloud computer no longer held awake");
    }
    *held = current;
    started
}

fn awake(columns: &str, filter: &str) -> String {
    format!(
        "SELECT {columns} FROM ({}) a WHERE {filter}",
        include_str!("awake.sql")
    )
}

/// The runtime renewed its credential within the credential lifetime and the
/// row still says it is online: it can answer a quiescence directive.
fn runtime_alive(row: &Value, now: i64) -> bool {
    row["state"] == "ready"
        && row["runtime_state"] == "online"
        && row["request_config"]["runtimeCredentialExpiresAtMs"]
            .as_i64()
            .is_some_and(|at| at > now)
}

impl RuntimeControl {
    /// No computer stays awake longer than this without activity.
    pub fn max_quiet_awake_ms(&self) -> i64 {
        self.idle_ms
            .saturating_add(crate::runtime::activity::COALESCE_MS)
            .saturating_add(BACKSTOP_GRACE_MS)
    }
    /// The earliest backstop deadline among computers it may act on.
    pub(super) async fn backstop_due(&self, now: i64) -> Result<Option<i64>, ApiError> {
        let due: Option<i64> =
            sqlx::query_scalar(&awake(&format!("min({})::bigint", due()), "hold IS NULL"))
                .bind(self.creation_accounts.filter())
                .bind(self.max_quiet_awake_ms())
                .bind(now)
                .bind(None::<String>)
                .fetch_one(&self.pool)
                .await?;
        Ok(due)
    }
    /// Admit at most one backstop sleep, then report computers the backstop
    /// may not act on (once per transition).
    pub(super) async fn backstop_sleep_once(&self, now: i64) -> Result<bool, ApiError> {
        let cutoff = now.saturating_sub(self.max_quiet_awake_ms());
        let candidate: Option<String> = sqlx::query_scalar(&awake(
            "workspace_id",
            &format!(
                "hold IS NULL AND {}<=$3 ORDER BY quiet_since,workspace_id LIMIT 1",
                due()
            ),
        ))
        .bind(self.creation_accounts.filter())
        .bind(self.max_quiet_awake_ms())
        .bind(now)
        .bind(None::<String>)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(id) = candidate
            && self.backstop_admit(&id, now).await?
        {
            return Ok(true);
        }
        let rows: Vec<(String, i64, String)> = sqlx::query_as(&awake(
            "workspace_id,quiet_since,hold",
            "hold IS NOT NULL AND quiet_since<=$2 ORDER BY workspace_id LIMIT 100",
        ))
        .bind(self.creation_accounts.filter())
        .bind(cutoff)
        .bind(now)
        .bind(None::<String>)
        .fetch_all(&self.pool)
        .await?;
        let mut held = HELD
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        held_transitions(&mut held, rows, now);
        Ok(false)
    }
    async fn backstop_admit(&self, id: &str, now: i64) -> Result<bool, ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locking::workspace(&mut tx, id, true).await?;
        // Recheck under the workspace lock: activity, a new operation or a
        // live turn that won the race keeps the computer awake.
        let due: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS({})",
            awake("1", &format!("hold IS NULL AND {}<=$3", due()))
        ))
        .bind(self.creation_accounts.filter())
        .bind(self.max_quiet_awake_ms())
        .bind(now)
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if !due
            || !row["account_id"]
                .as_str()
                .is_some_and(|owner| self.creation_accounts.contains(owner))
        {
            return Ok(false);
        }
        let config = &row["request_config"];
        let (Some(machine), Some(disk)) = (
            row["provider_sandbox_id"].as_str(),
            config["rustRuntimeStorage"]["storageIncarnationId"].as_str(),
        ) else {
            return Ok(false);
        };
        // A live runtime gets the ordinary quiescence directive (bounded by
        // the worker); with no runtime to answer, quiescence is already true.
        let alive = runtime_alive(&row, now);
        if !alive {
            // No runtime is left to run or finish a turn it recorded: the
            // turn is lost. Its message waits for the user's review (never
            // re-run on its own) instead of reading as running forever.
            settle_lost_turns_tx(&mut tx, &row, now).await?;
        }
        let phase = if alive { "quiescing" } else { "quiesced" };
        let superseded = config["rustLifecycleOperation"].clone();
        let operation = uuid::Uuid::new_v4().to_string();
        let mut config = config.clone();
        progress(&mut config, &operation, Action::Pause, phase);
        config["rustIdleBackstop"] = json!({
            "operationId": operation,
            "at": now,
            "state": row["state"],
            "statusCode": row["status_code"],
            "runtimeState": row["runtime_state"],
            "superseded": superseded,
        });
        // Retire every job of the superseded operation; its worker is fenced
        // by the new current operation either way.
        sqlx::query("UPDATE zuse_rust_jobs SET state='failed',last_error_code='superseded-by-idle-backstop',lease_token=NULL,lease_expires_at=NULL,updated_at=$3 WHERE kind=$2 AND resource_id=$1 AND state IN ('pending','running')")
            .bind(id).bind(JOB_KIND).bind(now).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,'pause',$7,$8,$8)")
            .bind(&operation).bind(id).bind(format!("backstop:{generation}:{now}")).bind(generation).bind(machine)
            .bind(disk).bind(phase).bind(now).execute(&mut *tx).await?;
        sqlx::query("UPDATE api_cloud_workspaces SET state='pausing',desired_state='paused',status_code=$4,runtime_state=CASE WHEN $5 THEN runtime_state ELSE 'offline' END,request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3),next_action_at=$3 WHERE workspace_id=$1")
            .bind(id).bind(&config).bind(now).bind(if alive {"pause-quiescing"} else {"runtime-quiesced"}).bind(alive)
            .execute(&mut *tx).await?;
        let job = jobs::enqueue(
            &mut tx,
            &format!("lifecycle:{operation}"),
            JOB_KIND,
            id,
            generation,
            &json!({"operationId":operation}),
            now,
        )
        .await?;
        sqlx::query("UPDATE zuse_rust_jobs SET max_attempts=120 WHERE job_id=$1")
            .bind(job)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::warn!(
            workspace = %id,
            state = row["state"].as_str().unwrap_or(""),
            status_code = row["status_code"].as_str().unwrap_or(""),
            runtime_state = row["runtime_state"].as_str().unwrap_or(""),
            superseded_phase = superseded["phase"].as_str().unwrap_or(""),
            superseded_action = superseded["action"].as_str().unwrap_or(""),
            runtime_alive = alive,
            "Cloud computer slept by the max-quiet-awake backstop; a normal sleep path did not run"
        );
        wakeup::send();
        Ok(true)
    }
}

/// Mark every turn still recorded as starting, running or waiting for input
/// on this computer as lost (`outcome-unknown`, `runtime-process-lost`), as
/// the next runtime would on its cold start, and tell open apps so each shows
/// the message for review.
async fn settle_lost_turns_tx(
    tx: &mut Transaction<'_, Postgres>,
    row: &Value,
    now: i64,
) -> Result<(), ApiError> {
    let id = row["workspace_id"].as_str().unwrap_or_default();
    let mut sessions: Vec<String> = sqlx::query_scalar("UPDATE zuse_rust_task_command_execution SET state='outcome-unknown',error_code='runtime-process-lost',unknown_reviewed=false,updated_at=$2 WHERE workspace_id=$1 AND state IN('starting','running','waiting-input') RETURNING session_id")
        .bind(id).bind(now).fetch_all(&mut **tx).await?;
    sessions.sort();
    sessions.dedup();
    if let Some(owner) = row["account_id"].as_str() {
        for session in &sessions {
            sqlx::query("INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,workspace_id,payload,created_at) VALUES($1,'task-command-progress',$2,$3,'{\"commandAdmissionVersion\":2}'::jsonb,$4)")
                .bind(owner).bind(session).bind(id).bind(now).execute(&mut **tx).await?;
        }
    }
    if !sessions.is_empty() {
        tracing::warn!(workspace = %id, conversations = sessions.len(), "Cloud turn lost with its runtime; waiting for review");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
