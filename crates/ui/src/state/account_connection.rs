//! The controller owns account credentials; the remote window owns only its socket.
use super::*;
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use zuse_protocol::managed_connection::Bootstrap;
#[cfg(test)]
mod tests;
#[cfg(test)]
use super::remote_connection::Status;
#[derive(Clone)]
pub struct AccountTarget {
    pub controller: EngineHandle,
    pub subject: String,
    pub environment: String,
    pub label: String,
}
#[derive(Clone)]
pub enum RemoteTarget {
    Paired(zuse_remote::PairedTarget),
    Account(AccountTarget),
}
impl std::fmt::Debug for RemoteTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Paired(_) => f.write_str("Paired"),
            Self::Account(target) => f
                .debug_struct("Account")
                .field("environment", &target.environment)
                .finish_non_exhaustive(),
        }
    }
}
struct Source {
    target: AccountTarget,
    closed: CancellationToken,
    origin: std::sync::RwLock<String>,
    remembered: std::sync::Mutex<String>,
    _watch: AbortOnDropHandle<()>,
}
impl Source {
    async fn open(target: AccountTarget) -> Result<Arc<Self>, RpcError> {
        let mut account = target
            .controller
            .client()
            .subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS, serde_json::json!({}))
            .await?;
        let value = tokio::time::timeout(std::time::Duration::from_secs(10), account.recv())
            .await
            .map_err(|_| RpcError::Transport("Account status timed out".into()))?
            .ok_or(RpcError::Closed)?;
        if value["user"]["id"].as_str() != Some(target.subject.as_str()) {
            return Err(RpcError::Failed(
                "The account changed. Select the computer again.".into(),
            ));
        }
        let closed = CancellationToken::new();
        let retire = closed.clone();
        let subject = target.subject.clone();
        let watch = tokio::spawn(async move {
            loop {
                tokio::select! {biased;
                    _=retire.cancelled()=>return,
                    value=account.recv()=>match value {
                        Some(value) if value["user"]["id"].as_str()==Some(subject.as_str())=>{},
                        _=>{retire.cancel();return;},
                    }
                }
            }
        });
        Ok(Arc::new(Self {
            target,
            closed,
            origin: std::sync::RwLock::new(String::new()),
            remembered: std::sync::Mutex::new(String::new()),
            _watch: AbortOnDropHandle::new(watch),
        }))
    }
}
#[async_trait]
impl zuse_rpc::managed::GrantSource for Source {
    fn environment(&self) -> &str {
        &self.target.environment
    }
    fn cancellation(&self) -> CancellationToken {
        self.closed.clone()
    }
    async fn grant(&self) -> Result<Bootstrap, RpcError> {
        let value=self.target.controller.client().call_scoped(methods::BOOTSTRAP_ACCOUNT_CONNECTION,serde_json::json!({"environmentId":self.target.environment,"expectedSubject":self.target.subject})).await.map_err(|error| {
            if matches!(&error,RpcError::Failed(message) if message==zuse_protocol::managed_connection::REJECTED) {
                self.closed.cancel();
                RpcError::Failed("The computer connection was rejected. Check its access and app version before selecting it again.".into())
            }else{error}
        })?;
        let grant: Bootstrap = serde_json::from_value(value)
            .map_err(|_| RpcError::Failed("Invalid account connection response".into()))?;
        if grant.environment_id != self.target.environment {
            return Err(RpcError::Failed("The selected computer changed".into()));
        }
        let origin = zuse_remote::normalized_origin(&grant.origin)
            .map_err(|_| RpcError::Failed("Invalid computer endpoint".into()))?;
        *self.origin.write().unwrap_or_else(|e| e.into_inner()) = origin;
        Ok(grant)
    }
}
fn origin(source: &Source) -> String {
    source
        .origin
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}
async fn remember(source: &Source, origin: &str) {
    let remember=source.target.controller.client().call_scoped(methods::REMEMBER_ACCOUNT_ROUTE,serde_json::json!({"expectedSubject":source.target.subject,"environmentId":source.target.environment,"label":source.target.label,"origin":origin}));
    if !matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), remember).await,
        Ok(Ok(_))
    ) {
        tracing::warn!("The account connection opened but saving its route was not confirmed");
    }
}

#[async_trait]
impl super::remote_connection::Source for Source {
    fn cancellation(&self) -> CancellationToken {
        self.closed.clone()
    }
    fn origin(&self) -> String {
        origin(self)
    }
    fn closed_message(&self) -> &'static str {
        "Account access ended. Check the controller account and select this computer again."
    }
    async fn connect(self: Arc<Self>) -> Result<zuse_rpc::reconnect::Channel, RpcError> {
        zuse_rpc::managed::connect_channel(self).await
    }
    async fn connected(&self) {
        let next = origin(self);
        if *self.remembered.lock().unwrap_or_else(|e| e.into_inner()) == next {
            return;
        }
        remember(self, &next).await;
        *self.remembered.lock().unwrap_or_else(|e| e.into_inner()) = next;
    }
}
pub(super) async fn bootstrap(target: AccountTarget) -> anyhow::Result<EngineHandle> {
    let environment = target.environment.clone();
    super::remote_connection::bootstrap(Source::open(target).await?, Some(&environment)).await
}
