//! Server-held attachments live in object storage only until the computer has
//! them. Lifecycle of one asset:
//!
//!   uploading -> ready -> (bound by commands / queued messages)
//!     -> delivered: a binding command is reported running, waiting for input
//!        or completed (the computer fetches every bound file before its turn
//!        starts), and no other command or queued message still needs it
//!     -> releasing -> released (objects deleted; the row keeps metadata only)
//!
//!   uploading | ready -> expired after `UNDELIVERED_ASSET_TTL_MS` -> releasing
//!
//! The decision is made inside the SQL transaction that records the fact; the
//! object deletes run after commit, never under a lock. `releasing` is durable,
//! so a failed delete is retried by the sweep.
use super::*;

/// An attachment that no computer received is deleted this long after upload.
pub const UNDELIVERED_ASSET_TTL_MS: i64 = 7 * 86_400_000;
/// Encrypted chunk bytes stay in PostgreSQL only until their object write is
/// confirmed. An unconfirmed chunk older than this is an abandoned upload.
pub const STALE_CHUNK_CIPHERTEXT_MS: i64 = 86_400_000;
const SWEEP_BATCH: i64 = 16;
/// A command that may still fetch its files: admitted but not yet running.
const LIVE_HOLD: &str = "h.delivered_at IS NULL AND ((h.kind='queue' AND EXISTS(SELECT 1 FROM zuse_rust_task_command_queue q WHERE q.workspace_id=h.workspace_id AND q.session_id=h.session_id AND q.queue_id=h.holder AND q.state='queued')) OR (h.kind='command' AND EXISTS(SELECT 1 FROM zuse_rust_task_command_execution e WHERE e.workspace_id=h.workspace_id AND e.command_id=h.holder AND e.state IN ('queued','starting'))))";

#[derive(Clone, Copy)]
pub(in crate::runtime) enum Holder<'a> {
    /// An admitted command; its id is unique within the workspace.
    Command { session: &'a str, command: &'a str },
    /// A queued message, live while its queue row is still `queued`.
    Queue { session: &'a str, queue: &'a str },
}

