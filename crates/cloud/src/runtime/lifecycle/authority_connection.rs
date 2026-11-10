//! Connection reuse chooses a transport, never authority or a cached decision.
use super::*;
use sqlx::pool::PoolConnection;

fn personal_memory_resume(row: &Value, request: &RuntimeRequest) -> bool {
    let c = &row["request_config"];
    let p = &c["rustMemoryParked"];
    matches!(
        request,
        RuntimeRequest::Query | RuntimeRequest::Resumed { .. }
    ) && row["account_id"]
        .as_str()
        .is_some_and(|v| !v.starts_with("organization:"))
        && matches!(row["provider"].as_str(), Some("boxd" | "e2b"))
        && row["desired_state"] == "ready"
        && c["rustLifecycleOperation"]["action"] == "resume"
        && c["rustMemorySleepVersion"] == 1
        && p["version"] == 1
        && p["generation"] == c["runtimeGeneration"]
        && p["sourceMachineId"] == row["provider_sandbox_id"]
        && p["storageIncarnationId"] == c["rustRuntimeStorage"]["storageIncarnationId"]
        && !super::super::repair::leases_blocked(row)
        && !super::super::recreation::leases_blocked(row)
}
impl RuntimeControl {
    pub(super) async fn runtime_authority_connection(
        &self,
        id: &str,
        request: &RuntimeRequest,
    ) -> Result<(Value, Option<PoolConnection<Postgres>>), ApiError> {
        let started = std::time::Instant::now();
        let mut connection = self.pool.acquire().await?;
        let acquired = std::time::Instant::now();
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(rejected)?;
        if personal_memory_resume(&row, request) {
            // Durations only: never log a bearer, row, account or request body.
            // Keep this opt-in so ordinary lifecycle queries do not add log traffic.
            tracing::debug!(
                target: "zuse_cloud::resume_timing",
                pool_wait_us = acquired.duration_since(started).as_micros() as u64,
                workspace_read_us = acquired.elapsed().as_micros() as u64,
                "memory_resume_authority_connection"
            );
            Ok((row, Some(connection)))
        } else {
            // Organization authorization and all other existing paths release
            // this checkout before invoking any independent pool/service seam.
            drop(connection);
            Ok((row, None))
        }
    }
}
