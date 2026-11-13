//! When to build and when to delete. Recipe saves and "Rebuild now" queue a
//! build directly. Every hour a staleness pass compares, for each recently
//! active account, the inputs of its current ready image (setup script
//! digest, managed base release, provider) with the current ones and queues
//! one rebuild when they differ; identical inputs never rebuild, so a quiet
//! account builds nothing. Inactive accounts are rebuilt lazily when they
//! start their next computer. A failed build keeps the previous image
//! serving and is retried on later passes with exponential backoff, at most
//! `MAX_FAILED_ATTEMPTS` times for the same inputs. Images no computer
//! references and no admission can select are retired and their provider
//! snapshots deleted.
use super::worker::{UserImageControl, enqueue_tx};
use super::*;
use serde_json::json;
use sqlx::{Postgres, Transaction};

/// Accounts with a computer active this recently rebuild in the background.
pub const ACTIVE_WINDOW_MS: i64 = 14 * 24 * 60 * 60 * 1000;
/// The staleness pass runs hourly.
pub const SCHEDULE_INTERVAL_MS: i64 = 60 * 60 * 1000;
/// Automatic retries of one failing script on one base; then only a script
/// edit or "Rebuild now" builds again.
pub const MAX_FAILED_ATTEMPTS: i64 = 3;
/// Accounts examined per pass.
const PASS_LIMIT: i64 = 200;

const NEWEST_BASE: &str = "SELECT release_id FROM cloud_managed_bases WHERE provider=$1 AND state='verified' AND runtime_manifest ? 'initialRuntimeBootstrap' ORDER BY created_at DESC,release_id DESC LIMIT 1";

/// Why the account's image should be rebuilt now, if at all: its newest
/// ready image was built from different inputs than the current script and
/// newest base, nothing is already waiting or running, and failures for
/// these exact inputs are under the cap and past their backoff.
pub(crate) async fn stale_reason_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    provider: &str,
    digest: &str,
    now: i64,
) -> Result<Option<&'static str>, ApiError> {
    let Some(newest): Option<String> = sqlx::query_scalar(NEWEST_BASE)
        .bind(provider)
        .fetch_optional(&mut **tx)
        .await?
    else {
        return Ok(None);
    };
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM cloud_machine_images WHERE owner_id=$1 AND provider=$2 AND state IN('queued','building','cleaning'))")
        .bind(owner)
        .bind(provider)
        .fetch_one(&mut **tx)
        .await?;
    if busy {
        return Ok(None);
    }
    let ready: Option<(Option<String>, String)> = sqlx::query_as("SELECT base_release_id,recipe_digest FROM cloud_machine_images WHERE owner_id=$1 AND provider=$2 AND state='ready' ORDER BY built_at DESC,image_id DESC LIMIT 1")
        .bind(owner)
        .bind(provider)
        .fetch_optional(&mut **tx)
        .await?;
    let reason = match &ready {
        Some((base, built)) if base.as_deref() == Some(newest.as_str()) && built == digest => {
            return Ok(None);
        }
        Some((_, built)) if built == digest => "base",
        _ => "recipe",
    };
    let (failures, last): (i64, Option<i64>) = sqlx::query_as("SELECT count(*),max(updated_at) FROM cloud_machine_images WHERE owner_id=$1 AND provider=$2 AND base_release_id=$3 AND recipe_digest=$4 AND state='failed' AND created_at>coalesce((SELECT max(created_at) FROM cloud_machine_images WHERE owner_id=$1 AND provider=$2 AND reason='manual' AND state<>'superseded'),0)")
        .bind(owner)
        .bind(provider)
        .bind(&newest)
        .bind(digest)
        .fetch_one(&mut **tx)
        .await?;
    if failures >= MAX_FAILED_ATTEMPTS
        || last
            .is_some_and(|at| now < at + SCHEDULE_INTERVAL_MS * (1 << (failures - 1).clamp(0, 8)))
    {
        return Ok(None);
    }
    Ok(Some(reason))
}

/// Lazy rebuild for an account starting a computer: same staleness rule as
/// the hourly pass, which skips inactive accounts.
pub(crate) async fn rebuild_on_use_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    provider: &str,
    digest: &str,
    now: i64,
) -> Result<bool, ApiError> {
    if stale_reason_tx(tx, owner, provider, digest, now)
        .await?
        .is_none()
    {
        return Ok(false);
    }
    enqueue_tx(tx, owner, "use", now, now).await
}

