//! Initial prepared-image allocation. Retained/recovery storage never enters this path.
use super::*;
use tracing::Instrument;
pub mod workers;
use crate::{
    jobs,
    providers::{Machine, MachineState, ProviderError, boxd::Boxd},
};
use async_trait::async_trait;
use zeroize::Zeroizing;

pub(super) mod initial_boot;
mod preparation_hold;
pub mod rejection;
pub(crate) mod timings;

pub const MIGRATION: &str = include_str!("../../migrations/0004_allocations.sql");
pub const JOB_KIND: &str = "workspace-allocate";
const BOOT_TTL: i64 = 30 * 60 * 1000;

/// In-process continuation of one positive, already-persisted create receipt.
/// Never a new claim, replacement allocation or authority carried across stages.
struct AllocationContinuation {
    operation: String,
    machine: String,
    snapshot: String,
    incarnation: String,
}

/// Only identity from this worker attempt, captured before unlocked provider calls.
/// Diagnostic persistence never grants an effect or adopts a changed source.
struct AllocationDiagnosticContext {
    operation: String,
    snapshot: String,
    incarnation: String,
    name: String,
    machine: Option<String>,
}

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
    /// Latest actual provider-call measurements; legacy adapters may omit them.
    fn startup_observations(&self) -> Value {
        json!({})
    }
    fn provider_id(&self) -> &'static str {
        "boxd"
    }
    async fn find(&self, name: &str) -> Result<Option<Machine>, ProviderError>;
    async fn inspect_id(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.find(id).await
    }
    async fn runtime_ready(&self, _id: &str) -> Result<bool, ProviderError> {
        Ok(true)
    }
    async fn create(&self, snapshot: &str, name: &str) -> Result<Machine, ProviderError>;
    async fn create_operation(
        &self,
        snapshot: &str,
        name: &str,
        _operation: &str,
    ) -> Result<Machine, ProviderError> {
        self.create(snapshot, name).await
    }
    async fn bootstrap_initial_runtime(
        &self,
        _machine: &str,
        _artifact: &str,
        _name: &str,
        _request: &crate::managed_bases::bootstrap::Request,
    ) -> Result<Option<Value>, ProviderError> {
        Err(ProviderError::Rejected)
    }
    async fn bootstrap_initial_runtime_with_source_receipt(
        &self,
        machine: &str,
        artifact: &str,
        name: &str,
        request: &crate::managed_bases::bootstrap::Request,
        _receipt: Option<&crate::providers::contract::VerifiedCreateSource>,
    ) -> Result<Option<Value>, ProviderError> {
        self.bootstrap_initial_runtime(machine, artifact, name, request)
            .await
    }
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
    async fn bootstrap_initial_runtime(
        &self,
        machine: &str,
        artifact: &str,
        name: &str,
        request: &crate::managed_bases::bootstrap::Request,
    ) -> Result<Option<Value>, ProviderError> {
        if request.source_machine_id != machine
            || request.provider != zuse_protocol::cloud_computers::ProviderId::Boxd
            || !request.valid()
        {
            return Err(ProviderError::Rejected);
        }
        self.verify_initial_snapshot_source(machine, artifact, name)
            .await?;
        let result = self.exec(machine, &request.command()?).await?;
        if result.exit_code != 0 || result.stdout.len() > 65536 {
            return Err(ProviderError::Rejected);
        }
        request.response(
            &serde_json::from_slice(&result.stdout).map_err(|_| ProviderError::InvalidResponse)?,
        )
    }
    async fn launch(&self, machine: &str, launch: &RuntimeLaunch) -> Result<(), ProviderError> {
        let (staged, command) = launch_command(launch);
        self.upload(machine, &staged, launch.boot_token.as_bytes())
            .await?;
        let result = self.exec(machine, &command).await?;
        if result.exit_code != 0 {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
}
pub(super) fn launch_command(launch: &RuntimeLaunch) -> (String, String) {
    let staged = format!("/tmp/zuse-rust-boot-{}", launch.operation_id);
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
    (staged, command)
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
        let mut tx = self.pool.begin().await?;
        let operation = self.queue_allocation_tx(&mut tx, id, snapshot, now).await?;
        tx.commit().await?;
        super::lifecycle::wakeup::send();
        Ok(operation)
    }
    pub(crate) async fn queue_allocation_tx(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        id: &str,
        snapshot: &str,
        now: i64,
    ) -> Result<String, ApiError> {
        if snapshot.is_empty() || snapshot.len() > 512 || snapshot.chars().any(char::is_control) {
            return Err(ApiError::bad_request("invalid_prepared_image"));
        }
        let (row, generation, epoch) = locked_workspace(tx, id).await?;
        if (row["provider"] != "boxd"
            && !(row["request_config"]["rustProviderControllerVersion"] == 1
                && zuse_protocol::cloud_computers::ProviderId::ALL
                    .iter()
                    .any(|p| row["provider"] == p.as_str())))
            || !super::scope::native_organization_task(&row)
            || row["request_config"]["providerConnectionId"].is_string()
            || blocked(&row)
            || row["desired_state"] != "ready"
        {
            return Err(ApiError::conflict("cloud_allocation_unavailable"));
        }
        let prior: Option<(String,String)> = sqlx::query_as("SELECT operation_id,snapshot_id FROM zuse_rust_allocations WHERE workspace_id=$1 AND generation=$2 FOR UPDATE")
            .bind(id).bind(generation).fetch_optional(&mut **tx).await?;
        if let Some((operation, image)) = prior {
            if image != snapshot {
                return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
            }
            return Ok(operation);
        }
        crate::images::deletion::snapshot_read_guard(
            tx,
            required(&row, "account_id")?,
            Some(snapshot),
        )
        .await?;
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
            .bind(id).bind(now).fetch_one(&mut **tx).await?;
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
            .bind(&operation).bind(id).bind(generation).bind(snapshot).bind(name).bind(&incarnation).bind(sealed).bind(now+BOOT_TTL).bind(now).execute(&mut **tx).await?;
        config["runtimeGeneration"] = json!(generation);
        config["gatewayEpoch"] = json!(epoch);
        config["runtimeRpcProtocol"] = json!(zuse_protocol::cloud_gateway::RUST_RPC);
        config["rustRuntimeStorage"] =
            json!({"format":"zuse-rust-engine-v1","storageIncarnationId":incarnation});
        config["rustSessionCommandVersion"] = json!(1);
        config["rustAllocationOperationId"] = json!(operation);
        config["startupTimings"]["requestedAt"] = json!(now);
        sqlx::query("UPDATE api_cloud_workspaces SET state='provisioning',status_code='allocating',runtime_state='offline',runtime_boot_token_hash=$2,runtime_boot_token_expires_at=$3,request_config=$4,revision=revision+1,updated_at=GREATEST(updated_at+1,$5),next_action_at=$5 WHERE workspace_id=$1")
            .bind(id).bind(hash(&boot)).bind(now+BOOT_TTL).bind(config).bind(now).execute(&mut **tx).await?;
        let job = jobs::enqueue(
            tx,
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
            .execute(&mut **tx)
            .await?;
        Ok(operation)
    }

    /// One durable stage, plus at most one immediate same-lease continuation
    /// after a positive create receipt. Other worker roles then get their round.
    pub async fn allocate_once(
        &self,
        provider: &dyn AllocationProvider,
        now: i64,
    ) -> Result<bool, ApiError> {
        let Some(job) = jobs::claim(&self.pool, JOB_KIND, "workspace", now, 120_000).await? else {
            return Ok(false);
        };
        self.allocation_claimed(provider, &job, now).await?;
        Ok(true)
    }
    pub async fn allocate_registry_once(
        &self,
        registry: &crate::providers::registry::Registry,
        now: i64,
    ) -> Result<bool, ApiError> {
        let Some(job) = jobs::claim(&self.pool, JOB_KIND, "workspace", now, 120000).await? else {
            return Ok(false);
        };
        let provider = self.provider_for_job(registry, &job).await?;
        self.allocation_claimed(&provider, &job, now).await?;
        Ok(true)
    }
    async fn allocation_claimed(
        &self,
        provider: &dyn AllocationProvider,
        job: &jobs::Job,
        now: i64,
    ) -> Result<(), ApiError> {
        tracing::info!(workspace=%job.resource_id, operation=job.payload["operationId"].as_str().unwrap_or(""),
            attempt=job.attempts, claimed_at=now, "Cloud allocation attempt started");
        let mut diagnostic_context = None;
        let allocation = async {
            if let Some(next) = self
                .advance_allocation(provider, job, now, None, &mut diagnostic_context)
                .await?
            {
                // No loop: preserve worker fairness after these two stages.
                // A crash still leaves this exact durable lease/operation for recovery.
                self.advance_allocation(
                    provider,
                    job,
                    clock_ms(),
                    Some(&next),
                    &mut diagnostic_context,
                )
                .await?;
            }
            Ok::<(), ApiError>(())
        };
        let span = tracing::info_span!("cloud_allocation", workspace=%job.resource_id,
            operation=job.payload["operationId"].as_str().unwrap_or(""), attempt=job.attempts);
        let (result, ()) = tokio::join!(
            allocation.instrument(span.clone()),
            self.prepare_allocation_access(job).instrument(span)
        );
        if let Err(error) = &result {
            // Keep the original failure even if telemetry storage is unavailable.
            // Save before retry/hold releases this exact attempt's lease.
            if let Some(context) = diagnostic_context.as_ref() {
                let _ = self
                    .persist_allocation_failure_observations(
                        job,
                        context,
                        &provider.startup_observations(),
                    )
                    .await;
            }
            if error.code == preparation_hold::ACTIVE && self.hold_preparing_allocation(job).await?
            {
                return Ok(());
            }
            if error.code == "cloud_provider_rejected"
                && self.stop_rejected_allocation(job, clock_ms()).await?
            {
                return result;
            }
            // Successful not-ready observations do not consume the provider-failure
            // backoff exponent. Physical delivery attempts remain monotonic.
            let mut failure_job = job.clone();
            let observations = job.payload["readinessPollCount"]
                .as_u64()
                .unwrap_or(0)
                .min(job.attempts.max(0) as u64) as i32;
            failure_job.attempts = job.attempts - observations;
            jobs::retry(&self.pool, &failure_job, clock_ms(), error.code).await?;
            // Publish actionable errors only while this attempt still owns pending startup.
            // A late uncertain response must not overwrite an already-online guest or deletion.
            sqlx::query("UPDATE api_cloud_workspaces w SET status_code=$3,revision=revision+1,updated_at=GREATEST(updated_at+1,$4) WHERE workspace_id=$1 AND desired_state='ready' AND state IN ('queued','provisioning') AND runtime_state<>'online' AND COALESCE(w.request_config#>>'{rustRuntimeRepair,phase}','ready')='ready' AND (w.request_config#>>'{rustRecreation,operationId}' IS NULL OR w.request_config#>>'{rustRecreation,phase}' IN ('ready','cancelled')) AND request_config->>'rustAllocationOperationId'=$5 AND EXISTS(SELECT 1 FROM zuse_rust_control_owners o WHERE o.resource_kind='workspace' AND o.resource_id=w.workspace_id AND o.backend='rust' AND o.generation=$2) AND EXISTS(SELECT 1 FROM zuse_rust_jobs j WHERE j.job_id=$6 AND j.state IN ('pending','failed') AND j.attempts=$7 AND j.last_error_code=$3)")
                .bind(&job.resource_id).bind(job.generation).bind(error.code).bind(clock_ms()).bind(job.payload["operationId"].as_str()).bind(&job.id).bind(job.attempts).execute(&self.pool).await?;
        }
        result
    }
    #[tracing::instrument(name="cloud_allocation_stage", skip_all, fields(workspace=%job.resource_id, operation="allocation_state"))]
    async fn allocation_state(
        &self,
        job: &jobs::Job,
    ) -> Result<(Value, Value, Result<(), ApiError>), ApiError> {
        let mut tx = self.pool.begin().await?;
        let (workspace,generation,operation,active,capacity_observation) =
            if let Some(snapshot)=self.allocation_snapshot(&mut tx,job).await? {
                (snapshot.workspace,snapshot.generation,snapshot.operation,snapshot.live,Some(snapshot.capacity))
            } else {
                let (workspace,generation,_)=locked_workspace(&mut tx,&job.resource_id).await?;
                let active:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3)")
                    .bind(&job.id).bind(&job.lease_token).bind(clock_ms()).fetch_one(&mut *tx).await?;
                let operation:Value=sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3")
                    .bind(job.payload["operationId"].as_str()).bind(&job.resource_id).bind(job.generation).fetch_one(&mut *tx).await?;
                (workspace,generation,operation,active,None)
            };
        let held=super::repair::leases_blocked(&workspace)||super::recreation::leases_blocked(&workspace);
        if !active || generation!=job.generation || blocked(&workspace)
            || (workspace["desired_state"]!="ready" && !held) {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        if workspace["request_config"]["rustAllocationOperationId"] != operation["operation_id"] {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        let capacity = if held {
            Ok(())
        } else if workspace["runtime_state"] == "online"
            && storage_enrolled(&workspace)
            && (workspace["request_config"]["rustCompletedLaunchCommandId"].is_string()
                || Self::early_agent_ready(&workspace))
        {
            Ok(())
        } else {
            self.allocation_snapshot_capacity(&mut tx,&workspace,capacity_observation.as_ref(),clock_ms())
            .await
        };
        tx.commit().await?;
        Ok((workspace, operation, capacity))
    }
    async fn reconcile_denied_allocation(
        &self,
        provider: &dyn AllocationProvider,
        workspace: &Value,
        operation: &Value,
    ) -> Result<(), ApiError> {
        if operation["machine_id"].is_string() || operation["phase"] != "creating" {
            return Ok(());
        }
        let name = required(operation, "machine_name")?;
        let known = self.allocation_created_identity(
            required(workspace, "workspace_id")?,
            required(operation, "operation_id")?,
            name,
        )
        .await?;
        let observed = if let Some(id) = known {
            provider.inspect_id(&id).await.map_err(provider_error)?
        } else {
            provider.find(name).await.map_err(provider_error)?
        };
        if let Some(machine) = observed {
            if machine.id.is_empty() || machine.name != name {
                return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
            }
            self.record_allocation_result(
                required(operation, "operation_id")?,
                &Ok(machine.clone()),
            )
            .await?;
            // Record the original outcome even after authority withdrawal; never
            // create, wake or launch another resource to resolve uncertainty.
            sqlx::query("UPDATE zuse_rust_allocations SET machine_id=COALESCE(machine_id,$2),updated_at=$3 WHERE operation_id=$1 AND workspace_id=$4 AND (machine_id IS NULL OR machine_id=$2)")
                .bind(required(operation,"operation_id")?).bind(&machine.id).bind(clock_ms()).bind(required(workspace,"workspace_id")?).execute(&self.pool).await?;
        }
        Ok(())
    }
    async fn advance_allocation(
        &self,
        provider: &dyn AllocationProvider,
        job: &jobs::Job,
        now: i64,
        continuation: Option<&AllocationContinuation>,
        diagnostic_context: &mut Option<AllocationDiagnosticContext>,
    ) -> Result<Option<AllocationContinuation>, ApiError> {
        let _effects = effects::Guard::acquire(&job.resource_id)?;
        let (mut workspace, mut operation, capacity) = self.allocation_state(job).await?;
        if let Some(expected) = continuation {
            if operation["operation_id"] != expected.operation
                || operation["machine_id"] != expected.machine
                || workspace["provider_sandbox_id"] != expected.machine
                || operation["snapshot_id"] != expected.snapshot
                || operation["storage_incarnation_id"] != expected.incarnation
            {
                return Err(ApiError::conflict("cloud_allocation_continuation_fenced"));
            }
        }
        // The original allocation remains durable while a retained runtime is
        // repaired/recreated. Never dispatch even an inspection into that lane.
        if super::repair::leases_blocked(&workspace)
            || super::recreation::leases_blocked(&workspace)
        {
            return self.hold_allocation(job).await.map(|_| None);
        }
        if workspace["provider"] != provider.provider_id() {
            return Err(ApiError::conflict("cloud_provider_identity_mismatch"));
        }
        // Canonical completion observes an already authenticated guest; it
        // grants no compute and must not replay a now-expired original boot.
        let canonical = workspace["runtime_state"] == "online"
            && storage_enrolled(&workspace)
            && (workspace["request_config"]["rustCompletedLaunchCommandId"].is_string()
                || Self::early_agent_ready(&workspace));
        if (operation["phase"] == "ready" || canonical)
            && (operation["machine_id"] != workspace["provider_sandbox_id"]
                || operation["storage_incarnation_id"]
                    != workspace["request_config"]["rustRuntimeStorage"]["storageIncarnationId"])
        {
            return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
        }
        if operation["phase"] == "ready" {
            jobs::finish(&self.pool, job, "workspace", clock_ms()).await?;
            return Ok(None);
        }
        if canonical {
            self.save_allocation(job, "ready", None, true).await?;
            return Ok(None);
        }
        // The enrolled guest now owns preparation. Re-uploading its original
        // boot grant cannot repair a failed checkout and hides the stage error.
        if preparation_hold::active(&workspace, &operation)
            && self.hold_preparing_allocation(job).await?
        {
            return Ok(None);
        }
        if let Err(error) = capacity {
            self.reconcile_denied_allocation(provider, &workspace, &operation)
                .await?;
            return Err(error);
        }
        if let Err(error) = self
            .allocation_scope_access(&workspace, &operation, now)
            .await
        {
            self.reconcile_denied_allocation(provider, &workspace, &operation)
                .await?;
            return Err(error);
        }
        if self
            .resolve_project_allocation_source(job, &mut workspace, &mut operation)
            .await?
            || self
                .project_cache_allocation_source(&mut workspace, &mut operation)
                .await?
        {
            return self.hold_allocation(job).await.map(|_| None);
        }
        if operation["boot_expires_at"].as_i64().unwrap_or(0) <= now {
            return Err(ApiError::conflict("cloud_allocation_boot_expired"));
        }
        let name = required(&operation, "machine_name")?;
        let machine_id = operation["machine_id"].as_str();
        *diagnostic_context = Some(AllocationDiagnosticContext {
            operation: required(&operation, "operation_id")?.into(),
            snapshot: required(&operation, "snapshot_id")?.into(),
            incarnation: required(&operation, "storage_incarnation_id")?.into(),
            name: name.into(),
            machine: machine_id.map(str::to_owned),
        });
        match machine_id {
            None => {
                self.save_allocation(job, "creating", None, false).await?;
                let initial_dispatch = operation["phase"] == "planned"
                    && job.attempts == 1
                    && workspace["request_config"]["rustFastStartVersion"] == 1;
                let known = if initial_dispatch {
                    // begin_allocation_dispatch still locks and validates the
                    // original journal before permitting a POST. A planned
                    // first attempt has no earlier provider effect to adopt.
                    None
                } else {
                    self.allocation_created_identity(
                        &job.resource_id, required(&operation, "operation_id")?, name,
                    ).await?
                };
                let found = if let Some(known) = known {
                    Some(
                        provider
                            .inspect_id(&known)
                            .await
                            .map_err(provider_error)?
                            .ok_or_else(|| {
                                ApiError::unavailable("cloud_provider_allocation_outcome_unknown")
                            })?,
                    )
                } else if initial_dispatch {
                    // This original operation has never dispatched a create.
                    // Every recovery attempt still reconciles the exact name.
                    None
                } else {
                    provider.find(name).await.map_err(provider_error)?
                };
                let mut direct_positive_create = false;
                let machine = match found {
                    Some(machine) => machine,
                    None => {
                        self.begin_allocation_dispatch(job, &operation).await?;
                        let result = provider
                            .create_operation(
                                required(&operation, "snapshot_id")?,
                                name,
                                required(&operation, "operation_id")?,
                            )
                            .await;
                        direct_positive_create = result.is_ok();
                        self.record_allocation_result(
                            required(&operation, "operation_id")?,
                            &result,
                        )
                        .await?;
                        result.map_err(provider_error)?
                    }
                };
                if machine.name != name || machine.id.is_empty() {
                    return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
                }
                if !direct_positive_create {
                    // A successful POST already persisted this exact result
                    // above. Reconciled identities still need their receipt.
                    self.record_allocation_result(
                        required(&operation, "operation_id")?,
                        &Ok(machine.clone()),
                    )
                    .await?;
                }
                // Keep the provider receipt even if a concurrent delete/handoff fenced this worker.
                sqlx::query("UPDATE zuse_rust_allocations SET machine_id=COALESCE(machine_id,$2),updated_at=$3 WHERE operation_id=$1 AND (machine_id IS NULL OR machine_id=$2)")
                .bind(required(&operation,"operation_id")?).bind(&machine.id).bind(clock_ms()).execute(&self.pool).await?;
                self.save_allocation_observed_with_positive_create(
                    job,
                    "allocated",
                    Some(&machine.id),
                    false,
                    &provider.startup_observations(),
                    (direct_positive_create && provider.provider_id() == "boat")
                        .then_some(&operation),
                )
                .await?;
                // A positive identity is enough for one immediate observation.
                // Create may still say Starting while the original machine is
                // already ready by inspection. The continuation rechecks every
                // lease/scope/source fence and inspects this ID; only Running
                // plus runtime_ready below permits credential/launch effects.
                return Ok(Some(AllocationContinuation {
                    operation: required(&operation, "operation_id")?.into(),
                    machine: machine.id,
                    snapshot: required(&operation, "snapshot_id")?.into(),
                    incarnation: required(&operation, "storage_incarnation_id")?.into(),
                }));
            }
            Some(id) => {
                let machine = provider
                    .inspect_id(id)
                    .await
                    .map_err(provider_error)?
                    .ok_or_else(|| ApiError::conflict("cloud_allocation_machine_missing"))?;
                if machine.id != id || machine.name != name {
                    return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
                }
                if machine.state == MachineState::Starting {
                    self.defer_starting_allocation(
                        job,
                        &operation,
                        id,
                        &provider.startup_observations(),
                    )
                    .await?;
                    return Ok(None);
                }
                if machine.state != MachineState::Running {
                    // Stopped/unknown states are not successful startup observations.
                    // Preserve generic error retry/backoff for unexpected state.
                    return Err(ApiError::unavailable("cloud_allocation_machine_starting"));
                }
                // A provider can expose commands before its final filesystem is
                // materialized. Never upload a boot credential in that window.
                // save_allocation below rechecks the lease/generation/source and
                // desired state AFTER this unlocked provider observation.
                if !provider.runtime_ready(id).await.map_err(provider_error)? {
                    self.defer_starting_allocation(
                        job,
                        &operation,
                        id,
                        &provider.startup_observations(),
                    )
                    .await?;
                    return Ok(None);
                }
                if !self
                    .initial_allocation_boot(provider, job, &workspace, &operation, id)
                    .await?
                {
                    return self.hold_allocation(job).await.map(|_| None);
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
                let prepared_fence = workspace["request_config"]["rustFastStartVersion"] == 1
                    && workspace["provider"] == "boxd"
                    && workspace["request_config"]["agent"] == "codex"
                    && crate::managed_bases::bootstrap::InitialRuntimeBootstrap::from_config(
                        &workspace["request_config"],
                    )?.is_some();
                if !prepared_fence {
                    self.save_allocation_observed(
                        job, "launching", Some(id), false, &provider.startup_observations(),
                    ).await?;
                }
                // Prepared dispatch has just committed the same checks and
                // launching transition in initial_boot_allocation_fence, after
                // scope access. Provider timings are saved by the existing
                // observed job transition below, including on failure.
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
        // Successful creation is a completed dependency, not a reason to wait
        // two seconds before launching the guest. Retain bounded readiness
        // retries after launch while the guest enrolls.
        self.defer_allocation_observed(
            job,
            if machine_id.is_none() { 0 } else { 2000 },
            &provider.startup_observations(),
        )
        .await?;
        Ok(None)
    }
    /// Expected readiness is an observation, not a failed provider operation.
    /// Recheck the original receipt after unlocked HTTP and defer only this lease.
    async fn defer_starting_allocation(
        &self,
        job: &jobs::Job,
        expected: &Value,
        machine: &str,
        observations: &Value,
    ) -> Result<(), ApiError> {
        let (current, operation, capacity) = self.allocation_state(job).await?;
        capacity?;
        self.allocation_scope_access(&current, &operation, clock_ms())
            .await?;
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        let now = clock_ms();
        let operation: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3 FOR UPDATE")
            .bind(job.payload["operationId"].as_str()).bind(&job.resource_id).bind(job.generation).fetch_optional(&mut *tx).await?;
        let operation = operation.ok_or_else(|| ApiError::conflict("cloud_allocation_fenced"))?;
        if generation != job.generation
            || row["request_config"]["runtimeGeneration"] != job.generation
            || row["runtime_state"] == "online"
            || blocked(&row)
            || row["desired_state"] != "ready"
            || row["request_config"]["rustAllocationOperationId"] != expected["operation_id"]
            || row["provider_sandbox_id"] != machine
            || operation["machine_id"] != machine
            || operation["operation_id"] != expected["operation_id"]
            || operation["snapshot_id"] != expected["snapshot_id"]
            || operation["boot_expires_at"] != expected["boot_expires_at"]
            || operation["phase"] != expected["phase"]
            || operation["machine_name"] != expected["machine_name"]
            || operation["storage_incarnation_id"] != expected["storage_incarnation_id"]
            || row["request_config"]["rustRuntimeStorage"]["storageIncarnationId"]
                != operation["storage_incarnation_id"]
            || super::repair::leases_blocked(&row)
            || super::recreation::leases_blocked(&row)
        {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        if preparation_hold::active(&row, &operation) {
            return Err(ApiError::conflict(preparation_hold::ACTIVE));
        }
        let expires = operation["boot_expires_at"].as_i64().unwrap_or(0);
        if expires <= now {
            return Err(ApiError::conflict("cloud_allocation_boot_expired"));
        }
        self.allocation_capacity(&mut tx, &row, now).await?;
        // Keep the original 120 failure deliveries while allowing bounded 2s
        // observations until the original 30min boot deadline. Never reset attempts.
        let ceiling = 120 + BOOT_TTL / 2000 + 1;
        let changed = sqlx::query("UPDATE zuse_rust_jobs SET state='pending',max_attempts=LEAST($6,max_attempts+1),payload=jsonb_set(payload,'{readinessPollCount}',to_jsonb(COALESCE((payload->>'readinessPollCount')::bigint,0)+1)),last_error_code=NULL,lease_token=NULL,lease_expires_at=NULL,due_at=$3,updated_at=$4 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$4 AND resource_generation=$5")
            .bind(&job.id).bind(&job.lease_token).bind((now+2000).min(expires)).bind(now).bind(job.generation).bind(ceiling as i32).execute(&mut *tx).await?;
        if changed.rows_affected() != 1 {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        let retained = self.staging_candidate.as_ref().is_some_and(|pin| {
            pin.retained_workspaces
                .iter()
                .any(|entry| entry.workspace_id == job.resource_id)
        });
        if !retained && observations.as_object().is_some_and(|v| !v.is_empty()) {
            let mut config = row["request_config"].clone();
            timings::merge(&mut config, observations);
            sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2 WHERE workspace_id=$1")
                .bind(&job.resource_id)
                .bind(config)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    async fn persist_allocation_failure_observations(
        &self,
        job: &jobs::Job,
        expected: &AllocationDiagnosticContext,
        observations: &Value,
    ) -> Result<(), ApiError> {
        let safe = timings::safe_failure_observations(observations);
        // Retained staging exemptions keep their original config byte-for-byte;
        // they are not fresh candidate allocation attempts.
        if self.staging_candidate.as_ref().is_some_and(|pin| {
            pin.retained_workspaces
                .iter()
                .any(|binding| binding.workspace_id == job.resource_id)
        }) {
            return Ok(());
        }
        if safe.as_object().is_none_or(|phases| phases.is_empty()) {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
        let now = clock_ms();
        let live: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3 AND resource_id=$4 AND resource_generation=$5)")
            .bind(&job.id).bind(&job.lease_token).bind(now).bind(&job.resource_id).bind(job.generation).fetch_one(&mut *tx).await?;
        let config = &row["request_config"];
        if !live
            || generation != job.generation
            || blocked(&row)
            || row["desired_state"] != "ready"
            || row["runtime_state"] == "online"
            || !matches!(row["state"].as_str(), Some("queued" | "provisioning"))
            || config["runtimeGeneration"] != job.generation
            || config["rustProviderControllerVersion"] != 1
            || config["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC
            || config["rustPreparationVersion"] != 1
            || config["rustAllocationOperationId"] != expected.operation
            || job.payload["operationId"] != expected.operation
            || config["rustRuntimeStorage"]["storageIncarnationId"] != expected.incarnation
            || super::repair::leases_blocked(&row)
            || super::recreation::leases_blocked(&row)
        {
            return Ok(());
        }
        let operation: Option<Value> = sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3")
            .bind(&expected.operation).bind(&job.resource_id).bind(job.generation).fetch_optional(&mut *tx).await?;
        let Some(operation) = operation else {
            return Ok(());
        };
        if operation["snapshot_id"] != expected.snapshot
            || operation["machine_name"] != expected.name
            || operation["storage_incarnation_id"] != expected.incarnation
            || operation["machine_id"].as_str() != expected.machine.as_deref()
            || row["provider_sandbox_id"].as_str() != expected.machine.as_deref()
            || !matches!(
                operation["phase"].as_str(),
                Some("creating" | "allocated" | "launching")
            )
        {
            return Ok(());
        }
        let mut config = config.clone();
        timings::merge(&mut config, &safe);
        sqlx::query("UPDATE api_cloud_workspaces SET request_config=$2,revision=revision+1,updated_at=GREATEST(updated_at+1,$3) WHERE workspace_id=$1")
            .bind(&job.resource_id).bind(config).bind(now).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    async fn save_allocation(
        &self,
        job: &jobs::Job,
        phase: &str,
        machine: Option<&str>,
        complete: bool,
    ) -> Result<(), ApiError> {
        self.save_allocation_observed(job, phase, machine, complete, &json!({}))
            .await
    }
    async fn save_allocation_observed(
        &self,
        job: &jobs::Job,
        phase: &str,
        machine: Option<&str>,
        complete: bool,
        observations: &Value,
    ) -> Result<(), ApiError> {
        self.save_allocation_observed_with_positive_create(
            job,
            phase,
            machine,
            complete,
            observations,
            None,
        )
        .await
    }
    #[tracing::instrument(name="cloud_allocation_stage", skip_all, fields(workspace=%job.resource_id, operation="save_allocation", phase=%phase))]
    async fn save_allocation_observed_with_positive_create(
        &self,
        job: &jobs::Job,
        phase: &str,
        machine: Option<&str>,
        complete: bool,
        observations: &Value,
        positive_create: Option<&Value>,
    ) -> Result<(), ApiError> {
        let mut tx = self.pool.begin().await?;
        let snapshot = if !complete && phase != "allocated" {
            self.allocation_snapshot(&mut tx, job).await?
        } else {
            None
        };
        let (row, generation, live, observed_operation, capacity_observation) =
            if let Some(snapshot) = snapshot {
                (snapshot.workspace, snapshot.generation, snapshot.live,
                    Some(snapshot.operation), Some(snapshot.capacity))
            } else {
                let (row, generation, _) = locked_workspace(&mut tx, &job.resource_id).await?;
                let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3)").bind(&job.id).bind(&job.lease_token).bind(clock_ms()).fetch_one(&mut *tx).await?;
                (row, generation, live, None, None)
            };
        let now = clock_ms();
        if !live
            || generation != job.generation
            || blocked(&row)
            || row["desired_state"] != "ready"
            || row["request_config"]["rustAllocationOperationId"] != job.payload["operationId"]
        {
            return Err(ApiError::conflict("cloud_allocation_fenced"));
        }
        if phase == "launching"
            && (super::repair::leases_blocked(&row) || super::recreation::leases_blocked(&row))
        {
            return Err(ApiError::conflict("cloud_allocation_lifecycle_in_progress"));
        }
        if phase == "launching" {
            // Enrollment may arrive while provider inspection is in flight.
            // Recheck under the current workspace lock before ANY boot upload.
            let operation: Value = if let Some(operation) = observed_operation { operation } else {
                sqlx::query_scalar("SELECT to_jsonb(a) FROM zuse_rust_allocations a WHERE operation_id=$1 AND workspace_id=$2 AND generation=$3")
                    .bind(job.payload["operationId"].as_str()).bind(&job.resource_id).bind(job.generation).fetch_one(&mut *tx).await?
            };
            if preparation_hold::active(&row, &operation) {
                return Err(ApiError::conflict(preparation_hold::ACTIVE));
            }
        }
        // Allocated is an observation/receipt after the external effect, not
        // authorization for another effect. Always retain that original result.
        if !complete && phase != "allocated" {
            self.allocation_snapshot_capacity(&mut tx, &row, capacity_observation.as_ref(), now).await?;
        }
        let operation: Option<Value> = sqlx::query_scalar("UPDATE zuse_rust_allocations SET phase=$2,sealed_boot_token=CASE WHEN $3 THEN NULL ELSE sealed_boot_token END,updated_at=$4 WHERE operation_id=$1 AND workspace_id=$5 AND generation=$6 RETURNING to_jsonb(zuse_rust_allocations)")
            .bind(job.payload["operationId"].as_str()).bind(phase).bind(complete).bind(now).bind(&job.resource_id).bind(job.generation).fetch_optional(&mut *tx).await?;
        let operation = operation.ok_or_else(|| ApiError::conflict("cloud_allocation_fenced"))?;
        if let Some(machine) = machine {
            if row["provider_sandbox_id"]
                .as_str()
                .is_some_and(|prior| prior != machine)
            {
                return Err(ApiError::conflict("cloud_allocation_identity_conflict"));
            }
            let mut config = row["request_config"].clone();
            if let Some(expected) = positive_create {
                // Only this direct successful POST branch can mint the marker.
                // The allocation, confirmation and marker commit under the same
                // fresh workspace/source/generation/lease transaction.
                if phase != "allocated"
                    || row["provider"] != "boat"
                    || operation["machine_id"] != machine
                    || operation["snapshot_id"] != expected["snapshot_id"]
                    || operation["machine_name"] != expected["machine_name"]
                    || operation["storage_incarnation_id"] != expected["storage_incarnation_id"]
                    || operation["boot_expires_at"].as_i64().unwrap_or(0) <= now
                    || super::repair::leases_blocked(&row)
                    || super::recreation::leases_blocked(&row)
                {
                    return Err(ApiError::conflict("cloud_initial_runtime_bootstrap_fenced"));
                }
                if let Some(request) = initial_boot::request(
                    &row,
                    &operation,
                    machine,
                    crate::managed_bases::bootstrap::Purpose::InitialAllocation,
                    required(&operation, "storage_incarnation_id")?,
                )? {
                    if config["runtimeGeneration"] != request.generation
                        || config["rustRuntimeStorage"]["storageIncarnationId"]
                            != request.storage_incarnation_id
                    {
                        return Err(ApiError::conflict("cloud_initial_runtime_bootstrap_fenced"));
                    }
                    let confirmed = sqlx::query("UPDATE zuse_rust_provider_creates SET outcome='confirmed',updated_at=$5 WHERE operation_id=$1 AND workspace_id=$2 AND machine_name=$3 AND machine_id=$4 AND outcome='confirmed'")
                        .bind(&request.operation_id).bind(&request.workspace_id).bind(required(&operation,"machine_name")?).bind(machine).bind(now).execute(&mut *tx).await?;
                    if confirmed.rows_affected() != 1 {
                        return Err(ApiError::conflict("cloud_initial_runtime_bootstrap_fenced"));
                    }
                    config["rustPositiveCreateReceipt"] = initial_boot::positive_create_source(
                        &request,
                        required(&operation, "snapshot_id")?,
                        required(&operation, "machine_name")?,
                    );
                }
            }
            timings::merge(&mut config, observations);
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
    async fn hold_allocation(&self, job: &jobs::Job) -> Result<(), ApiError> {
        let now = clock_ms();
        // Waiting for an explicitly owned repair/recreation is not a failed
        // provider attempt. Preserve its budget even during a long user review.
        sqlx::query("UPDATE zuse_rust_jobs SET state='pending',attempts=GREATEST(attempts-1,0),lease_token=NULL,lease_expires_at=NULL,due_at=$3,updated_at=$4 WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$4")
            .bind(&job.id).bind(&job.lease_token).bind(now+2000).bind(now).execute(&self.pool).await?;
        Ok(())
    }
    async fn defer_allocation_observed(
        &self,
        job: &jobs::Job,
        delay_ms: i64,
        observations: &Value,
    ) -> Result<(), ApiError> {
        let now = clock_ms();
        // The existing lease transition also saves measurements, in one roundtrip.
        // It grants no effect and never overwrites another runtime's config snapshot.
        sqlx::query(timings::DEFER_ALLOCATION_SQL)
            .bind(&job.id)
            .bind(&job.lease_token)
            .bind(now + delay_ms)
            .bind(now)
            .bind(observations)
            .bind(job.payload["operationId"].as_str())
            .bind(&job.resource_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
fn clock_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(super) async fn check_creation_capacity(
    pin: Option<&crate::managed_bases::candidate::StagingBasePin>,
    tx: &mut Transaction<'_, Postgres>,
    workspace: &Value,
    now: i64,
) -> Result<(), ApiError> {
    if !workspace["request_config"]["rustCreateRequestDigest"].is_string() {
        return Ok(());
    }
    if crate::managed_bases::candidate::compute_allowance(tx, pin, workspace, now).await? {
        return Ok(());
    }
    let account = required(workspace, "account_id")?;
    let entitled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND (status IN ('active','grace') OR (status='ended' AND paid_through>$2)) AND (paid_through IS NULL OR paid_through>$2))").bind(account).bind(now).fetch_one(&mut **tx).await?;
    if !entitled {
        return Err(ApiError::forbidden("cloud_entitlement_required"));
    }
    crate::billing::require_capacity(tx, account, now).await
}
