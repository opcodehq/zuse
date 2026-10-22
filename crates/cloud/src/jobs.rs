//! PostgreSQL job delivery; provider operations remain in typed reconcilers.
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

pub const MIGRATION: &str = include_str!("../migrations/0001_control_plane.sql");

pub struct Job {
    pub id: String,
    pub kind: String,
    pub resource_id: String,
    pub generation: i64,
    pub payload: Value,
    pub lease_token: String,
    pub attempts: i32,
}

/// Enqueue inside the lifecycle intent's transaction, before acknowledging it.
pub async fn enqueue(
    tx: &mut Transaction<'_, Postgres>,
    key: &str,
    kind: &str,
    resource: &str,
    generation: i64,
    payload: &Value,
    now: i64,
) -> Result<String, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    let row = sqlx::query("INSERT INTO zuse_rust_jobs (job_id,idempotency_key,kind,resource_id,resource_generation,payload,due_at,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$7,$7) ON CONFLICT (idempotency_key) DO UPDATE SET idempotency_key=EXCLUDED.idempotency_key WHERE zuse_rust_jobs.kind=EXCLUDED.kind AND zuse_rust_jobs.resource_id=EXCLUDED.resource_id AND zuse_rust_jobs.resource_generation=EXCLUDED.resource_generation AND zuse_rust_jobs.payload=EXCLUDED.payload RETURNING job_id")
        .bind(id).bind(key).bind(kind).bind(resource).bind(generation).bind(payload).bind(now)
        .fetch_optional(&mut **tx).await?;
    row.map(|r| r.try_get("job_id"))
        .unwrap_or_else(|| Err(sqlx::Error::Protocol("job_idempotency_conflict".into())))
}

/// Claim only Rust-owned resources at the exact generation. A missing owner is legacy.
pub async fn claim(
    pool: &PgPool,
    kind: &str,
    owner_kind: &str,
    now: i64,
    lease_ms: i64,
) -> Result<Option<Job>, sqlx::Error> {
    if !(1000..=300000).contains(&lease_ms) {
        return Err(sqlx::Error::Protocol("invalid_job_lease".into()));
    }
    let token = Uuid::new_v4().to_string();
    let row = sqlx::query("WITH exhausted AS (UPDATE zuse_rust_jobs SET state='failed',lease_token=NULL,lease_expires_at=NULL,last_error_code='job_attempts_exhausted',updated_at=$3 WHERE kind=$1 AND state='running' AND lease_expires_at <= $3 AND attempts >= max_attempts RETURNING job_id), candidate AS (SELECT j.job_id FROM zuse_rust_jobs j JOIN zuse_rust_control_owners o ON o.resource_id=j.resource_id AND o.resource_kind=$2 AND o.backend='rust' AND o.generation=j.resource_generation WHERE j.kind=$1 AND j.state IN ('pending','running') AND j.due_at <= $3 AND (j.lease_expires_at IS NULL OR j.lease_expires_at <= $3) AND j.attempts < j.max_attempts ORDER BY j.due_at,j.created_at FOR UPDATE OF j SKIP LOCKED LIMIT 1) UPDATE zuse_rust_jobs j SET state='running',attempts=attempts+1,lease_token=$4,lease_expires_at=$5,updated_at=$3 FROM candidate WHERE j.job_id=candidate.job_id RETURNING j.*")
        .bind(kind).bind(owner_kind).bind(now).bind(&token).bind(now.saturating_add(lease_ms)).fetch_optional(pool).await?;
    row.map(|row| {
        Ok(Job {
            id: row.try_get("job_id")?,
            kind: row.try_get("kind")?,
            resource_id: row.try_get("resource_id")?,
            generation: row.try_get("resource_generation")?,
            payload: row.try_get("payload")?,
            lease_token: token,
            attempts: row.try_get("attempts")?,
        })
    })
    .transpose()
}

pub async fn finish(
    pool: &PgPool,
    job: &Job,
    owner_kind: &str,
    now: i64,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("UPDATE zuse_rust_jobs j SET state='completed',lease_token=NULL,lease_expires_at=NULL,updated_at=$4 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at > $4 AND EXISTS (SELECT 1 FROM zuse_rust_control_owners o WHERE o.resource_kind=$3 AND o.resource_id=j.resource_id AND o.backend='rust' AND o.generation=j.resource_generation)")
        .bind(&job.id).bind(&job.lease_token).bind(owner_kind).bind(now).execute(pool).await?;
    Ok(result.rows_affected() == 1)
}

pub async fn retry(
    pool: &PgPool,
    job: &Job,
    now: i64,
    error_code: &str,
) -> Result<bool, sqlx::Error> {
    if error_code.is_empty()
        || error_code.len() > 128
        || !error_code
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(sqlx::Error::Protocol("invalid_job_error_code".into()));
    }
    let backoff = (1000_i64 << job.attempts.clamp(0, 8)).min(300000);
    let result = sqlx::query("UPDATE zuse_rust_jobs SET state=CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'pending' END,due_at=$4,lease_token=NULL,lease_expires_at=NULL,last_error_code=$5,updated_at=$3 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at > $3")
        .bind(&job.id).bind(&job.lease_token).bind(now).bind(now.saturating_add(backoff)).bind(error_code).execute(pool).await?;
    Ok(result.rows_affected() == 1)
}