impl UserImageControl {
    pub(crate) async fn schedule_one(&self, now: i64) -> Result<bool, ApiError> {
        self.staleness_pass(now).await.map(|queued| queued > 0)
    }
    /// One hourly pass over recently active accounts with a setup script.
    /// Returns how many rebuilds it queued.
    pub(crate) async fn staleness_pass(&self, now: i64) -> Result<usize, ApiError> {
        self.mark_scheduled(now);
        let owners: Vec<String> = sqlx::query_scalar("SELECT v.owner_id FROM (SELECT DISTINCT ON (owner_id) owner_id,recipe FROM cloud_machine_versions ORDER BY owner_id,revision DESC) v WHERE v.recipe->>'setupScript' ~ '\\S' AND EXISTS(SELECT 1 FROM api_cloud_workspaces w WHERE w.account_id=v.owner_id AND w.last_activity_at>$1) ORDER BY v.owner_id LIMIT $2")
            .bind(now - ACTIVE_WINDOW_MS)
            .bind(PASS_LIMIT)
            .fetch_all(self.pool())
            .await?;
        let mut queued = 0;
        for owner in owners {
            let mut tx = self.pool().begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(format!("machine:{owner}"))
                .execute(&mut *tx)
                .await?;
            let Some(digest) = super::selection::current_digest_tx(&mut tx, &owner).await? else {
                tx.commit().await?;
                continue;
            };
            for provider in PROVIDERS {
                if let Some(reason) =
                    stale_reason_tx(&mut tx, &owner, provider.as_str(), &digest, now).await?
                {
                    enqueue_tx(&mut tx, &owner, reason, now, now).await?;
                    queued += 1;
                    break;
                }
            }
            tx.commit().await?;
        }
        Ok(queued)
    }

    /// Start deleting one image nobody can use: a newer ready image on a
    /// verified base replaced it, its base is no longer verified, or the
    /// account's recipe no longer has a setup script. Never while any
    /// non-deleted computer (including an archived one) references it.
    pub(crate) async fn retire_one(&self, now: i64) -> Result<bool, ApiError> {
        let candidates: Vec<String> = sqlx::query_scalar("SELECT a.image_id FROM cloud_machine_images a WHERE a.state='ready' AND (EXISTS(SELECT 1 FROM cloud_machine_images n JOIN cloud_managed_bases b ON b.provider=n.provider AND b.release_id=n.base_release_id AND b.state='verified' WHERE n.owner_id=a.owner_id AND n.provider=a.provider AND n.state='ready' AND n.built_at>a.built_at) OR NOT EXISTS(SELECT 1 FROM cloud_managed_bases b WHERE b.provider=a.provider AND b.release_id=a.base_release_id AND b.state='verified' AND b.artifact_id=a.base_artifact_id) OR NOT coalesce((SELECT recipe->>'setupScript' FROM cloud_machine_versions v WHERE v.owner_id=a.owner_id ORDER BY revision DESC LIMIT 1) ~ '\\S',false)) AND NOT EXISTS(SELECT 1 FROM api_cloud_workspaces w WHERE w.state<>'deleted' AND w.request_config->'rustMachineImage'->>'imageId'=a.image_id) AND NOT EXISTS(SELECT 1 FROM cloud_machine_images o WHERE o.owner_id=a.owner_id AND o.state IN('building','cleaning','retiring')) ORDER BY a.built_at LIMIT 8")
            .fetch_all(self.pool())
            .await?;
        for id in candidates {
            let mut tx = self.pool().begin().await?;
            // Admission holds this row FOR SHARE until its computer row
            // commits; once we hold it FOR UPDATE, the reference check below
            // sees every computer admitted onto it.
            let locked: Option<String> = sqlx::query_scalar(
                "SELECT state FROM cloud_machine_images WHERE image_id=$1 FOR UPDATE",
            )
            .bind(&id)
            .fetch_optional(&mut *tx)
            .await?;
            if locked.as_deref() != Some("ready") {
                tx.rollback().await?;
                continue;
            }
            let referenced: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE state<>'deleted' AND request_config->'rustMachineImage'->>'imageId'=$1)")
                .bind(&id)
                .fetch_one(&mut *tx)
                .await?;
            if referenced {
                tx.rollback().await?;
                continue;
            }
            let started = sqlx::query("UPDATE cloud_machine_images SET state='retiring',phase='retire',resources=$2,next_attempt_at=$3,lease_id=NULL,lease_expires_at=NULL,revision=revision+1,updated_at=$3 WHERE image_id=$1 AND state='ready'")
                .bind(&id)
                .bind(json!({}))
                .bind(now)
                .execute(&mut *tx)
                .await;
            match started {
                Ok(_) => {
                    tx.commit().await?;
                    return Ok(true);
                }
                Err(error)
                    if error.as_database_error().and_then(|e| e.constraint())
                        == Some("cloud_machine_images_one_active") =>
                {
                    tx.rollback().await?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(false)
    }
}
