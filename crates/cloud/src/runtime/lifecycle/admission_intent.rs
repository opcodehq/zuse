//! One statement for personal resume intent, workspace and job. authority/workspace/capacity are already locked by caller.
use super::*;
pub(super) struct Intent<'a> {
    pub(super) operation: &'a str,
    pub(super) command: &'a str,
    pub(super) row: &'a Value,
    pub(super) generation: i64,
    pub(super) action: Action,
    pub(super) phase: &'a str,
    pub(super) state: &'a str,
    pub(super) desired: &'a str,
    pub(super) config: Value,
    pub(super) requested: Option<i64>,
    pub(super) deadline: Option<i64>,
    pub(super) now: i64,
}
pub(super) async fn persist(
    tx: &mut Transaction<'_, Postgres>,
    i: Intent<'_>,
) -> Result<Value, ApiError> {
    let id = required(i.row, "workspace_id")?;
    let source = required(i.row, "provider_sandbox_id")?;
    let storage = required(
        &i.row["request_config"]["rustRuntimeStorage"],
        "storageIncarnationId",
    )?;
    let row:Option<Value>=sqlx::query_scalar(
        "WITH intent AS (INSERT INTO zuse_rust_lifecycle_operations(operation_id,workspace_id,command_id,generation,source_machine_id,storage_incarnation_id,action,phase,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$9) RETURNING operation_id), workspace AS (UPDATE api_cloud_workspaces w SET state=$10,desired_state=$11,status_code=$12,request_config=$13,archive_requested_at=$14,archive_delete_at=$15,revision=revision+1,updated_at=GREATEST(updated_at+1,$9),next_action_at=$9 WHERE w.workspace_id=$2 AND EXISTS(SELECT 1 FROM intent) RETURNING to_jsonb(w) AS body), job AS (INSERT INTO zuse_rust_jobs(job_id,idempotency_key,kind,resource_id,resource_generation,payload,due_at,created_at,updated_at,max_attempts) SELECT $16,$17,$18,$2,$4,$19,$9,$9,$9,120 FROM workspace WHERE $8<>'paused' ON CONFLICT(idempotency_key) DO UPDATE SET idempotency_key=EXCLUDED.idempotency_key,max_attempts=120 WHERE zuse_rust_jobs.kind=EXCLUDED.kind AND zuse_rust_jobs.resource_id=EXCLUDED.resource_id AND zuse_rust_jobs.resource_generation=EXCLUDED.resource_generation AND zuse_rust_jobs.payload=EXCLUDED.payload RETURNING job_id) SELECT body FROM workspace WHERE $8='paused' OR EXISTS(SELECT 1 FROM job)"
    ).bind(i.operation).bind(id).bind(i.command).bind(i.generation).bind(source).bind(storage).bind(i.action.as_str()).bind(i.phase).bind(i.now).bind(i.state).bind(i.desired).bind(format!("{}-{}",i.action.as_str(),i.phase)).bind(i.config).bind(i.requested).bind(i.deadline).bind(uuid::Uuid::new_v4().to_string()).bind(format!("lifecycle:{}",i.operation)).bind(JOB_KIND).bind(json!({"operationId":i.operation})).fetch_optional(&mut **tx).await?;
    row.ok_or_else(|| ApiError::conflict("cloud_lifecycle_job_idempotency_conflict"))
}
