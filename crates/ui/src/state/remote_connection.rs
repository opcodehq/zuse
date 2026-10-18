//! Stable native remote clients; each authentication mode supplies its own channel.
use super::*;
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
#[cfg(test)]
mod tests;
#[async_trait]
pub(super) trait Source: Send + Sync {
    fn cancellation(&self) -> CancellationToken;
    fn origin(&self) -> String;
    fn closed_message(&self) -> &'static str;
    fn blocked_error(&self, _: &RpcError) -> bool {
        false
    }
    fn legacy_identity(&self) -> bool {
        false
    }
    async fn connect(self: Arc<Self>) -> Result<zuse_rpc::reconnect::Channel, RpcError>;
    async fn connected(&self) {}
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Status {
    Ready,
    Reconnecting,
    Blocked(String),
}
struct RemoteEngine {
    client: Arc<RpcClient>,
    expected: EngineInfo,
    source: Arc<dyn Source>,
    control: zuse_rpc::reconnect::Control,
    status: tokio::sync::watch::Receiver<Status>,
    retry: Arc<tokio::sync::Notify>,
    _reconnect: AbortOnDropHandle<()>,
}
#[async_trait]
impl EngineBackend for RemoteEngine {
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
        if self.source.cancellation().is_cancelled() || self.control.stopped().is_cancelled() {
            return false;
        }
        self.retry.notify_one();
        true
    }
    fn mode(&self) -> EngineMode {
        EngineMode::Remote {
            url: self.source.origin(),
        }
    }
    async fn media_client(&self) -> Result<RpcClient, RpcError> {
        let (channel, info) = verified(self.source.clone()).await?;
        if info != self.expected {
            self.source.cancellation().cancel();
            return Err(RpcError::Failed(
                "The computer runtime changed. Reopen this connection.".into(),
            ));
        }
        Ok(RpcClient::new(channel.outgoing, channel.incoming))
    }
    async fn shutdown(&self) {
        self.source.cancellation().cancel();
        self.control.close();
    }
}
/// Identity is verified before a channel can enter the stable RPC client.
async fn verified(
    source: Arc<dyn Source>,
) -> Result<(zuse_rpc::reconnect::Channel, EngineInfo), RpcError> {
    let cancelled = source.cancellation();
    tokio::select! {biased;_=cancelled.cancelled()=>Err(RpcError::Closed),value=verify_identity(source)=>value}
}
async fn verify_identity(
    source: Arc<dyn Source>,
) -> Result<(zuse_rpc::reconnect::Channel, EngineInfo), RpcError> {
    let legacy = source.legacy_identity();
    let mut channel = source.connect().await?;
    let verify = async {
        async fn query(
            channel: &mut zuse_rpc::reconnect::Channel,
            method: &str,
        ) -> Result<serde_json::Value, RpcError> {
            channel
                .outgoing
                .send(serde_json::json!({"id":0,"method":method,"params":{}}).to_string())
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
            if let Some(error) = frame.err {
                if error == format!("unknown method: {method}") {
                    return Err(RpcError::UnknownMethod(method.into()));
                }
                return Err(RpcError::Failed("Computer identity unavailable".into()));
            }
            frame
                .ok
                .ok_or_else(|| RpcError::Transport("Computer identity unavailable".into()))
        }
        match query(&mut channel, methods::ENGINE_INFO).await {
            Ok(value) => serde_json::from_value(value)
                .map_err(|_| RpcError::Transport("Invalid computer identity".into())),
            Err(RpcError::UnknownMethod(_)) if legacy => {
                let value = query(&mut channel, methods::LOCAL_DEVICE).await?;
                let device = value["deviceId"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| RpcError::Transport("Invalid computer identity".into()))?;
                Ok(EngineInfo {
                    device_id: device.into(),
                    workspace_scope: WorkspaceScope::Synced,
                    cursor_sdk_version: None,
                    capabilities: vec![],
                })
            }
            Err(error) => Err(error),
        }
    };
    let info = tokio::time::timeout(std::time::Duration::from_secs(10), verify)
        .await
        .map_err(|_| RpcError::Transport("Computer identity timed out".into()))??;
    Ok((channel, info))
}
async fn reconnect(
    source: Arc<dyn Source>,
    expected: EngineInfo,
    control: zuse_rpc::reconnect::Control,
    status: tokio::sync::watch::Sender<Status>,
    retry: Arc<tokio::sync::Notify>,
    mut lease: CancellationToken,
) {
    let stopped = control.stopped();
    let cancelled = source.cancellation();
    loop {
        tokio::select! {biased;_=stopped.cancelled()=>return,_=cancelled.cancelled()=>{},_=lease.cancelled()=>{}}
        if cancelled.is_cancelled() {
            control.close();
            status.send_replace(Status::Blocked(source.closed_message().into()));
            return;
        }
        status.send_replace(Status::Reconnecting);
        let mut attempt = 0;
        loop {
            let delay = std::time::Duration::from_millis(
                zuse_protocol::browser_link::retry_delay_ms(attempt) as u64,
            );
            tokio::select! {biased;_=stopped.cancelled()=>return,_=cancelled.cancelled()=>break,_=retry.notified()=>{},_=tokio::time::sleep(delay)=>{}}
            let result = tokio::select! {biased;_=stopped.cancelled()=>return,_=cancelled.cancelled()=>break,result=verified(source.clone())=>result};
            match result {
                Ok((channel, info)) => {
                    if info != expected {
                        cancelled.cancel();
                        control.close();
                        status.send_replace(Status::Blocked("The computer identity or runtime changed. Reopen it from Remote access before continuing; local edits are preserved.".into()));
                        return;
                    }
                    source.connected().await;
                    if cancelled.is_cancelled() {
                        break;
                    }
                    match control.attach(channel) {
                        Ok(next) => lease = next,
                        Err(_) => return,
                    }
                    status.send_replace(Status::Ready);
                    break;
                }
                Err(error) => {
                    if source.blocked_error(&error) {
                        cancelled.cancel();
                        control.close();
                        status.send_replace(Status::Blocked(error.to_string()));
                        return;
                    }
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }
}
pub(super) async fn bootstrap(
    source: Arc<dyn Source>,
    expected_device: Option<&str>,
) -> anyhow::Result<EngineHandle> {
    let (channel, info) = verified(source.clone()).await?;
    anyhow::ensure!(
        expected_device.is_none_or(|device| info.device_id == device),
        "The selected computer returned a different identity"
    );
    source.connected().await;
    anyhow::ensure!(
        !source.cancellation().is_cancelled(),
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
        inner: Arc::new(RemoteEngine {
            client: Arc::new(client),
            expected: info.clone(),
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

struct Pairing {
    target: zuse_remote::PairedTarget,
    closed: CancellationToken,
}
#[async_trait]
impl Source for Pairing {
    fn cancellation(&self) -> CancellationToken {
        self.closed.clone()
    }
    fn origin(&self) -> String {
        self.target.origin().into()
    }
    fn closed_message(&self) -> &'static str {
        "Paired access ended. Open a new invitation before reconnecting."
    }
    fn blocked_error(&self, error: &RpcError) -> bool {
        matches!(error, RpcError::Failed(_))
    }
    fn legacy_identity(&self) -> bool {
        true
    }
    async fn connect(self: Arc<Self>) -> Result<zuse_rpc::reconnect::Channel, RpcError> {
        zuse_rpc::connect_paired_channel(&self.target, self.closed.clone()).await
    }
}
pub(super) async fn paired(target: zuse_remote::PairedTarget) -> anyhow::Result<EngineHandle> {
    bootstrap(
        Arc::new(Pairing {
            target,
            closed: CancellationToken::new(),
        }),
        None,
    )
    .await
}
