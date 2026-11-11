//! Provider running limits must not end a computer that is still in use.
//! E2B pauses a computer when its timeout ends, even mid-turn, and the
//! staging plan caps that timeout at one hour. The worker resets it for every
//! awake computer that is busy: a task queued or running, or user activity
//! within the idle window. An abandoned computer keeps the provider's own
//! limit as a safety net; the idle scheduler governs everything else.
use super::{RuntimeControl, activity::COALESCE_MS};
use crate::{error::ApiError, providers::registry::Registry};
use zuse_protocol::cloud_computers::ProviderId;

/// Run well inside E2B's one-hour cap. Provider timeouts shorter than twice
/// this interval are rejected at configuration (see `E2b::with_max_timeout`).
pub const INTERVAL_SECS: u64 = 600;
/// One renewal request may take at most this long.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// Renewals in flight at once, so a pass over many computers stays short:
/// 1,000 computers that all time out still finish in about 21 minutes, inside
/// the 40 minutes left before a one-hour limit ends.
const CONCURRENCY: usize = 16;
const PAGE: i64 = 500;

pub(crate) const BUSY_COMPUTERS: &str = "SELECT w.workspace_id,w.provider,w.provider_sandbox_id FROM api_cloud_workspaces w WHERE w.state='ready' AND w.desired_state='ready' AND w.runtime_state='online' AND w.provider_sandbox_id IS NOT NULL AND w.provider=ANY($2) AND w.workspace_id>$3 AND (COALESCE(w.last_activity_at,0)>=$1 OR EXISTS(SELECT 1 FROM zuse_rust_task_command_execution e WHERE e.workspace_id=w.workspace_id AND e.state IN('queued','starting','running','waiting-input'))) ORDER BY w.workspace_id LIMIT $4";

impl RuntimeControl {
    /// Returns how many computers had their provider limit reset.
    pub async fn keep_running_registry_once(
        &self,
        registry: &Registry,
        now: i64,
    ) -> Result<usize, ApiError> {
        use futures::StreamExt as _;
        let limited: Vec<String> = ProviderId::ALL
            .into_iter()
            .filter(|id| registry.get(*id).is_some_and(|p| p.has_running_limit()))
            .map(|id| id.as_str().to_owned())
            .collect();
        if limited.is_empty() {
            return Ok(0);
        }
        let ready: bool = sqlx::query_scalar(
            "SELECT to_regclass('zuse_rust_task_command_execution') IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await?;
        if !ready {
            return Ok(0);
        }
        let since = now.saturating_sub(self.idle_ms).saturating_sub(COALESCE_MS);
        let mut after = String::new();
        let mut kept = 0;
        loop {
            let rows: Vec<(String, String, String)> = sqlx::query_as(BUSY_COMPUTERS)
                .bind(since)
                .bind(&limited)
                .bind(&after)
                .bind(PAGE)
                .fetch_all(&self.pool)
                .await?;
            let Some((last, _, _)) = rows.last() else {
                break;
            };
            after = last.clone();
            let full = rows.len() as i64 == PAGE;
            kept += futures::stream::iter(rows)
                .map(|(_, provider, machine)| async move {
                    let adapter = ProviderId::ALL
                        .into_iter()
                        .find(|id| id.as_str() == provider)
                        .and_then(|id| registry.get(id))?;
                    match tokio::time::timeout(REQUEST_TIMEOUT, adapter.keep_running(&machine))
                        .await
                    {
                        Ok(Ok(())) => Some(()),
                        Ok(Err(error)) => {
                            tracing::warn!(
                                provider,
                                ?error,
                                "Provider running limit not reset; retrying next interval"
                            );
                            None
                        }
                        Err(_) => {
                            tracing::warn!(
                                provider,
                                "Provider running limit reset timed out; retrying next interval"
                            );
                            None
                        }
                    }
                })
                .buffer_unordered(CONCURRENCY)
                .filter_map(std::future::ready)
                .count()
                .await;
            if !full {
                break;
            }
        }
        Ok(kept)
    }
}

#[cfg(test)]
#[path = "keep_running_tests.rs"]
mod tests;
