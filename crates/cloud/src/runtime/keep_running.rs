//! Provider running limits must not end a computer that is still in use.
//! E2B pauses a computer when its timeout ends, even mid-turn, and the
//! staging plan caps that timeout at one hour. The worker resets it for every
//! awake computer that is busy: a task queued or running, or user activity
//! within the idle window. An abandoned computer keeps the provider's own
//! limit as a safety net; the idle scheduler governs everything else.
use super::{RuntimeControl, activity::COALESCE_MS};
use crate::{error::ApiError, providers::registry::Registry};
use zuse_protocol::cloud_computers::ProviderId;

/// Run well inside E2B's one-hour cap.
pub const INTERVAL_SECS: u64 = 600;

pub(crate) const BUSY_COMPUTERS: &str = "SELECT w.provider,w.provider_sandbox_id FROM api_cloud_workspaces w WHERE w.state='ready' AND w.desired_state='ready' AND w.runtime_state='online' AND w.provider_sandbox_id IS NOT NULL AND (COALESCE(w.last_activity_at,0)>=$1 OR EXISTS(SELECT 1 FROM zuse_rust_task_command_execution e WHERE e.workspace_id=w.workspace_id AND e.state IN('queued','starting','running','waiting-input'))) ORDER BY w.workspace_id LIMIT 500";

impl RuntimeControl {
    /// Returns how many computers had their provider limit reset.
    pub async fn keep_running_registry_once(
        &self,
        registry: &Registry,
        now: i64,
    ) -> Result<usize, ApiError> {
        let ready: bool = sqlx::query_scalar(
            "SELECT to_regclass('zuse_rust_task_command_execution') IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await?;
        if !ready {
            return Ok(0);
        }
        let since = now.saturating_sub(self.idle_ms).saturating_sub(COALESCE_MS);
        let rows: Vec<(String, String)> = sqlx::query_as(BUSY_COMPUTERS)
            .bind(since)
            .fetch_all(&self.pool)
            .await?;
        let mut kept = 0;
        for (provider, machine) in rows {
            let Some(adapter) = ProviderId::ALL
                .into_iter()
                .find(|id| id.as_str() == provider)
                .and_then(|id| registry.get(id))
            else {
                continue;
            };
            match adapter.keep_running(&machine).await {
                Ok(()) => kept += 1,
                Err(error) => tracing::warn!(
                    provider,
                    ?error,
                    "Provider running limit not reset; retrying next interval"
                ),
            }
        }
        Ok(kept)
    }
}
