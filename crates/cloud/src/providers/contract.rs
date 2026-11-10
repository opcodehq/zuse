//! Provider operations acknowledge intent; the durable controller reconciles completion.
use super::{ExecResult, Machine, ProviderError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use zuse_protocol::cloud_computers::ProviderId;

pub struct Allocation<'a> {
    pub artifact: &'a str,
    pub artifact_binding: Option<&'a super::artifact::ArtifactBinding>,
    pub name: &'a str,
    pub operation_id: &'a str,
    pub size: &'a str,
    pub lifetime_seconds: u32,
}
impl Allocation<'_> {
    pub fn validate(&self) -> Result<(), ProviderError> {
        if self.artifact.is_empty()
            || self.artifact.len() > 512
            || self.artifact.chars().any(char::is_control)
            || self.name.is_empty()
            || self.name.len() > 63
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || self.operation_id.is_empty()
            || self.operation_id.len() > 160
            || !self
                .operation_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b))
            || !(60..=86400).contains(&self.lifetime_seconds)
        {
            return Err(ProviderError::Rejected);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesiredState {
    Sleeping,
    Running,
    Deleted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderOperation {
    pub resource_id: String,
    pub operation_id: Option<String>,
    pub desired: DesiredState,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationState {
    Pending,
    /// Provider still owns a retrying operation. Do not submit another mutation.
    Retrying,
    Completed,
    Superseded,
}

/// Opaque authority minted only after the allocation worker validates its durable
/// direct-create receipt under the current source, generation and job lease.
/// It cannot be deserialized from a client, provider response or guest payload.
pub struct VerifiedCreateSource {
    request: crate::managed_bases::bootstrap::Request,
    artifact: String,
    name: String,
}
impl VerifiedCreateSource {
    pub(crate) fn from_fenced_positive_create(
        request: &crate::managed_bases::bootstrap::Request,
        artifact: &str,
        name: &str,
    ) -> Self {
        Self {
            request: request.clone(),
            artifact: artifact.into(),
            name: name.into(),
        }
    }
    pub(crate) fn matches_request(
        &self,
        request: &crate::managed_bases::bootstrap::Request,
        artifact: &str,
        name: &str,
    ) -> bool {
        self.request == *request
            && request.valid()
            && self.artifact == artifact
            && self.name == name
    }
    pub(crate) fn matches_operation_label(&self, value: &serde_json::Value) -> bool {
        value.as_str() == Some(self.request.operation_id.as_str())
    }
    pub(crate) fn matches_boat_source(&self, machine: &str, artifact: &str, name: &str) -> bool {
        self.request.valid()
            && self.request.provider == ProviderId::Boat
            && self.request.source_machine_id == machine
            && self.artifact == artifact
            && self.name == name
    }
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError>;
    /// Separate task-runtime readiness from generic running/stop reconciliation.
    /// Providers with a materialization boundary override this without adding
    /// an extra inspection to the other providers' launch paths.
    async fn runtime_ready(&self, _id: &str) -> Result<bool, ProviderError> {
        Ok(true)
    }
    /// Only an explicitly opted-in fresh managed image may enter this path.
    async fn verify_initial_source(
        &self,
        _machine: &str,
        _artifact: &str,
        _name: &str,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::Rejected)
    }
    /// Providers retain their strict verification unless they explicitly support
    /// the worker's opaque, positive-create authority.
    async fn verify_initial_source_with_receipt(
        &self,
        machine: &str,
        artifact: &str,
        name: &str,
        _receipt: Option<&VerifiedCreateSource>,
    ) -> Result<(), ProviderError> {
        self.verify_initial_source(machine, artifact, name).await
    }
    /// A fully exhausted listing can establish absence; an incomplete listing cannot.
    async fn find_allocation(&self, name: &str) -> Result<Option<Machine>, ProviderError>;
    async fn verify_artifact(
        &self,
        artifact: &str,
        binding: Option<&super::artifact::ArtifactBinding>,
    ) -> Result<(), ProviderError> {
        let _ = artifact;
        if binding.is_some() {
            Err(ProviderError::Rejected)
        } else {
            Ok(())
        }
    }
    async fn allocate(&self, request: &Allocation<'_>) -> Result<Machine, ProviderError>;
    async fn sleep(&self, id: &str) -> Result<ProviderOperation, ProviderError>;
    /// Only an authenticated, checked-flushed in-process park may request RAM preservation.
    async fn sleep_memory(&self, _id: &str) -> Result<ProviderOperation, ProviderError> {
        Err(ProviderError::Rejected)
    }
    async fn memory_operation(
        &self,
        _receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        Err(ProviderError::Rejected)
    }
    async fn resume(
        &self,
        id: &str,
        lifetime_seconds: u32,
    ) -> Result<ProviderOperation, ProviderError>;
    async fn delete(&self, id: &str) -> Result<ProviderOperation, ProviderError>;
    async fn operation(&self, receipt: &ProviderOperation)
    -> Result<OperationState, ProviderError>;
    async fn execute(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError>;
    async fn upload(&self, id: &str, path: &str, contents: &[u8]) -> Result<(), ProviderError>;
    /// Push back a provider-side running limit for a computer that is still in
    /// use. Providers whose limit already outlasts any run keep this no-op.
    async fn keep_running(&self, _id: &str) -> Result<(), ProviderError> {
        Ok(())
    }
}

pub(super) fn observed(receipt: &ProviderOperation, machine: Option<&Machine>) -> OperationState {
    use super::MachineState;
    let done = match (receipt.desired, machine) {
        (DesiredState::Deleted, None) => true,
        (DesiredState::Running, Some(machine)) => machine.state == MachineState::Running,
        (DesiredState::Sleeping, Some(machine)) => matches!(
            machine.state,
            MachineState::Standby | MachineState::Paused | MachineState::Stopped
        ),
        _ => false,
    };
    if done {
        OperationState::Completed
    } else {
        OperationState::Pending
    }
}
