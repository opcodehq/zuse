//! The controller owns account credentials; the remote window owns only its socket.
use super::*;
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use zuse_protocol::managed_connection::Bootstrap;
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
struct AccountEngine {
    client: Arc<RpcClient>,
    source: Arc<Source>,
    closed: CancellationToken,
}
#[async_trait]
impl EngineBackend for AccountEngine {
    fn client(&self) -> &RpcClient {
        &self.client
    }
    fn connection_end(&self) -> Option<CancellationToken> {
        Some(self.closed.clone())
    }
    fn mode(&self) -> EngineMode {
        EngineMode::Remote {
            url: self
                .source
                .origin
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }
    async fn media_client(&self) -> Result<RpcClient, RpcError> {
        Ok(zuse_rpc::managed::connect(self.source.clone())
            .await?
            .client)
    }
    async fn shutdown(&self) {
        self.source.closed.cancel();
    }
}
pub(super) async fn bootstrap(target: AccountTarget) -> anyhow::Result<EngineHandle> {
    let environment = target.environment.clone();
    let source = Source::open(target).await?;
    let connection = zuse_rpc::managed::connect(source.clone()).await?;
    let client = connection.client;
    let info = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        query_engine_info(&client),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Account computer identity timed out"))??;
    anyhow::ensure!(
        info.device_id == environment,
        "The selected computer returned a different identity"
    );
    let origin = source
        .origin
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let remember=source.target.controller.client().call_scoped(methods::REMEMBER_ACCOUNT_ROUTE,serde_json::json!({"expectedSubject":source.target.subject,"environmentId":source.target.environment,"label":source.target.label,"origin":origin}));
    if !matches!(
        tokio::time::timeout(std::time::Duration::from_secs(2), remember).await,
        Ok(Ok(_))
    ) {
        tracing::warn!("The account connection opened but saving its route was not confirmed");
    }
    Ok(EngineHandle {
        inner: Arc::new(AccountEngine {
            client: Arc::new(client),
            source,
            closed: connection.closed,
        }),
        engine_info: info,
        deferred_state: None,
    })
}
