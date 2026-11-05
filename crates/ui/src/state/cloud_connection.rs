//! Native tools share the controller's cloud supervisor. No grant or account token enters this backend.
use super::*;
#[cfg(test)]
mod tests;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
#[derive(Clone)]
pub struct CloudTarget {
    pub controller: EngineHandle,
    pub subject: String,
    pub organization: Option<String>,
    pub workspace: String,
    pub initial_session: String,
}
struct Source {
    target: CloudTarget,
    closed: CancellationToken,
    status: watch::Receiver<Value>,
    _watch: AbortOnDropHandle<()>,
}
fn binding(value: &Value) -> Option<(String, Option<String>)> {
    Some((
        value["user"]["id"].as_str()?.to_owned(),
        value["orgId"].as_str().map(str::to_owned),
    ))
}

impl Source {
    async fn open(target: CloudTarget) -> Result<Arc<Self>, RpcError> {
        let mut auth = target
            .controller
            .client()
            .subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS, json!({}))
            .await?;
        let first = tokio::time::timeout(std::time::Duration::from_secs(10), auth.recv())
            .await
            .map_err(|_| RpcError::Transport("Account status timed out".into()))?
            .ok_or(RpcError::Closed)?;
        let expected = (target.subject.clone(), target.organization.clone());
        if binding(&first) != Some(expected.clone()) {
            return Err(RpcError::Failed(
                "Select this cloud workspace from its original account and organization.".into(),
            ));
        }
        let mut updates = target
            .controller
            .client()
            .subscribe_scoped(
                methods::CLOUD_ENVIRONMENT,
                json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,"expectedOrganizationId":target.organization,"interactiveResume":false}),
            )
            .await?;
        let closed = CancellationToken::new();
        let cancel = closed.clone();
        let (send, status) = watch::channel(json!({"state":"connecting"}));
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {biased;
                    _=cancel.cancelled()=>return,
                    value=auth.recv()=>{if value.as_ref().and_then(binding)!=Some(expected.clone()){cancel.cancel();return;}},
                    value=updates.recv()=>match value {
                        Some(value)=>{send.send_replace(value);},None=>{cancel.cancel();return;}
                    }
                }
            }
        });
        Ok(Arc::new(Self {
            target,
            closed,
            status,
            _watch: AbortOnDropHandle::new(task),
        }))
    }
}
#[async_trait]
impl super::remote_connection::Source for Source {
    fn blocked_error(&self, error: &RpcError) -> bool {
        matches!(error,RpcError::Failed(message) if message==zuse_engine::cloud::environment::PAUSED_MESSAGE || message==zuse_engine::cloud::environment::PAUSING_MESSAGE)
    }
    fn cancellation(&self) -> CancellationToken {
        self.closed.clone()
    }
    fn closed_message(&self) -> &'static str {
        "Cloud access ended. Return to the controller and select this workspace again."
    }
    fn controller(&self) -> Option<EngineHandle> {
        Some(self.target.controller.clone())
    }
    /// Account methods are forwarded to the controller.
    fn signs_in_on_this_machine(&self) -> bool {
        self.target.controller.inner.signs_in_on_this_machine()
    }
    fn cloud_workspace(&self) -> Option<String> {
        Some(self.target.workspace.clone())
    }
    fn initial_chat(&self) -> Option<String> {
        Some(self.target.initial_session.clone())
    }
    async fn connect(self: Arc<Self>) -> Result<(zuse_rpc::reconnect::Channel, String), RpcError> {
        let mut status = self.status.clone();
        let ready = async {
            loop {
                let value = status.borrow_and_update().clone();
                match value["state"].as_str() {
                    Some("ready") => {
                        return value["epoch"].as_u64().ok_or_else(|| {
                            RpcError::Failed("Invalid cloud connection epoch".into())
                        });
                    }
                    Some("blocked" | "failed" | "paused") => {
                        return Err(RpcError::Failed(
                            value["message"]
                                .as_str()
                                .unwrap_or("Cloud startup failed")
                                .into(),
                        ));
                    }
                    _ => {}
                }
                tokio::select! {biased;_=self.closed.cancelled()=>return Err(RpcError::Closed),result=status.changed()=>result.map_err(|_|RpcError::Closed)?}
            }
        };
        let epoch = tokio::time::timeout(std::time::Duration::from_secs(120), ready)
            .await
            .map_err(|_| {
                RpcError::Transport(
                    "Cloud startup timed out. Reopen the accepted workspace to check its state."
                        .into(),
                )
            })??;
        let (outgoing, mut requests) = mpsc::channel::<String>(16);
        let (replies, incoming) = mpsc::channel::<String>(16);
        let closed = self.closed.child_token();
        let stop = closed.clone();
        let source = self.clone();
        tokio::spawn(async move {
            let _closed = stop.clone().drop_guard();
            let mut tasks = HashMap::<u64, AbortOnDropHandle<()>>::new();
            loop {
                tokio::select! {biased;
                    _=stop.cancelled()=>return,
                    changed=status.changed()=>{
                        if changed.is_err(){return;}
                        let value=status.borrow_and_update().clone();
                        if value["state"]!="ready" || value["epoch"].as_u64()!=Some(epoch){return;}
                    },
                    request=requests.recv()=>{
                        let Some(request)=request else{return;};
                        for line in request.lines(){
                            let frame: zuse_rpc::ClientFrame=match serde_json::from_str(line){Ok(frame)=>frame,Err(_)=>continue};
                            tasks.retain(|_,task|!task.is_finished());
                            if frame.cancel {tasks.remove(&frame.id);continue;}
                            let id=frame.id;let reply=replies.clone();let controller=source.target.controller.clone();let workspace=source.target.workspace.clone();let subject=source.target.subject.clone();let organization=source.target.organization.clone();
                            tasks.insert(id,AbortOnDropHandle::new(tokio::spawn(async move{forward(controller,workspace,subject,organization,frame,reply).await;})));
                        }
                    }
                }
            }
        });
        Ok((
            zuse_rpc::reconnect::Channel {
                outgoing,
                incoming,
                closed,
            },
            format!("cloud:{}", self.target.workspace),
        ))
    }
}
async fn forward(
    controller: EngineHandle,
    workspace: String,
    subject: String,
    organization: Option<String>,
    frame: zuse_rpc::ClientFrame,
    replies: mpsc::Sender<String>,
) {
    let id = frame.id;
    let Some(method) = frame.method else {
        return;
    };
    // Authentication, account scope and cloud administration belong to the controller.
    let authority = AuthRpc::handles(&method)
        || matches!(
            method.as_str(),
            methods::CLOUD_ACCOUNT_STATUS
                | methods::CLOUD_ACCOUNT_SIGN_IN
                | methods::CLOUD_ACCOUNT_SIGN_OUT
        );
    let local_chat = frame.params["targetDeviceId"]
        .as_str()
        .and_then(|s| s.strip_prefix(crate::terminal::panel::LOCAL_MIRROR_PREFIX));
    let (method, params) = if let Some(chat) = local_chat {
        (
            methods::CLOUD_LOCAL_TERMINAL.into(),
            json!({"workspaceId":workspace,"expectedAccountId":subject,"expectedOrganizationId":organization,"chatId":chat,"requestId":id,"method":method,"params":frame.params}),
        )
    } else if authority {
        (method, frame.params)
    } else {
        (
            methods::CLOUD_RPC.into(),
            json!({"workspaceId":workspace,"expectedAccountId":subject,"expectedOrganizationId":organization,"logicalCommandId":uuid::Uuid::new_v4().to_string(),"requestId":id,"method":method,"params":frame.params}),
        )
    };
    let result = controller.client().forward(&method, params).await;
    let mut stream = match result {
        Ok(stream) => stream,
        Err(error) => {
            let frame = zuse_rpc::ServerFrame {
                id,
                err: Some(error.to_string()),
                ..Default::default()
            };
            let _ = replies.send(serde_json::to_string(&frame).unwrap()).await;
            return;
        }
    };
    while let Some(mut frame) = stream.recv().await {
        let wire = if authority {
            frame.id = id;
            serde_json::to_string(&frame)
        } else if let Some(value) = frame.item.take() {
            serde_json::to_string(&value)
        } else if let Some(error) = frame.err.take() {
            serde_json::to_string(&zuse_rpc::ServerFrame {
                id,
                err: Some(error),
                ..Default::default()
            })
        } else {
            continue;
        };
        if let Ok(wire) = wire {
            if replies.send(wire).await.is_err() {
                return;
            }
        }
    }
}
pub(super) async fn bootstrap(target: CloudTarget) -> anyhow::Result<EngineHandle> {
    super::remote_connection::bootstrap(Source::open(target).await?, None).await
}
