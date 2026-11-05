//! One statement preserves owner -> workspace -> operation -> job lock order.
//! Provider I/O remains outside the transaction; only immutable original identity
//! and the current lease authorize a subsequent effect.
use super::*;
pub(super) fn validate_workspace(
    owner: Option<i64>,
    row: Option<Value>,
) -> Result<(Value, i64, i64), ApiError> {
    let owner = owner.ok_or_else(|| ApiError::unauthorized("workspace_runtime_fenced"))?;
    let row = row.ok_or_else(rejected)?;
    let generation = row["request_config"]["runtimeGeneration"]
        .as_i64()
        .unwrap_or(1);
    let epoch = row["request_config"]["gatewayEpoch"]
        .as_i64()
        .unwrap_or(generation);
    if owner != generation || generation < 1 || epoch < 1 {
        return Err(ApiError::unauthorized("workspace_runtime_fenced"));
    }
    Ok((row, generation, epoch))
}
pub(super) async fn workspace(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    exclusive_owner: bool,
) -> Result<(Value, i64, i64), ApiError> {
    let mode = if exclusive_owner { "UPDATE" } else { "SHARE" };
    let statement = format!(
        "WITH authority AS MATERIALIZED (SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR {mode}), workspace AS MATERIALIZED (SELECT to_jsonb(w) AS body FROM api_cloud_workspaces w CROSS JOIN authority WHERE w.workspace_id=$1 FOR UPDATE OF w) SELECT (SELECT generation FROM authority),(SELECT body FROM workspace)"
    );
    let (owner, row): (Option<i64>, Option<Value>) = sqlx::query_as(&statement)
        .bind(id)
        .fetch_one(&mut **tx)
        .await?;
    validate_workspace(owner, row)
}
pub(super) const JOB_SNAPSHOT: &str = "WITH authority AS MATERIALIZED (SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR SHARE), workspace AS MATERIALIZED (SELECT to_jsonb(w) AS body FROM api_cloud_workspaces w CROSS JOIN authority WHERE w.workspace_id=$1 FOR UPDATE OF w), operation AS MATERIALIZED (SELECT to_jsonb(o) AS body FROM zuse_rust_lifecycle_operations o CROSS JOIN workspace w WHERE o.workspace_id=$1 AND o.operation_id=w.body#>>'{request_config,rustLifecycleOperation,id}' FOR UPDATE OF o), lease AS MATERIALIZED (SELECT j.lease_expires_at FROM zuse_rust_jobs j CROSS JOIN operation o WHERE j.job_id=$2 AND j.lease_token=$3 AND j.state='running' AND j.resource_id=$1 AND j.resource_generation=$4 AND j.kind=$6 AND j.payload->>'operationId'=o.body->>'operation_id' FOR UPDATE OF j) SELECT (SELECT generation FROM authority),(SELECT body FROM workspace),(SELECT body FROM operation),EXISTS(SELECT 1 FROM lease WHERE lease_expires_at>GREATEST($5::bigint,(extract(epoch from clock_timestamp())*1000)::bigint))";

/// Retain authority -> workspace -> operation lock ordering in one statement.
pub(super) async fn workspace_operation(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<(Value, Option<Value>), ApiError> {
    let (owner, row, operation): (Option<i64>, Option<Value>, Option<Value>) = sqlx::query_as(
        "WITH authority AS MATERIALIZED (SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1 AND backend='rust' FOR SHARE), workspace AS MATERIALIZED (SELECT to_jsonb(w) AS body FROM api_cloud_workspaces w CROSS JOIN authority WHERE w.workspace_id=$1 FOR UPDATE OF w), operation AS MATERIALIZED (SELECT to_jsonb(o) AS body FROM zuse_rust_lifecycle_operations o CROSS JOIN workspace w WHERE o.workspace_id=$1 AND o.operation_id=w.body#>>'{request_config,rustLifecycleOperation,id}' FOR UPDATE OF o) SELECT (SELECT generation FROM authority),(SELECT body FROM workspace),(SELECT body FROM operation)"
    ).bind(id).fetch_one(&mut **tx).await?;
    let (row, _, _) = validate_workspace(owner, row)?;
    Ok((row, operation))
}
