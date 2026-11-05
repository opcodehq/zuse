//! Compact committed events. Domain writes only append an outbox row; the short
//! dispatcher transaction assigns replay cursors after those writes have committed.
//! There is no database notification channel and no provider/object-store I/O.
use crate::error::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};

pub const MIGRATION: &str = include_str!("../../migrations/0017_account_events.sql");
pub const BATCH: i64 = 128;
pub const PAGE: i64 = 128;
#[derive(Clone)]
pub struct EventStore {
    pool: PgPool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub sequence: i64,
    pub kind: String,
    pub resource_id: String,
    pub workspace_id: Option<String>,
    pub payload: Value,
    pub created_at: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Replay {
    pub events: Vec<Event>,
    pub latest: i64,
    pub reset_required: bool,
}
impl EventStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn ready(&self) -> Result<(), ApiError> {
        sqlx::query("SELECT owner_id,sequence FROM zuse_rust_account_events LIMIT 0")
            .execute(&self.pool)
            .await?;
        sqlx::query("SELECT owner_id,outbox_id FROM zuse_rust_account_event_outbox LIMIT 0")
            .execute(&self.pool)
            .await?;
        Ok(())
    }
    pub async fn latest(&self, owner: &str) -> Result<i64, ApiError> {
        Ok(sqlx::query_scalar("SELECT COALESCE((SELECT next_sequence-1 FROM zuse_rust_account_event_cursors WHERE owner_id=$1),0)")
            .bind(owner).fetch_one(&self.pool).await?)
    }
    /// One database statement atomically claims committed outbox rows, assigns
    /// consecutive owner cursors, appends events and deletes the claimed rows.
    /// Ordered owner locks avoid cross-owner dispatcher deadlocks. An aborted
    /// writer cannot consume an event cursor or cause a later commit to be skipped.
    pub async fn dispatch(&self) -> Result<Vec<(String, i64)>, ApiError> {
        let rows = sqlx::query_as::<_,(String,i64)>(r#"
WITH picked AS MATERIALIZED (
 SELECT outbox_id,owner_id,kind,resource_id,workspace_id,payload,created_at
 FROM zuse_rust_account_event_outbox ORDER BY outbox_id
 FOR UPDATE SKIP LOCKED LIMIT $1
), grouped AS MATERIALIZED (
 SELECT owner_id,count(*)::bigint AS n FROM picked GROUP BY owner_id
), allocated AS (
 INSERT INTO zuse_rust_account_event_cursors(owner_id,next_sequence)
 SELECT owner_id,n+1 FROM grouped ORDER BY owner_id
 ON CONFLICT(owner_id) DO UPDATE SET next_sequence=zuse_rust_account_event_cursors.next_sequence+EXCLUDED.next_sequence-1
 RETURNING owner_id,next_sequence
), inserted AS (
 INSERT INTO zuse_rust_account_events(owner_id,sequence,kind,resource_id,workspace_id,payload,created_at)
 SELECT p.owner_id,a.next_sequence-g.n-1+row_number() OVER(PARTITION BY p.owner_id ORDER BY p.outbox_id),p.kind,p.resource_id,p.workspace_id,p.payload,p.created_at
 FROM picked p JOIN grouped g USING(owner_id) JOIN allocated a USING(owner_id)
 RETURNING owner_id,sequence
), cleared AS (
 DELETE FROM zuse_rust_account_event_outbox WHERE outbox_id IN(SELECT outbox_id FROM picked)
 AND EXISTS(SELECT 1 FROM inserted WHERE inserted.owner_id=zuse_rust_account_event_outbox.owner_id)
 RETURNING outbox_id
)
SELECT owner_id,max(sequence)::bigint FROM inserted GROUP BY owner_id
"#).bind(BATCH).fetch_all(&self.pool).await?;
        Ok(rows)
    }
    /// Internal owner scope only. The transport must recheck current identity and
    /// sharing permissions before emitting a workspace-bound item to a device.
    pub async fn replay(&self, owner: &str, after: i64) -> Result<Replay, ApiError> {
        self.replay_limit(owner, after, PAGE).await
    }
    pub async fn replay_limit(
        &self,
        owner: &str,
        after: i64,
        limit: i64,
    ) -> Result<Replay, ApiError> {
        if owner.is_empty() || after < 0 || !(1..=PAGE).contains(&limit) {
            return Err(ApiError::bad_request("invalid_account_event_cursor"));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        let latest:i64=sqlx::query_scalar("SELECT COALESCE((SELECT next_sequence-1 FROM zuse_rust_account_event_cursors WHERE owner_id=$1),0)").bind(owner).fetch_one(&mut *tx).await?;
        if after > latest {
            return Err(ApiError::conflict("account_event_cursor_ahead"));
        }
        let first: Option<i64> = sqlx::query_scalar(
            "SELECT min(sequence) FROM zuse_rust_account_events WHERE owner_id=$1",
        )
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
        let reset_required = after < latest && first.is_none_or(|first| after < first - 1);
        let rows = if reset_required {
            vec![]
        } else {
            sqlx::query("SELECT sequence,kind,resource_id,workspace_id,payload,created_at FROM zuse_rust_account_events WHERE owner_id=$1 AND sequence>$2 ORDER BY sequence LIMIT $3").bind(owner).bind(after).bind(limit).fetch_all(&mut *tx).await?
        };
        let events: Vec<Event> = rows
            .into_iter()
            .map(|row| {
                Ok(Event {
                    sequence: row.try_get("sequence")?,
                    kind: row.try_get("kind")?,
                    resource_id: row.try_get("resource_id")?,
                    workspace_id: row.try_get("workspace_id")?,
                    payload: row.try_get("payload")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect::<Result<_, sqlx::Error>>()?;
        let contiguous = events
            .iter()
            .enumerate()
            .all(|(index, event)| event.sequence == after + index as i64 + 1);
        let reset_required = reset_required || !contiguous || (after < latest && events.is_empty());
        let events = if reset_required { vec![] } else { events };
        tx.commit().await?;
        Ok(Replay {
            events,
            latest,
            reset_required,
        })
    }
    /// Retention is independent of computer lifecycle. Falling behind the retained
    /// window produces an explicit resync, never a fabricated transcript cursor.
    pub async fn prune(&self, older_than: i64) -> Result<u64, ApiError> {
        let result=sqlx::query("DELETE FROM zuse_rust_account_events WHERE (owner_id,sequence) IN (SELECT owner_id,sequence FROM zuse_rust_account_events WHERE created_at<$1 ORDER BY created_at LIMIT 2048)").bind(older_than).execute(&self.pool).await?;
        Ok(result.rows_affected())
    }
}
/// Called in the canonical history publication transaction. Token-only streaming
/// updates do not append account events. First/new messages and status transitions
/// do; detailed text remains encrypted ordinary history and the opened task stream.
pub async fn projection_summary(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workspace: &str,
    session: &str,
    message_count: i64,
    last_id: Option<&str>,
    last_status: Option<&str>,
    now: i64,
) -> Result<(), ApiError> {
    let summary = json!({"messageCount":message_count,"lastMessageId":last_id,"lastMessageStatus":last_status});
    let signature = crate::transcript::digest(
        &serde_json::to_vec(&summary)
            .map_err(|_| ApiError::unavailable("account_event_summary_invalid"))?,
    );
    // The outbox depends on the changed summary, preserving deduplication while
    // avoiding another network round trip under the workspace publication lock.
    sqlx::query("WITH changed AS (INSERT INTO zuse_rust_account_event_summaries(workspace_id,session_id,signature) VALUES($1,$2,$3) ON CONFLICT(workspace_id,session_id) DO UPDATE SET signature=EXCLUDED.signature WHERE zuse_rust_account_event_summaries.signature<>EXCLUDED.signature RETURNING workspace_id) INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,workspace_id,payload,created_at) SELECT w.account_id,'conversation',$2,w.workspace_id,$4,$5 FROM api_cloud_workspaces w JOIN changed c ON c.workspace_id=w.workspace_id")
        .bind(workspace).bind(session).bind(signature).bind(summary).bind(now).execute(&mut **tx).await?;
    Ok(())
}