impl RuntimeControl {
    async fn asset_release_ready<'e>(
        &self,
        db: impl sqlx::PgExecutor<'e>,
    ) -> Result<bool, ApiError> {
        use std::sync::atomic::Ordering;
        if self.asset_release_schema.load(Ordering::Acquire) {
            return Ok(true);
        }
        let ready: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_launch_asset_holds') IS NOT NULL")
                .fetch_one(db)
                .await?;
        if ready {
            self.asset_release_schema.store(true, Ordering::Release);
        }
        Ok(ready)
    }

    /// Record who needs these verified assets. A queued message's set is
    /// replaced on edit; a retried command keeps its single binding.
    pub(in crate::runtime) async fn hold_assets_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: &str,
        holder: Holder<'_>,
        assets: &[Asset],
        now: i64,
    ) -> Result<(), ApiError> {
        // Nothing to record, and (for a command) nothing earlier to replace.
        if assets.is_empty() && matches!(holder, Holder::Command { .. }) {
            return Ok(());
        }
        if !self.asset_release_ready(&mut **tx).await? {
            return Ok(());
        }
        let (kind, session, id) = match holder {
            Holder::Command { session, command } => ("command", session, command),
            Holder::Queue { session, queue } => {
                sqlx::query("DELETE FROM zuse_rust_launch_asset_holds WHERE workspace_id=$1 AND kind='queue' AND session_id=$2 AND holder=$3").bind(workspace).bind(session).bind(queue).execute(&mut **tx).await?;
                ("queue", session, queue)
            }
        };
        if assets.is_empty() {
            return Ok(());
        }
        let ids: Vec<&str> = assets.iter().map(|a| a.asset_id.as_str()).collect();
        sqlx::query("INSERT INTO zuse_rust_launch_asset_holds(workspace_id,asset_id,kind,session_id,holder,created_at) SELECT $1,a,$2,$3,$4,$5 FROM unnest($6::text[]) a ON CONFLICT DO NOTHING").bind(workspace).bind(kind).bind(session).bind(id).bind(now).bind(&ids).execute(&mut **tx).await?;
        Ok(())
    }

    /// Called with a command's accepted execution report. `delivered` means
    /// the computer has its files. Returns assets to delete after commit.
    pub(in crate::runtime) async fn settle_command_assets_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: &str,
        command: &str,
        delivered: bool,
        now: i64,
    ) -> Result<Vec<String>, ApiError> {
        if !self.asset_release_ready(&mut **tx).await? {
            return Ok(vec![]);
        }
        let assets: Vec<String> = if delivered {
            sqlx::query_scalar("UPDATE zuse_rust_launch_asset_holds SET delivered_at=COALESCE(delivered_at,$3) WHERE workspace_id=$1 AND kind='command' AND holder=$2 RETURNING asset_id").bind(workspace).bind(command).bind(now).fetch_all(&mut **tx).await?
        } else {
            sqlx::query_scalar("SELECT asset_id FROM zuse_rust_launch_asset_holds WHERE workspace_id=$1 AND kind='command' AND holder=$2").bind(workspace).bind(command).fetch_all(&mut **tx).await?
        };
        self.release_delivered_tx(tx, workspace, &assets, now).await
    }

    /// Mark delivered assets that no live command or queued message needs.
    async fn release_delivered_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        workspace: &str,
        assets: &[String],
        now: i64,
    ) -> Result<Vec<String>, ApiError> {
        if assets.is_empty() {
            return Ok(vec![]);
        }
        // Lock first; the check below then runs on a fresh snapshot, after any
        // admission that held the row (verification takes FOR SHARE) committed.
        let locked: Vec<String> = sqlx::query_scalar("SELECT asset_id FROM zuse_rust_launch_assets WHERE workspace_id=$1 AND asset_id=ANY($2) AND state='ready' ORDER BY asset_id FOR UPDATE").bind(workspace).bind(assets).fetch_all(&mut **tx).await?;
        if locked.is_empty() {
            return Ok(vec![]);
        }
        Ok(sqlx::query_scalar(&format!("UPDATE zuse_rust_launch_assets a SET state='releasing',release_reason='delivered',updated_at=$3 WHERE a.workspace_id=$1 AND a.asset_id=ANY($2) AND a.state='ready' AND EXISTS(SELECT 1 FROM zuse_rust_launch_asset_holds h WHERE h.workspace_id=a.workspace_id AND h.asset_id=a.asset_id AND h.delivered_at IS NOT NULL) AND NOT EXISTS(SELECT 1 FROM zuse_rust_launch_asset_holds h WHERE h.workspace_id=a.workspace_id AND h.asset_id=a.asset_id AND {LIVE_HOLD}) RETURNING a.asset_id")).bind(workspace).bind(&locked).bind(now).fetch_all(&mut **tx).await?)
    }

    /// Delete released assets' objects, then record them released. Runs with
    /// no SQL transaction open; a failure leaves them `releasing` for the sweep.
    pub(in crate::runtime) async fn release_assets(
        &self,
        workspace: &str,
        assets: &[String],
        now: i64,
    ) -> Result<(), ApiError> {
        let objects = self.transcript_objects.as_ref().ok_or_else(failed)?;
        for asset in assets {
            let base = prefix(workspace, asset);
            let mut purged = false;
            for _ in 0..4 {
                if objects.purge_prefix(&format!("{base}/")).await? {
                    purged = true;
                    break;
                }
            }
            if !purged {
                return Err(ApiError::unavailable("cloud_asset_release_incomplete"));
            }
            objects.delete_object(&format!("{base}.json")).await?;
            let mut tx = self.pool.begin().await?;
            let released = sqlx::query("UPDATE zuse_rust_launch_assets SET state='released',released_at=$3,updated_at=$3 WHERE workspace_id=$1 AND asset_id=$2 AND state='releasing'").bind(workspace).bind(asset).bind(now).execute(&mut *tx).await?;
            if released.rows_affected() == 1 {
                // Chunk receipts only served the upload; their bytes are gone.
                sqlx::query(
                    "DELETE FROM zuse_rust_task_asset_chunks WHERE workspace_id=$1 AND asset_id=$2",
                )
                .bind(workspace)
                .bind(asset)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
        }
        Ok(())
    }

    /// One bounded pass: expire undelivered assets, release delivered assets
    /// whose last other holder went away, retry pending deletes, and drop
    /// abandoned chunk ciphertext. True when any work was found.
    pub async fn sweep_asset_releases_once(&self, now: i64) -> Result<bool, ApiError> {
        let mut worked = false;
        let chunks: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_task_asset_chunks') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if chunks {
            worked |= sqlx::query("DELETE FROM zuse_rust_task_asset_chunks WHERE ctid IN (SELECT ctid FROM zuse_rust_task_asset_chunks WHERE NOT ready AND updated_at<=$1 LIMIT 100)").bind(now - STALE_CHUNK_CIPHERTEXT_MS).execute(&self.pool).await?.rows_affected() > 0;
        }
        if !self.asset_release_ready(&self.pool).await? {
            return Ok(worked);
        }
        // Never-created launch workspaces belong to the staging sweep, which
        // serializes with creation under the account lock.
        let mut tx = self.pool.begin().await?;
        let due: Vec<(String, String)> = sqlx::query_as("SELECT a.workspace_id,a.asset_id FROM zuse_rust_launch_assets a WHERE a.state IN ('uploading','ready') AND a.created_at<=$1 AND EXISTS(SELECT 1 FROM api_cloud_workspaces w WHERE w.workspace_id=a.workspace_id) ORDER BY a.created_at LIMIT $2 FOR UPDATE OF a SKIP LOCKED").bind(now - UNDELIVERED_ASSET_TTL_MS).bind(SWEEP_BATCH).fetch_all(&mut *tx).await?;
        if !due.is_empty() {
            let (workspaces, ids): (Vec<String>, Vec<String>) = due.into_iter().unzip();
            sqlx::query("UPDATE zuse_rust_launch_assets a SET state='releasing',release_reason='expired',updated_at=$3 FROM unnest($1::text[],$2::text[]) d(workspace_id,asset_id) WHERE a.workspace_id=d.workspace_id AND a.asset_id=d.asset_id AND a.state IN ('uploading','ready')").bind(&workspaces).bind(&ids).bind(now).execute(&mut *tx).await?;
            worked = true;
        }
        tx.commit().await?;
        // A delivered asset may have waited on another command that has since
        // ended without fetching it, or on a queued message since removed.
        let candidates: Vec<(String, String)> = sqlx::query_as(&format!("SELECT a.workspace_id,a.asset_id FROM zuse_rust_launch_assets a WHERE a.state='ready' AND EXISTS(SELECT 1 FROM zuse_rust_launch_asset_holds h WHERE h.workspace_id=a.workspace_id AND h.asset_id=a.asset_id AND h.delivered_at IS NOT NULL) AND NOT EXISTS(SELECT 1 FROM zuse_rust_launch_asset_holds h WHERE h.workspace_id=a.workspace_id AND h.asset_id=a.asset_id AND {LIVE_HOLD}) ORDER BY a.created_at LIMIT $1")).bind(SWEEP_BATCH).fetch_all(&self.pool).await?;
        for (workspace, asset) in candidates {
            let mut tx = self.pool.begin().await?;
            worked |= !self
                .release_delivered_tx(&mut tx, &workspace, std::slice::from_ref(&asset), now)
                .await?
                .is_empty();
            tx.commit().await?;
        }
        let pending: Vec<(String, String)> = sqlx::query_as("SELECT workspace_id,asset_id FROM zuse_rust_launch_assets WHERE state='releasing' ORDER BY updated_at LIMIT $1").bind(SWEEP_BATCH).fetch_all(&self.pool).await?;
        for (workspace, asset) in pending {
            worked = true;
            if let Err(error) = self
                .release_assets(&workspace, std::slice::from_ref(&asset), now)
                .await
            {
                tracing::warn!(code = error.code, "Attachment object release deferred");
                // Rotate a failing delete behind the rest of the batch.
                sqlx::query("UPDATE zuse_rust_launch_assets SET updated_at=$3 WHERE workspace_id=$1 AND asset_id=$2 AND state='releasing'").bind(&workspace).bind(&asset).bind(now).execute(&self.pool).await?;
            }
        }
        Ok(worked)
    }
}
