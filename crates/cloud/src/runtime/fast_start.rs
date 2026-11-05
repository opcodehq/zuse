//! Prepare the selected account concurrently; credentials still require guest enrollment.
use super::*;
impl RuntimeControl {
    pub fn with_fast_start_accounts(mut self, accounts: Vec<String>) -> Self {
        self.fast_start_accounts = accounts;
        self
    }
    pub(super) async fn prepare_allocation_access(&self, job: &crate::jobs::Job) {
        if self.fast_start_accounts.is_empty() {
            return;
        }
        let started = std::time::Instant::now();
        let result = async {
            let Some(authority) = &self.authority else { return Ok(()); };
            let row: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1 AND desired_state='ready' AND request_config->>'rustFastStartVersion'='1' AND (request_config->>'runtimeGeneration')::bigint=$2 AND request_config->>'rustAllocationOperationId'=$3")
                .bind(&job.resource_id).bind(job.generation).bind(job.payload["operationId"].as_str()).fetch_optional(&self.pool).await?;
            let Some(row) = row else { return Ok(()); };
            let config = &row["request_config"];
            let (Some(connection), Some(owner), Some(agent)) = (config["agentConnectionId"].as_str(), config["agentConnectionOwnerId"].as_str(), config["agent"].as_str()) else { return Ok(()); };
            self.scope_access(&job.resource_id, config["rustActorId"].as_str(), true).await?;
            let script = std::env::var_os("ZUSE_CLOUD_AUTH_DRIVER").map(std::path::PathBuf::from)
                .unwrap_or_else(|| "/usr/local/lib/zuse/auth-driver.py".into());
            let driver = crate::connections::worker::CommandDriver::new(&script)
                .map_err(|_| ApiError::unavailable("cloud_agent_prefetch_unavailable"))?;
            let agent = if agent == "claude-code" { "claude" } else { agent };
            // Central authority owns refresh serialization. Nothing is delivered
            // to the guest or retained in this allocation task.
            let _access = authority.credential(owner, connection, agent, false, &driver).await?;
            Ok::<(), ApiError>(())
        }.await;
        tracing::info!(workspace=%job.resource_id, phase="selected-account-preparation", duration_ms=started.elapsed().as_millis() as u64, succeeded=result.is_ok(), "Cloud startup phase");
    }
}
#[cfg(test)]
mod tests;
