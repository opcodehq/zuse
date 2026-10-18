//! The controller owns account credentials; the remote window owns only its socket.
use super::*;
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use zuse_protocol::managed_connection::Bootstrap;
#[cfg(test)]
mod tests;
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Status {
    Ready,
    Reconnecting,
    Blocked(String),
}
struct AccountEngine {
    client: Arc<RpcClient>,
    source: Arc<Source>,
    control: zuse_rpc::reconnect::Control,
    status: tokio::sync::watch::Receiver<Status>,
    retry: Arc<tokio::sync::Notify>,
    _reconnect: AbortOnDropHandle<()>,
}
#[async_trait]
impl EngineBackend for AccountEngine {
    fn client(&self) -> &RpcClient {
        &self.client
    }
    fn remote_status(&self) -> Option<tokio::sync::watch::Receiver<Status>> {
        Some(self.status.clone())
    }
    fn connection_revision(&self) -> u64 {
        self.control.generation()
    }
    fn retry_connection(&self) -> bool {
        if self.source.closed.is_cancelled() || self.control.stopped().is_cancelled() {
            return false;
        }
        self.retry.notify_one();
        true
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
        self.control.close();
    }
}
/// Identity is verified before a channel can enter the stable RPC client.
async fn verified(
    source: Arc<Source>,
) -> anyhow::Result<(zuse_rpc::reconnect::Channel, EngineInfo)> {
    let mut channel = zuse_rpc::managed::connect_channel(source).await?;
    let verify = async {
        channel
            .outgoing
            .send(serde_json::json!({"id":0,"method":methods::ENGINE_INFO,"params":{}}).to_string())
            .await
            .map_err(|_| RpcError::Closed)?;
        let text = channel.incoming.recv().await.ok_or(RpcError::Closed)?;
        let frame: zuse_rpc::ServerFrame = serde_json::from_str(&text)
            .map_err(|_| RpcError::Transport("Invalid computer identity response".into()))?;
        if frame.id != 0 {
            return Err(RpcError::Transport(
                "Unexpected computer identity response".into(),
            ));
        }
        let info = serde_json::from_value(
            frame
                .ok
                .ok_or_else(|| RpcError::Transport("Computer identity unavailable".into()))?,
        )
        .map_err(|_| RpcError::Transport("Invalid computer identity".into()))?;
        Ok::<EngineInfo, RpcError>(info)
    };
    let info = tokio::time::timeout(std::time::Duration::from_secs(10), verify)
        .await
        .map_err(|_| anyhow::anyhow!("Account computer identity timed out"))??;
    Ok((channel, info))
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
async fn reconnect(
    source: Arc<Source>,
    expected: EngineInfo,
    control: zuse_rpc::reconnect::Control,
    status: tokio::sync::watch::Sender<Status>,
    retry: Arc<tokio::sync::Notify>,
    mut lease: CancellationToken,
) {
    let stopped = control.stopped();
    let mut remembered = origin(&source);
    loop {
        tokio::select! {biased;_=stopped.cancelled()=>return,_=source.closed.cancelled()=>{},_=lease.cancelled()=>{}}
        if source.closed.is_cancelled() {
            control.close();
            status.send_replace(Status::Blocked("Account access ended. Check the controller account and select this computer again.".into()));
            return;
        }
        status.send_replace(Status::Reconnecting);
        let mut attempt = 0;
        loop {
            let delay = std::time::Duration::from_millis(
                zuse_protocol::browser_link::retry_delay_ms(attempt) as u64,
            );
            tokio::select! {biased;_=stopped.cancelled()=>return,_=source.closed.cancelled()=>break,_=retry.notified()=>{},_=tokio::time::sleep(delay)=>{}}
            let result = tokio::select! {biased;_=stopped.cancelled()=>return,_=source.closed.cancelled()=>break,result=verified(source.clone())=>result};
            match result {
                Ok((channel, info)) => {
                    if info != expected {
                        source.closed.cancel();
                        control.close();
                        status.send_replace(Status::Blocked("The computer identity or runtime changed. Reopen it from Remote access before continuing; local edits are preserved.".into()));
                        return;
                    }
                    let next = origin(&source);
                    if next != remembered {
                        remember(&source, &next).await;
                        remembered = next;
                    }
                    if source.closed.is_cancelled() {
                        break;
                    }
                    match control.attach(channel) {
                        Ok(next) => lease = next,
                        Err(_) => return,
                    }
                    status.send_replace(Status::Ready);
                    break;
                }
                Err(_) => {
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }
}
pub(super) async fn bootstrap(target: AccountTarget) -> anyhow::Result<EngineHandle> {
    let environment = target.environment.clone();
    let source = Source::open(target).await?;
    let (channel, info) = verified(source.clone()).await?;
    anyhow::ensure!(
        info.device_id == environment,
        "The selected computer returned a different identity"
    );
    remember(&source, &origin(&source)).await;
    anyhow::ensure!(
        !source.closed.is_cancelled(),
        "The controller account connection closed"
    );
    let (client, control) = RpcClient::reconnecting();
    let lease = control.attach(channel)?;
    let (status, snapshots) = tokio::sync::watch::channel(Status::Ready);
    let retry = Arc::new(tokio::sync::Notify::new());
    let task = tokio::spawn(reconnect(
        source.clone(),
        info.clone(),
        control.clone(),
        status,
        retry.clone(),
        lease,
    ));
    let connection_revision = control.generation();
    Ok(EngineHandle {
        connection_revision,
        inner: Arc::new(AccountEngine {
            client: Arc::new(client),
            source,
            control,
            status: snapshots,
            retry,
            _reconnect: AbortOnDropHandle::new(task),
        }),
        engine_info: info,
        deferred_state: None,
    })
}
