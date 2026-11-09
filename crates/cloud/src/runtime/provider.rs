//! Connect the runtime controller to a provider selected from the workspace's durable identity.
use super::allocation::{AllocationProvider, RuntimeLaunch, launch_command};
use super::deletion::DeletionProvider;
use super::lifecycle::RetainedProvider;
use crate::providers::contract::Allocation;
use crate::providers::{
    Machine, MachineState, ProviderError,
    contract::{OperationState, Provider, ProviderOperation},
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use zuse_protocol::cloud_lifecycle::Directive;
#[cfg(test)]
mod tests;

pub struct RuntimeProvider {
    pub(super) provider: Arc<dyn Provider>,
    pub(super) size: String,
    startup_timings: super::allocation::timings::Observations,
    pub(super) artifact_binding: Option<crate::providers::artifact::ArtifactBinding>,
    fast_start: bool,
}
impl super::RuntimeControl {
    pub(super) async fn provider_for_job(
        &self,
        registry: &crate::providers::registry::Registry,
        job: &crate::jobs::Job,
    ) -> Result<RuntimeProvider, crate::error::ApiError> {
        let row: Option<(String, serde_json::Value)> = sqlx::query_as(
            "SELECT provider,request_config FROM api_cloud_workspaces WHERE workspace_id=$1",
        )
        .bind(&job.resource_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some((id, config)) = row {
            if let Ok(id) = serde_json::from_value::<zuse_protocol::cloud_computers::ProviderId>(
                serde_json::json!(id),
            ) {
                if let Some(provider) = registry.get(id) {
                    let binding = config
                        .get("providerArtifactBinding")
                        .filter(|v| !v.is_null())
                        .map(|v| serde_json::from_value(v.clone()))
                        .transpose()
                        .map_err(|_| {
                            crate::error::ApiError::conflict(
                                "cloud_provider_artifact_binding_invalid",
                            )
                        })?;
                    let mut adapter = RuntimeProvider::new(
                        provider,
                        config["computerSizeId"]
                            .as_str()
                            .unwrap_or("default")
                            .into(),
                    );
                    adapter.artifact_binding = binding;
                    adapter.fast_start = config["rustFastStartVersion"] == 1;
                    return Ok(adapter);
                }
            }
        }
        if crate::jobs::retry(
            &self.pool,
            job,
            chrono::Utc::now().timestamp_millis(),
            "cloud_provider_not_configured",
        )
        .await?
        {
            sqlx::query("UPDATE api_cloud_workspaces w SET status_code='cloud_provider_not_configured',revision=revision+1 WHERE workspace_id=$1 AND state NOT IN ('deleted','archived','paused') AND EXISTS(SELECT 1 FROM zuse_rust_control_owners o WHERE o.resource_kind='workspace' AND o.resource_id=w.workspace_id AND o.backend='rust' AND o.generation=$2)")
                .bind(&job.resource_id).bind(job.generation).execute(&self.pool).await?;
        }
        Err(crate::error::ApiError::unavailable(
            "cloud_provider_not_configured",
        ))
    }
}
impl RuntimeProvider {
    pub fn new(provider: Arc<dyn Provider>, size: String) -> Self {
        Self {
            provider,
            size,
            artifact_binding: None,
            fast_start: false,
            startup_timings: Default::default(),
        }
    }
}
#[async_trait]
impl AllocationProvider for RuntimeProvider {
    fn startup_observations(&self) -> Value {
        self.startup_timings.snapshot()
    }
    fn provider_id(&self) -> &'static str {
        self.provider.id().as_str()
    }
    async fn find(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        self.startup_timings
            .measure("providerLookup", self.provider.find_allocation(name))
            .await
    }
    async fn inspect_id(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.startup_timings
            .measure("providerInspect", self.provider.inspect(id))
            .await
    }
    async fn runtime_ready(&self, id: &str) -> Result<bool, ProviderError> {
        self.startup_timings
            .measure("providerRuntimeReadiness", self.provider.runtime_ready(id))
            .await
    }
    async fn create(&self, snapshot: &str, name: &str) -> Result<Machine, ProviderError> {
        self.create_operation(snapshot, name, name).await
    }
    async fn create_operation(
        &self,
        snapshot: &str,
        name: &str,
        operation: &str,
    ) -> Result<Machine, ProviderError> {
        self.startup_timings
            .measure(
                "providerCreate",
                self.provider.allocate(&Allocation {
                    artifact: snapshot,
                    artifact_binding: self.artifact_binding.as_ref(),
                    name,
                    operation_id: operation,
                    size: &self.size,
                    lifetime_seconds: 86400,
                }),
            )
            .await
    }
    async fn bootstrap_initial_runtime(
        &self,
        machine: &str,
        artifact: &str,
        name: &str,
        request: &crate::managed_bases::bootstrap::Request,
    ) -> Result<Option<Value>, ProviderError> {
        self.bootstrap_initial_runtime_with_source_receipt(machine, artifact, name, request, None)
            .await
    }
    async fn bootstrap_initial_runtime_with_source_receipt(
        &self,
        machine: &str,
        artifact: &str,
        name: &str,
        request: &crate::managed_bases::bootstrap::Request,
        receipt: Option<&crate::providers::contract::VerifiedCreateSource>,
    ) -> Result<Option<Value>, ProviderError> {
        if request.source_machine_id != machine
            || request.provider != self.provider.id()
            || request.purpose == crate::managed_bases::bootstrap::Purpose::RetainedResume
            || !request.valid()
            || receipt.is_some_and(|r| !r.matches_request(request, artifact, name))
        {
            return Err(ProviderError::Rejected);
        }
        self.startup_timings
            .measure(
                "initialSourceVerification",
                self.provider
                    .verify_initial_source_with_receipt(machine, artifact, name, receipt),
            )
            .await?;
        let result = self
            .startup_timings
            .measure(
                "initialBootstrapHelper",
                self.provider.execute(machine, &request.command()?),
            )
            .await?;
        self.startup_timings
            .measure("initialBootstrapProofValidation", async {
                if result.exit_code != 0 || result.stdout.len() > 65536 {
                    return Err(ProviderError::Rejected);
                }
                request.response(
                    &serde_json::from_slice(&result.stdout)
                        .map_err(|_| ProviderError::InvalidResponse)?,
                )
            })
            .await
    }
    async fn launch(&self, machine: &str, launch: &RuntimeLaunch) -> Result<(), ProviderError> {
        let mut verification = String::new();
        if let Some(binding) = &self.artifact_binding {
            binding.validate(self.provider.id(), &binding.artifact_id)?;
            self.startup_timings
                .measure(
                    "launchArtifactVerification",
                    self.provider
                        .verify_artifact(&binding.artifact_id, Some(binding)),
                )
                .await?;
            if self.fast_start {
                verification = format!(
                    "set -eu\nprintf '%s\\n' {} {} | sha256sum -c - >/dev/null || exit 78\n",
                    quote(&format!(
                        "{}  /usr/local/bin/zuse-cloud-runtime",
                        binding.runtime_sha256
                    )),
                    quote(&format!(
                        "{}  /opt/zuse/preparation/status/manifest.json",
                        binding.manifest_sha256
                    ))
                );
            } else {
                let result=self.startup_timings.measure("launchBinaryVerification",self.provider.execute(machine,"sha256sum /usr/local/bin/zuse-cloud-runtime /opt/zuse/preparation/status/manifest.json")).await?;
                let expected = format!(
                    "{}  /usr/local/bin/zuse-cloud-runtime\n{}  /opt/zuse/preparation/status/manifest.json",
                    binding.runtime_sha256, binding.manifest_sha256
                );
                if result.exit_code != 0
                    || std::str::from_utf8(&result.stdout).map(str::trim).ok()
                        != Some(expected.as_str())
                {
                    return Err(ProviderError::Rejected);
                }
            }
        }
        let (staged, command) = launch_command(launch);
        let command = format!("{verification}{command}");
        self.startup_timings
            .measure(
                "bootCredentialUpload",
                self.provider
                    .upload(machine, &staged, launch.boot_token.as_bytes()),
            )
            .await?;
        let result = self
            .startup_timings
            .measure(
                "runtimeLaunchCommand",
                self.provider.execute(machine, &command),
            )
            .await?;
        if result.exit_code == 78 {
            return Err(ProviderError::Rejected);
        }
        if result.exit_code != 0 {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
}
pub(super) fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
#[async_trait]
impl DeletionProvider for RuntimeProvider {
    fn provider_id(&self) -> &'static str {
        self.provider.id().as_str()
    }
    async fn find(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.provider.inspect(id).await
    }
    async fn find_target(
        &self,
        id: Option<&str>,
        name: Option<&str>,
    ) -> Result<Option<Machine>, ProviderError> {
        match (id, name) {
            (Some(id), _) => self.provider.inspect(id).await,
            (None, Some(name)) => self.provider.find_allocation(name).await,
            _ => Err(ProviderError::Rejected),
        }
    }
    async fn destroy(&self, id: &str) -> Result<(), ProviderError> {
        self.provider.delete(id).await.map(|_| ())
    }
    async fn destroy_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.provider.delete(id).await
    }
    async fn receipt_state(&self, r: &ProviderOperation) -> Result<OperationState, ProviderError> {
        self.provider.operation(r).await
    }
}
#[async_trait]
impl RetainedProvider for RuntimeProvider {
    async fn bootstrap_retained_runtime(
        &self,
        request: &crate::managed_bases::bootstrap::Request,
    ) -> Result<bool, ProviderError> {
        use crate::managed_bases::bootstrap::Purpose;
        if self.provider.id() != zuse_protocol::cloud_computers::ProviderId::Boat
            || request.provider != self.provider.id()
            || request.purpose != Purpose::RetainedResume
            || !request.valid()
        {
            return Err(ProviderError::Rejected);
        }
        if !self
            .provider
            .runtime_ready(&request.source_machine_id)
            .await?
        {
            return Ok(false);
        }
        let result = self
            .provider
            .execute(&request.source_machine_id, &request.command()?)
            .await?;
        if result.exit_code != 0 || result.stdout.len() > 65536 {
            return Err(ProviderError::Rejected);
        }
        Ok(request
            .response(
                &serde_json::from_slice(&result.stdout)
                    .map_err(|_| ProviderError::InvalidResponse)?,
            )?
            .is_some())
    }
    async fn retained_launch_ready(&self, id: &str) -> Result<bool, ProviderError> {
        self.provider.runtime_ready(id).await
    }
    fn provider_id(&self) -> &'static str {
        self.provider.id().as_str()
    }
    async fn inspect_retained(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.provider.inspect(id).await
    }
    async fn pause_retained(&self, id: &str) -> Result<(), ProviderError> {
        self.provider.sleep(id).await.map(|_| ())
    }
    async fn wake_retained(&self, id: &str, _: &MachineState) -> Result<(), ProviderError> {
        self.provider.resume(id, 86400).await.map(|_| ())
    }
    async fn sleep_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.provider.sleep(id).await
    }
    async fn memory_sleep_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.provider.sleep_memory(id).await
    }
    async fn memory_receipt_state(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        self.provider.memory_operation(receipt).await
    }
    async fn wake_receipt(
        &self,
        id: &str,
        _: &MachineState,
    ) -> Result<ProviderOperation, ProviderError> {
        self.provider.resume(id, 86400).await
    }
    async fn receipt_state(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        self.provider.operation(receipt).await
    }
    async fn launch_retained(
        &self,
        d: &Directive,
        workspace: &str,
        api: &str,
    ) -> Result<(), ProviderError> {
        let command = format!(
            "sudo -n -u zuse env ZUSE_RUNTIME_RETAINED=true ZUSE_CLOUD_WORKSPACE_ID={} ZUSE_API_URL={} ZUSE_RUNTIME_GENERATION={} ZUSE_STORAGE_INCARNATION_ID={} ZUSE_RUNTIME_RESUME_OPERATION_ID={} /usr/local/lib/zuse/launch-rust-runtime",
            quote(workspace),
            quote(api),
            d.generation,
            quote(&d.storage_incarnation_id),
            quote(&d.operation_id)
        );
        if self
            .provider
            .execute(&d.source_machine_id, &command)
            .await?
            .exit_code
            != 0
        {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
}

#[async_trait]
impl super::recovery::RecoveryProvider for RuntimeProvider {
    fn provider_id(&self) -> &'static str {
        self.provider.id().as_str()
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.provider.inspect(id).await
    }
    async fn wake(&self, id: &str, _: &MachineState) -> Result<(), ProviderError> {
        self.provider.resume(id, 86400).await.map(|_| ())
    }
    async fn pause(&self, id: &str) -> Result<(), ProviderError> {
        self.provider.sleep(id).await.map(|_| ())
    }
    async fn retire(&self, id: &str) -> Result<(), ProviderError> {
        self.provider.delete(id).await.map(|_| ())
    }
    async fn wake_receipt(
        &self,
        id: &str,
        _: &MachineState,
    ) -> Result<ProviderOperation, ProviderError> {
        self.provider.resume(id, 86400).await
    }
    async fn pause_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.provider.sleep(id).await
    }
    async fn retire_receipt(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.provider.delete(id).await
    }
    async fn recovery_state(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        self.provider.operation(receipt).await
    }
    async fn retirement_state(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        self.provider.operation(receipt).await
    }
    async fn capture(&self, plan: &super::recovery::CapturePlan) -> Result<(), ProviderError> {
        let (staged, command) = super::recovery::capture_command(plan);
        self.provider
            .upload(&plan.source_machine_id, &staged, plan.config.as_bytes())
            .await?;
        if self
            .provider
            .execute(&plan.source_machine_id, &command)
            .await?
            .exit_code
            != 0
        {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
    async fn stop_capture(&self, id: &str, operation: &str) -> Result<(), ProviderError> {
        let command = format!(
            "sudo -n -u zuse /usr/local/lib/zuse/launch-rust-recovery {} --cancel",
            quote(operation)
        );
        if self.provider.execute(id, &command).await?.exit_code != 0 {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
}
#[async_trait]
impl super::restore::RestoreProvider for RuntimeProvider {
    async fn prepare_recreation(
        &self,
        machine: &str,
        request: &Value,
    ) -> Result<Value, ProviderError> {
        let command = super::recreation::environment::command(request);
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            self.provider.execute(machine, &command),
        )
        .await
        .map_err(|_| ProviderError::Unavailable)??;
        if result.exit_code != 0 || result.stdout.len() > 65536 {
            return Err(ProviderError::InvalidResponse);
        }
        serde_json::from_slice(&result.stdout).map_err(|_| ProviderError::InvalidResponse)
    }

    async fn restore(
        &self,
        machine: &str,
        plan: &super::recovery::CapturePlan,
    ) -> Result<(), ProviderError> {
        let target = super::recovery::CapturePlan {
            operation_id: plan.operation_id.clone(),
            workspace_id: plan.workspace_id.clone(),
            source_machine_id: machine.into(),
            config: zeroize::Zeroizing::new(plan.config.to_string()),
        };
        super::recovery::RecoveryProvider::capture(self, &target).await
    }
    async fn installed(&self, machine: &str, archive: &str) -> Result<bool, ProviderError> {
        let path = format!("/var/lib/zuse/recovery/{archive}/restored");
        let command = format!(
            "sudo -n -u zuse sh -c 'if test -f \"$1\"; then cat -- \"$1\"; fi' sh {}",
            quote(&path)
        );
        let result = self.provider.execute(machine, &command).await?;
        if result.exit_code != 0 {
            return Err(ProviderError::Unavailable);
        }
        Ok(result.stdout == b"workspace_archive_restored\n")
    }
}

impl super::RuntimeControl {
    pub async fn recovery_registry_once(
        &self,
        registry: &crate::providers::registry::Registry,
        now: i64,
    ) -> Result<bool, crate::error::ApiError> {
        let ready: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_recovery_operations') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if !ready {
            return Ok(false);
        }
        self.recovery_admit_due(now).await?;
        let Some(job) = crate::jobs::claim(
            &self.pool,
            super::recovery::JOB_KIND,
            "workspace",
            now,
            120000,
        )
        .await?
        else {
            return Ok(false);
        };
        let provider = self.provider_for_job(registry, &job).await?;
        self.recovery_run_job(&provider, &job, now).await
    }
    pub async fn restore_registry_once(
        &self,
        registry: &crate::providers::registry::Registry,
        now: i64,
    ) -> Result<bool, crate::error::ApiError> {
        let ready: bool =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_restores') IS NOT NULL")
                .fetch_one(&self.pool)
                .await?;
        if !ready {
            return Ok(false);
        }
        let Some(job) = crate::jobs::claim(
            &self.pool,
            super::restore::JOB_KIND,
            "workspace",
            now,
            120000,
        )
        .await?
        else {
            return Ok(false);
        };
        let provider = self.provider_for_job(registry, &job).await?;
        self.restore_run_job(&provider, &job, now).await
    }
}
