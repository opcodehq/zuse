//! Initial prepared-image allocation. Retained/recovery storage never enters this path.
use super::*;
use crate::{
    jobs,
    providers::{Machine, MachineState, ProviderError, boxd::Boxd},
};
use async_trait::async_trait;
use zeroize::Zeroizing;

pub const MIGRATION: &str = include_str!("../../migrations/0004_allocations.sql");
pub const JOB_KIND: &str = "workspace-allocate";
const BOOT_TTL: i64 = 30 * 60 * 1000;

/// Deliberately not Debug/Serialize: the token must never enter logs or job payloads.
pub struct RuntimeLaunch {
    pub operation_id: String,
    pub workspace_id: String,
    pub generation: i64,
    pub incarnation: String,
    pub api_url: String,
    pub boot_token: Zeroizing<String>,
}
#[async_trait]
pub trait AllocationProvider: Send + Sync {
    async fn find(&self, name: &str) -> Result<Option<Machine>, ProviderError>;
    async fn create(&self, snapshot: &str, name: &str) -> Result<Machine, ProviderError>;
    /// Must be idempotent for one operation; an uncertain response is not evidence of failure.
    async fn launch(&self, machine: &str, launch: &RuntimeLaunch) -> Result<(), ProviderError>;
}
#[async_trait]
impl AllocationProvider for Boxd {
    async fn find(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        self.inspect(name).await
    }
    async fn create(&self, snapshot: &str, name: &str) -> Result<Machine, ProviderError> {
        self.create_from_snapshot(snapshot, name, true).await
    }
    async fn launch(&self, machine: &str, launch: &RuntimeLaunch) -> Result<(), ProviderError> {
        let staged = format!("/tmp/zuse-rust-boot-{}", launch.operation_id);
        self.upload(machine, &staged, launch.boot_token.as_bytes())
            .await?;
        let boot = format!("/var/lib/zuse/workspace/rust-boot-{}", launch.operation_id);
        let mut command = format!(
            "set -eu\nsudo -n install -o zuse -g zuse -m 600 {} {}\nsudo -n rm -- {}\nsudo -n -u zuse env",
            quote(&staged),
            quote(&boot),
            quote(&staged)
        );
        for (key, value) in [
            ("ZUSE_CLOUD_WORKSPACE_ID", launch.workspace_id.clone()),
            ("ZUSE_API_URL", launch.api_url.clone()),
            ("ZUSE_RUNTIME_GENERATION", launch.generation.to_string()),
            ("ZUSE_STORAGE_INCARNATION_ID", launch.incarnation.clone()),
            ("ZUSE_RUNTIME_OPERATION_ID", launch.operation_id.clone()),
            ("ZUSE_RUNTIME_BOOT_TOKEN_FILE", boot),
        ] {
            command.push_str(&format!(" {key}={}", quote(&value)));
        }
        command.push_str(" /usr/local/lib/zuse/launch-rust-runtime\n");
        let result = self.exec(machine, &command).await?;
        if result.exit_code != 0 {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn boot_context(workspace: &str, operation: &str, generation: i64) -> String {
    format!("rust-workspace-allocation-boot:{workspace}:{generation}:{operation}")
}
fn provider_error(error: ProviderError) -> ApiError {
    ApiError::unavailable(match error {
        ProviderError::Rejected => "cloud_provider_rejected",
        ProviderError::Conflict => "cloud_provider_conflict",
        ProviderError::RateLimited => "cloud_provider_rate_limited",
        ProviderError::NotFound => "cloud_provider_machine_missing",
        ProviderError::InvalidResponse => "cloud_provider_invalid_response",
        ProviderError::OutcomeUnknown => "cloud_provider_outcome_unknown",
        ProviderError::Unavailable => "cloud_provider_unavailable",
    })
}

impl RuntimeControl {
    pub async fn allocation_ready(&self) -> Result<(), ApiError> {
        let ready: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_allocations') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if !ready {
            return Err(ApiError::unavailable("cloud_allocation_schema_not_ready"));
        }
        Ok(())
    }
    /// Authority-only admission for a new, already-owned workspace and approved prepared image.
    /// HTTP creation must first authorize scope, image access, entitlement and spend reservation.
    pub async fn queue_allocation(
        &self,
        id: &str,
        snapshot: &str,
        now: i64,
    ) -> Result<String, ApiError> {
        if snapshot.is_empty() || snapshot.len() > 512 || snapshot.chars().any(char::is_control) {
            return Err(ApiError::bad_request("invalid_prepared_image"));
        }
        let mut tx = self.pool.begin().await?;
        let (row, generation, epoch) = locked_workspace(&mut tx, id).await?;
        if row["provider"] != "boxd"
            || required(&row, "account_id")?.starts_with("organization:")
            || row["request_config"]["providerConnectionId"].is_string()
            || blocked(&row)
            || row["desired_state"] != "ready"
        {
            return Err(ApiError::conflict("cloud_allocation_unavailable"));
        }
        let prior: Option<(String,String)> = sqlx::query_as("SELECT operation_id,snapshot_id FROM zuse_rust_allocations WHERE workspace_id=$1 AND generation=$2 FOR UPDATE")
            .bind(id).bind(generation).fetch_optional(&mut *tx).await?;
        if let Some((operation, image)) = prior {
            if image != snapshot {
                return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
            }
            tx.commit().await?;
            return Ok(operation);
        }
        let mut config = row["request_config"].clone();
        if row["provider_sandbox_id"].is_string()
            || row["runtime_credential_hash"].is_string()
            || row["runtime_boot_token_hash"].is_string()
            || config["sessionHeadVersion"].is_number()
            || !config["rustStorageEnrollment"].is_null()
            || !config["runtimeBootstrapReceipt"].is_null()
            || !config["recoveryArchive"].is_null()
            || !matches!(row["state"].as_str(), Some("queued" | "provisioning"))
        {
            return Err(ApiError::conflict("cloud_allocation_retained_state"));
        }
        let launch_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspace_launch_intents WHERE workspace_id=$1 AND expires_at>$2)")
            .bind(id).bind(now).fetch_one(&mut *tx).await?;
        if !launch_exists {
            return Err(ApiError::conflict(
                "cloud_workspace_launch_intent_unavailable",
            ));
        }
        let operation = uuid::Uuid::new_v4().to_string();
        let incarnation = uuid::Uuid::new_v4().to_string();
        let name = format!("zr-{}", hash(&format!("{id}\n{generation}\n{operation}")));
        let name = &name[..63];
        let boot = Zeroizing::new(format!(
            "rust_boot_{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ));
        let sealed = self
            .master
            .seal_api(&boot_context(id, &operation, generation), &boot)
            .map_err(|_| ApiError::unavailable("cloud_boot_grant_unavailable"))?;
        sqlx::query("INSERT INTO zuse_rust_allocations (operation_id,workspace_id,generation,snapshot_id,machine_name,storage_incarnation_id,sealed_boot_token,boot_expires_at,phase,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'planned',$9,$9)")
            .bind(&operation).bind(id).bind(generation).bind(snapshot).bind(name).bind(&incarnation).bind(sealed).bind(now+BOOT_TTL).bind(now).execute(&mut *tx).await?;
        config["runtimeGeneration"] = json!(generation);
        config["gatewayEpoch"] = json!(epoch);
        config["runtimeRpcProtocol"] = json!(zuse_protocol::cloud_gateway::RUST_RPC);
        config["rustRuntimeStorage"] =
            json!({"format":"zuse-rust-engine-v1","storageIncarnationId":incarnation});
        config["rustSessionCommandVersion"] = json!(1);
        config["rustAllocationOperationId"] = json!(operation);
        config["startupTimings"]["requestedAt"] = json!(now);
        sqlx::query("UPDATE api_cloud_workspaces SET state='provisioning',status_code='allocating',runtime_state='offline',runtime_boot_token_hash=$2,runtime_boot_token_expires_at=$3,request_config=$4,revision=revision+1,updated_at=GREATEST(updated_at+1,$5),next_action_at=$5 WHERE workspace_id=$1")
            .bind(id).bind(hash(&boot)).bind(now+BOOT_TTL).bind(config).bind(now).execute(&mut *tx).await?;
        let job = jobs::enqueue(
            &mut tx,
            &format!("allocation:{operation}"),
            JOB_KIND,
            id,
            generation,
            &json!({"operationId":operation}),
            now,
        )
        .await?;
        sqlx::query("UPDATE zuse_rust_jobs SET max_attempts=120 WHERE job_id=$1")
            .bind(job)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(operation)
    }

    /// One durable stage. The worker calls again after next_action_at; no client viewer is needed.
    pub async fn allocate_once(
        &self,
        provider: &dyn AllocationProvider,
        now: i64,
    ) -> Result<bool, ApiError> {
        let Some(job) = jobs::claim(&self.pool, JOB_KIND, "workspace", now, 120_000).await? else {
            return Ok(false);
        };
        let result = self.advance_allocation(provider, &job, now).await;
        if let Err(error) = &result {
            jobs::retry(&self.pool, &job, clock_ms(), error.code).await?;
            // Publish actionable errors only while this attempt still owns pending startup.
            // A late uncertain response must not overwrite an already-online guest or deletion.
            sqlx::query("UPDATE api_cloud_workspaces w SET status_code=$3,revision=revision+1,updated_at=GREATEST(updated_at+1,$4) WHERE workspace_id=$1 AND desired_state='ready' AND state IN ('queued','provisioning') AND runtime_state<>'online' AND request_config->>'rustAllocationOperationId'=$5 AND EXISTS(SELECT 1 FROM zuse_rust_control_owners o WHERE o.resource_kind='workspace' AND o.resource_id=w.workspace_id AND o.backend='rust' AND o.generation=$2) AND EXISTS(SELECT 1 FROM zuse_rust_jobs j WHERE j.job_id=$6 AND j.state IN ('pending','failed') AND j.attempts=$7 AND j.last_error_code=$3)")
                .bind(&job.resource_id).bind(job.generation).bind(error.code).bind(clock_ms()).bind(job.payload["operationId"].as_str()).bind(&job.id).bind(job.attempts).execute(&self.pool).await?;
        }
        result.map(|_| true)
    }
    async fn allocation_state(&self, job: &jobs::Job) -> Result<(Value, Value), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (workspace, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3)")
            .bind(&job.id).bind(&job.lease_token).bind(clock_ms()).fetch_one(&mut *tx).await?;
        if !active
            || generation != job.generation
            || blocked(&workspace)
            || workspace["desired_state"] != "ready"
        {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        let operation:Value=sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3")
            .bind(job.payload["operationId"].as_str()).bind(&job.resource_id).bind(job.generation).fetch_one(&mut *tx).await?;
        if workspace["request_config"]["rustAllocationOperationId"] != operation["operation_id"] {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        tx.commit().await?;
        Ok((workspace, operation))
    }
    async fn advance_allocation(
        &self,
        provider: &dyn AllocationProvider,
        job: &jobs::Job,
        now: i64,
    ) -> Result<(), ApiError> {
        let (workspace, operation) = self.allocation_state(job).await?;
        if operation["phase"] == "ready" {
            jobs::finish(&self.pool, job, "workspace", clock_ms()).await?;
            return Ok(());
        }
        if workspace["runtime_state"] == "online"
            && storage_enrolled(&workspace)
            && workspace["request_config"]["rustCompletedLaunchCommandId"].is_string()
        {
            self.save_allocation(job, "ready", None, true).await?;
            return Ok(());
        }
        if operation["boot_expires_at"].as_i64().unwrap_or(0) <= now {
            return Err(ApiError::conflict("cloud_allocation_boot_expired"));
        }
        let name = required(&operation, "machine_name")?;
        let machine_id = operation["machine_id"].as_str();
        match machine_id {
            None => {
                self.save_allocation(job, "creating", None, false).await?;
                let machine = match provider.find(name).await.map_err(provider_error)? {
                    Some(machine) => machine,
                    None => provider
                        .create(required(&operation, "snapshot_id")?, name)
                        .await
                        .map_err(provider_error)?,
                };
                if machine.name != name || machine.id.is_empty() {
                    return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
                }
                // Keep the provider receipt even if a concurrent delete/handoff fenced this worker.
                sqlx::query("UPDATE zuse_rust_allocations SET machine_id=COALESCE(machine_id,$2),updated_at=$3 WHERE operation_id=$1 AND (machine_id IS NULL OR machine_id=$2)")
                .bind(required(&operation,"operation_id")?).bind(&machine.id).bind(clock_ms()).execute(&self.pool).await?;
                self.save_allocation(job, "allocated", Some(&machine.id), false)
                    .await?;
            }
            Some(id) => {
                let machine = provider
                    .find(id)
                    .await
                    .map_err(provider_error)?
                    .ok_or_else(|| ApiError::conflict("cloud_allocation_machine_missing"))?;
                if machine.id != id || machine.name != name {
                    return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
                }
                if machine.state != MachineState::Running {
                    return Err(ApiError::unavailable("cloud_allocation_machine_starting"));
                }
                let boot = self
                    .master
                    .open_api(
                        &boot_context(
                            &job.resource_id,
                            required(&operation, "operation_id")?,
                            job.generation,
                        ),
                        required(&operation, "sealed_boot_token")?,
                    )
                    .map_err(|_| ApiError::unavailable("cloud_boot_grant_unavailable"))?;
                let boot = Zeroizing::new(
                    String::from_utf8(boot.to_vec())
                        .map_err(|_| ApiError::unavailable("cloud_boot_grant_unavailable"))?,
                );
                // Commit authorization before effects. Retries use exactly this launch and machine.
                self.save_allocation(job, "launching", Some(id), false)
                    .await?;
                provider
                    .launch(
                        id,
                        &RuntimeLaunch {
                            operation_id: required(&operation, "operation_id")?.into(),
                            workspace_id: job.resource_id.clone(),
                            generation: job.generation,
                            incarnation: required(&operation, "storage_incarnation_id")?.into(),
                            api_url: self.signer.issuer(),
                            boot_token: boot,
                        },
                    )
                    .await
                    .map_err(provider_error)?;
            }
        }
        self.defer_allocation(job).await
    }
    async fn save_allocation(
        &self,
        job: &jobs::Job,
        phase: &str,
        machine: Option<&str>,
        complete: bool,
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        let now = clock_ms();
        let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3)").bind(&job.id).bind(&job.lease_token).bind(now).fetch_one(&mut *tx).await?;
        if !live
            || generation != job.generation
            || blocked(&row)
            || row["desired_state"] != "ready"
            || row["request_config"]["rustAllocationOperationId"] != job.payload["operationId"]
        {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        let changed=sqlx::query("UPDATE zuse_rust_allocations SET phase=$2,sealed_boot_token=CASE WHEN $3 THEN NULL ELSE sealed_boot_token END,updated_at=$4 WHERE operation_id=$1 AND workspace_id=$5 AND generation=$6")
            .bind(job.payload["operationId"].as_str()).bind(phase).bind(complete).bind(now).bind(&job.resource_id).bind(job.generation).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        if let Some(machine) = machine {
            if row["provider_sandbox_id"]
                .as_str()
                .is_some_and(|prior| prior != machine)
            {
                return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
            }
            let mut config = row["request_config"].clone();
            if config["startupTimings"]["allocatedAt"].is_null() {
                config["startupTimings"]["allocatedAt"] = json!(now);
            }
            sqlx::query("UPDATE api_cloud_workspaces SET provider_sandbox_id=$2,status_code=CASE WHEN runtime_state='online' THEN status_code ELSE $3 END,request_config=$4,revision=revision+1,updated_at=GREATEST(updated_at+1,$5) WHERE workspace_id=$1")
                .bind(&job.resource_id).bind(machine).bind(if phase=="launching"{"starting-runtime"}else{"preparing-runtime"}).bind(config).bind(now).execute(&mut *tx).await?;
        }
        if complete {
            sqlx::query("UPDATE zuse_rust_jobs SET state='completed',lease_token=NULL,lease_expires_at=NULL,updated_at=$3 WHERE job_id=$1 AND lease_token=$2").bind(&job.id).bind(&job.lease_token).bind(now).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
    async fn defer_allocation(&self, job: &jobs::Job) -> Result<(), ApiError> {
        let now = clock_ms();
        sqlx::query("UPDATE zuse_rust_jobs SET state='pending',lease_token=NULL,lease_expires_at=NULL,due_at=$3,updated_at=$4 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$4")
            .bind(&job.id).bind(&job.lease_token).bind(now+2000).bind(now).execute(&self.pool).await?;
        Ok(())
    }
}
fn clock_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
