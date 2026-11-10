use super::{ExecResult, Machine, MachineState, ProviderError, boxd::Boxd, contract::*};
use async_trait::async_trait;
use zuse_protocol::cloud_computers::ProviderId;

#[async_trait]
impl Provider for Boxd {
    fn id(&self) -> ProviderId {
        ProviderId::Boxd
    }
    async fn inspect(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        Boxd::inspect(self, id).await
    }
    async fn verify_initial_source(
        &self,
        machine: &str,
        artifact: &str,
        name: &str,
    ) -> Result<(), ProviderError> {
        self.verify_initial_snapshot_source(machine, artifact, name)
            .await
    }
    async fn find_allocation(&self, name: &str) -> Result<Option<Machine>, ProviderError> {
        Boxd::inspect(self, name).await
    }
    async fn allocate(&self, request: &Allocation<'_>) -> Result<Machine, ProviderError> {
        request.validate()?;
        self.verify_artifact(request.artifact, request.artifact_binding)
            .await?;
        if request.size != "default" {
            return Err(ProviderError::Rejected);
        }
        self.create_from_snapshot(request.artifact, request.name, true)
            .await
    }
    async fn sleep(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.hibernate(id).await?;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Sleeping,
        })
    }
    async fn sleep_memory(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        self.suspend(id).await?;
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Sleeping,
        })
    }
    async fn memory_operation(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        if receipt.operation_id.is_some() || receipt.desired != DesiredState::Sleeping {
            return Err(ProviderError::InvalidResponse);
        }
        let machine = Boxd::inspect(self, &receipt.resource_id).await?;
        Ok(match machine.as_ref().map(|m| &m.state) {
            Some(MachineState::Standby) => OperationState::Completed,
            Some(MachineState::Paused | MachineState::Stopped) | None => OperationState::Superseded,
            _ => OperationState::Pending,
        })
    }
    async fn resume(
        &self,
        id: &str,
        lifetime_seconds: u32,
    ) -> Result<ProviderOperation, ProviderError> {
        if !(60..=86400).contains(&lifetime_seconds) {
            return Err(ProviderError::Rejected);
        }
        let machine = Boxd::inspect(self, id)
            .await?
            .ok_or(ProviderError::NotFound)?;
        match machine.state {
            MachineState::Running | MachineState::Starting => {}
            _ => self.activate(id, &machine.state).await?,
        }
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Running,
        })
    }
    async fn delete(&self, id: &str) -> Result<ProviderOperation, ProviderError> {
        match self.destroy(id).await {
            Ok(()) | Err(ProviderError::NotFound) => {}
            Err(error) => return Err(error),
        }
        Ok(ProviderOperation {
            resource_id: id.into(),
            operation_id: None,
            desired: DesiredState::Deleted,
        })
    }
    async fn operation(
        &self,
        receipt: &ProviderOperation,
    ) -> Result<OperationState, ProviderError> {
        if receipt.operation_id.is_some() {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(super::contract::observed(
            receipt,
            Boxd::inspect(self, &receipt.resource_id).await?.as_ref(),
        ))
    }
    async fn execute(&self, id: &str, command: &str) -> Result<ExecResult, ProviderError> {
        self.exec(id, command).await
    }
    async fn upload(&self, id: &str, path: &str, contents: &[u8]) -> Result<(), ProviderError> {
        Boxd::upload(self, id, path, contents).await
    }
}
