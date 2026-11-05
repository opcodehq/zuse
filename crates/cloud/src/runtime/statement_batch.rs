//! Batch transport only: policy stays in the existing Rust transaction owner.
//! SQL text must be static server code. Request data is always a bound JSON value.
use super::*;
pub(crate) const MIGRATION: &str =
    include_str!("../../migrations/0029_runtime_statement_batch.sql");

impl RuntimeControl {
    async fn startup_capabilities(&self) -> Result<(bool, bool, bool), ApiError> {
        if self.fast_start_accounts.is_empty() {
            return Ok((false, false, false));
        }
        Ok(*self.startup_schema.get_or_try_init(||async {
            sqlx::query_as::<_,(bool,bool,bool)>("SELECT to_regprocedure('zuse_rust_runtime_statement_batch(jsonb,jsonb)') IS NOT NULL,to_regprocedure('zuse_rust_transcript_write_intents_v1()') IS NOT NULL,to_regclass('zuse_rust_provider_creates') IS NOT NULL")
                .fetch_one(&self.pool).await.map_err(ApiError::from)
        }).await?)
    }
    pub(super) async fn statement_batch_available(&self) -> Result<bool, ApiError> {
        Ok(self.startup_capabilities().await?.0)
    }
    pub(super) async fn transcript_intents_available(&self) -> Result<bool, ApiError> {
        Ok(self.startup_capabilities().await?.1)
    }
}

pub(super) async fn run(
    tx: &mut Transaction<'_, Postgres>,
    statements: &'static [(&'static str, &'static str)],
    arguments: Value,
) -> Result<Value, ApiError> {
    on_connection(&mut **tx, statements, arguments).await
}

pub(super) async fn on_connection(
    connection: &mut sqlx::PgConnection,
    statements: &'static [(&'static str, &'static str)],
    arguments: Value,
) -> Result<Value, ApiError> {
    // This API deliberately cannot accept SQL assembled from request strings.
    let statements: Vec<Value> = statements
        .iter()
        .map(|(name, sql)| json!({"name":name,"sql":sql}))
        .collect();
    Ok(
        sqlx::query_scalar("SELECT zuse_rust_runtime_statement_batch($1,$2)")
            .bind(json!(statements))
            .bind(arguments)
            .fetch_one(connection)
            .await?,
    )
}

#[cfg(test)]
mod tests;

pub(super) const CANDIDATE_READ: &[(&str, &str)] = &[
    (
        "schema",
        "SELECT to_jsonb(EXISTS(SELECT 1 FROM information_schema.columns WHERE table_schema=current_schema() AND table_name='cloud_managed_bases' AND column_name='candidate_manifest'))",
    ),
    (
        "record",
        "SELECT (SELECT to_jsonb(b) FROM zuse_rust_managed_base_admission_lock($1->>'provider',$1->>'release') b)",
    ),
    (
        "inventory",
        "SELECT jsonb_build_object('count',count(*),'owns',bool_or(workspace_id=$1->>'workspace' AND provider=$1->>'provider' AND request_config->>'managedReleaseId'=$1->>'release' AND request_config->>'managedCandidateSha256'=$1->>'candidate' AND request_config->>'managedCandidateArtifactId'=$1->>'artifact')) FROM api_cloud_workspaces WHERE account_id=$1->>'owner' AND state<>'deleted' AND request_config ? 'managedCandidateSha256'",
    ),
];

pub(super) fn candidate_arguments(
    pin: &crate::managed_bases::candidate::StagingBasePin,
    row: &Value,
) -> Result<Value, ApiError> {
    let effective = pin.pin_for_row(row)?;
    Ok(
        json!({"provider":effective.provider.as_str(),"release":effective.release_id,
        "owner":effective.owner_id,"workspace":required(row,"workspace_id")?,
        "candidate":effective.candidate_sha256,"artifact":effective.artifact_id}),
    )
}

impl RuntimeControl {
    pub(super) async fn candidate_row_allowed_batched(
        &self,
        pin: &crate::managed_bases::candidate::StagingBasePin,
        row: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        let arguments = candidate_arguments(pin, row)?;
        let mut connection = self.pool.acquire().await?;
        let observed = on_connection(&mut connection, CANDIDATE_READ, arguments).await?;
        // This read-only batch has committed its locks before any workspace
        // transaction begins. Validate the immutable returned record before any
        // effect, just as the original row_allowed path does before returning.
        pin.validate_row_observation(row, &observed, now)
    }
}

