//! Reuse one live PostgreSQL checkout for the personal RAM-resume critical path.
//! No transaction or database lock crosses provider I/O. Every effect still uses
//! the ordinary current-operation/lease/capacity fence on that connection.
use super::*;
use sqlx::pool::PoolConnection;

impl RuntimeControl {
    pub(super) async fn personal_memory_connection(
        &self,
        job: &jobs::Job,
    ) -> Result<Option<PoolConnection<Postgres>>, ApiError> {
        let mut connection = self.pool.acquire().await?;
        let (row, action): (Value, Option<String>) = sqlx::query_as(
            "SELECT to_jsonb(w),(SELECT action FROM zuse_rust_lifecycle_operations o WHERE o.workspace_id=w.workspace_id AND o.operation_id=$2 AND o.generation=$3) FROM api_cloud_workspaces w WHERE w.workspace_id=$1",
        )
        .bind(&job.resource_id)
        .bind(job.payload["operationId"].as_str())
        .bind(job.generation)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(|| ApiError::not_found("cloud_workspace_not_found"))?;
        let c = &row["request_config"];
        let parked = &c["rustMemoryParked"];
        // Organization/requester authorization can need another service/pool.
        // Keep its established path and never hold this checkout across it.
        let personal = row["account_id"]
            .as_str()
            .is_some_and(|owner| !owner.starts_with("organization:"));
        let candidate_shape =
            c["managedCandidateSha256"].is_null() || c["managedCandidateSha256"].is_string();
        let eligible = personal
            && action.as_deref() == Some("resume")
            && c["rustMemorySleepVersion"] == 1
            && c["runtimeGeneration"] == job.generation
            && parked["version"] == 1
            && parked["generation"] == job.generation
            && parked["sourceMachineId"] == row["provider_sandbox_id"]
            && parked["storageIncarnationId"] == c["rustRuntimeStorage"]["storageIncarnationId"]
            && parked["operationId"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
            && c["rustStorageEnrollment"]["capabilities"]
                .as_array()
                .is_some_and(|a| {
                    a.iter()
                        .any(|v| v == zuse_protocol::cloud_lifecycle::MEMORY_CAPABILITY)
                })
            && c["rustRecreation"].is_null()
            && !super::super::super::repair::leases_blocked(&row)
            && candidate_shape;
        if !eligible {
            return Ok(None);
        }
        // This chooses a connection, not permission to touch a provider. The
        // immediately following locked snapshot applies the exact stored pin,
        // actor, capacity and lease checks before even provider inspection.
        Ok(Some(connection))
    }
}