const CAPACITY_READ: &[(&str, &str)] = &[
    (
        "lock",
        "WITH locked AS MATERIALIZED(SELECT pg_advisory_xact_lock(hashtextextended('cloud-create:'||($1->>'owner'),0))) SELECT NULL::jsonb FROM locked",
    ),
    CANDIDATE_READ[0],
    CANDIDATE_READ[1],
    CANDIDATE_READ[2],
];
impl RuntimeControl {
    pub(super) async fn allocation_capacity(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        now: i64,
    ) -> Result<(), ApiError> {
        // Availability is discovered before taking a workspace transaction;
        // never borrow a second pooled connection behind its lock.
        if self
            .startup_schema
            .get()
            .is_some_and(|capabilities| capabilities.0)
            && row["request_config"]["rustCreateRequestDigest"].is_string()
            && row["request_config"]["managedCandidateSha256"].is_string()
        {
            let pin = self
                .staging_candidate
                .as_ref()
                .ok_or_else(|| ApiError::conflict("staging_managed_base_candidate_unavailable"))?;
            let arguments = candidate_arguments(pin, row)?;
            let observed = run(tx, CAPACITY_READ, arguments).await?;
            if pin.validate_capacity_observation(row, &observed, now)? {
                return Ok(());
            }
        }
        super::allocation::check_creation_capacity(self.staging_candidate.as_deref(), tx, row, now)
            .await
    }
}

const ALLOCATION_READ: &[(&str, &str)] = &[
    (
        "workspace",
        "WITH owner AS MATERIALIZED(SELECT generation FROM zuse_rust_control_owners WHERE resource_kind='workspace' AND resource_id=$1->>'workspace' AND backend='rust' FOR SHARE), workspace AS MATERIALIZED(SELECT to_jsonb(w) AS row FROM api_cloud_workspaces w WHERE workspace_id=$1->>'workspace' AND EXISTS(SELECT 1 FROM owner) FOR UPDATE) SELECT jsonb_build_object('generation',(SELECT generation FROM owner),'row',(SELECT row FROM workspace))",
    ),
    (
        "operation",
        "SELECT(SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1->>'operation' AND workspace_id=$1->>'workspace' AND generation=($1->>'generation')::bigint FOR UPDATE)",
    ),
    (
        "lock",
        "WITH locked AS MATERIALIZED(SELECT pg_advisory_xact_lock(hashtextextended('cloud-create:'||($2#>>'{workspace,row,account_id}'),0)) WHERE jsonb_typeof($2#>'{workspace,row,request_config,rustCreateRequestDigest}')='string' AND jsonb_typeof($2#>'{workspace,row,request_config,managedCandidateSha256}')='string') SELECT(SELECT NULL::jsonb FROM locked)",
    ),
    CANDIDATE_READ[0],
    (
        "record",
        "SELECT(SELECT to_jsonb(b) FROM zuse_rust_managed_base_admission_lock($2#>>'{workspace,row,provider}',$2#>>'{workspace,row,request_config,managedReleaseId}') b)",
    ),
    (
        "inventory",
        "SELECT jsonb_build_object('count',count(*),'owns',bool_or(workspace_id=$1->>'workspace' AND provider=$2#>>'{workspace,row,provider}' AND request_config->>'managedReleaseId'=$2#>>'{workspace,row,request_config,managedReleaseId}' AND request_config->>'managedCandidateSha256'=$2#>>'{workspace,row,request_config,managedCandidateSha256}' AND request_config->>'managedCandidateArtifactId'=$2#>>'{workspace,row,request_config,managedCandidateArtifactId}')) FROM api_cloud_workspaces WHERE account_id=$2#>>'{workspace,row,account_id}' AND state<>'deleted' AND request_config ? 'managedCandidateSha256'",
    ),
    (
        "live",
        "SELECT to_jsonb(EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1->>'job' AND lease_token=$1->>'lease' AND state='running' AND lease_expires_at>GREATEST(($1->>'now')::bigint,(extract(epoch FROM clock_timestamp())*1000)::bigint)))",
    ),
];

const DISPATCH_READ: &[(&str, &str)] = &[
    ALLOCATION_READ[0], ALLOCATION_READ[1], ALLOCATION_READ[2],
    ALLOCATION_READ[3], ALLOCATION_READ[4], ALLOCATION_READ[5],
    ("leaseUntil", "SELECT(SELECT to_jsonb(lease_expires_at) FROM zuse_rust_jobs WHERE job_id=$1->>'job' AND lease_token=$1->>'lease' AND state='running' AND lease_expires_at>GREATEST(($1->>'now')::bigint,(extract(epoch FROM clock_timestamp())*1000)::bigint) AND resource_generation=($2#>>'{workspace,generation}')::bigint FOR UPDATE)"),
    ("journal", "SELECT(SELECT to_jsonb(c) FROM zuse_rust_provider_creates c WHERE operation_id=$1->>'operation' FOR UPDATE)"),
];

pub(super) struct AllocationSnapshot {
    pub(super) workspace: Value,
    pub(super) generation: i64,
    pub(super) operation: Value,
    pub(super) live: bool,
    pub(super) capacity: Value,
}
impl RuntimeControl {
    pub(super) async fn allocation_snapshot(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        job: &crate::jobs::Job,
    ) -> Result<Option<AllocationSnapshot>, ApiError> {
        self.read_allocation_snapshot(tx, job, ALLOCATION_READ).await
    }
    pub(super) async fn allocation_dispatch_snapshot(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        job: &crate::jobs::Job,
    ) -> Result<Option<AllocationSnapshot>, ApiError> {
        if !self.startup_schema.get().is_some_and(|schema| schema.2) {
            return Ok(None);
        }
        self.read_allocation_snapshot(tx, job, DISPATCH_READ).await
    }
    async fn read_allocation_snapshot(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        job: &crate::jobs::Job,
        statements: &'static [(&'static str, &'static str)],
    ) -> Result<Option<AllocationSnapshot>, ApiError> {
        if !self
            .startup_schema
            .get()
            .is_some_and(|capabilities| capabilities.0)
        {
            return Ok(None);
        }
        let result = run(
            tx,
            statements,
            json!({"workspace":job.resource_id,
            "operation":job.payload["operationId"],"job":job.id,"lease":job.lease_token,
            "generation":job.generation,"now":chrono::Utc::now().timestamp_millis()}),
        )
        .await?;
        let generation = result["workspace"]["generation"]
            .as_i64()
            .ok_or_else(|| ApiError::unauthorized("workspace_runtime_fenced"))?;
        let workspace = result["workspace"]["row"].clone();
        if workspace.is_null() {
            return Err(rejected());
        }
        let actual = workspace["request_config"]["runtimeGeneration"]
            .as_i64()
            .unwrap_or(1);
        let epoch = workspace["request_config"]["gatewayEpoch"]
            .as_i64()
            .unwrap_or(actual);
        if generation != actual || generation < 1 || epoch < 1 {
            return Err(ApiError::unauthorized("workspace_runtime_fenced"));
        }
        if result["operation"].is_null() {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        Ok(Some(AllocationSnapshot {
            workspace,
            generation,
            operation: result["operation"].clone(),
            live: result["live"] == true,
            capacity: result,
        }))
    }
    pub(super) async fn allocation_snapshot_capacity(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        row: &Value,
        observed: Option<&Value>,
        now: i64,
    ) -> Result<(), ApiError> {
        if !row["request_config"]["rustCreateRequestDigest"].is_string() {
            return Ok(());
        }
        if let Some(observed) =
            observed.filter(|_| row["request_config"]["managedCandidateSha256"].is_string())
        {
            let pin = self
                .staging_candidate
                .as_ref()
                .ok_or_else(|| ApiError::conflict("staging_managed_base_candidate_unavailable"))?;
            if pin.validate_capacity_observation(row, observed, now)? {
                return Ok(());
            }
        }
        self.allocation_capacity(tx, row, now).await
    }
}

#[cfg(test)]
pub(crate) async fn fixture_schema(pool: &PgPool) {
    sqlx::raw_sql("CREATE TABLE IF NOT EXISTS cloud_managed_bases(provider text,release_id text,artifact_id text,architecture text,capabilities jsonb,state text,created_at bigint,runtime_manifest jsonb,candidate_manifest jsonb,PRIMARY KEY(provider,release_id));")
        .execute(pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/0023_managed_base_admission_lock.sql"
    ))
    .execute(pool)
    .await
    .unwrap();
    sqlx::raw_sql(MIGRATION).execute(pool).await.unwrap();
}
