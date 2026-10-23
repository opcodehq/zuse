//! Shared interactive cloud transport. Cached projections never acquire one.
use super::*;
use futures::StreamExt;
use std::{
    collections::HashMap,
    sync::{Mutex, Weak},
};
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;
use zuse_rpc::{RpcClient, RpcError, RpcReply, reconnect::Control};

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    workspace_id: String,
    state: &'static str,
    epoch: u64,
    message: String,
}
pub struct Environment {
    client: Arc<RpcClient>,
    status: watch::Receiver<Status>,
    retry: Arc<Notify>,
    cancel: CancellationToken,
    control: Control,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Environment {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.control.close();
        self.task.abort();
    }
}
#[derive(Default)]
pub struct Manager(Mutex<HashMap<(String, String), Weak<Environment>>>);
impl Manager {
    fn get(&self, account: &str, workspace: &str) -> Option<Arc<Environment>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(account.into(), workspace.into()))
            .and_then(Weak::upgrade)
    }
    fn open(
        &self,
        api: Arc<CloudApi>,
        auth: Auth,
        account: String,
        workspace: String,
    ) -> Arc<Environment> {
        let mut entries = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| entry.strong_count() > 0);
        let key = (account.clone(), workspace.clone());
        if let Some(old) = entries.get(&key).and_then(Weak::upgrade) {
            old.retry.notify_one();
            return old;
        }
        let (client, control) = RpcClient::reconnecting();
        let cancel = CancellationToken::new();
        let retry = Arc::new(Notify::new());
        let (status, receiver) = watch::channel(Status {
            workspace_id: workspace.clone(),
            state: "connecting",
            epoch: 0,
            message: "Connecting to cloud workspace…".into(),
        });
        let task = tokio::spawn(run(
            api,
            auth,
            account,
            workspace,
            control.clone(),
            cancel.clone(),
            retry.clone(),
            status,
        ));
        let environment = Arc::new(Environment {
            client: Arc::new(client),
            control,
            cancel,
            retry,
            status: receiver,
            task,
        });
        entries.insert(key, Arc::downgrade(&environment));
        environment
    }
}
struct Failure {
    state: &'static str,
    message: String,
    retry: bool,
}
fn failed(state: &'static str, message: impl Into<String>, retry: bool) -> Failure {
    Failure {
        state,
        message: message.into(),
        retry,
    }
}
async fn grant(
    api: &CloudApi,
    auth: &Auth,
    account: &str,
    workspace: &str,
    resume: &str,
) -> Result<zuse_rpc::cloud::Grant, Failure> {
    if auth.user_id().as_deref() != Some(account) {
        return Err(failed(
            "blocked",
            "The controller account changed. Select the workspace again.",
            false,
        ));
    }
    let token = auth
        .access_token()
        .await
        .map_err(|_| failed("blocked", "Sign in to connect to cloud workspaces.", false))?;
    let value = api
        .request(&token, &["v1", "cloud", "workspaces", workspace], None)
        .await
        .map_err(|e| failed("reconnecting", e.to_string(), true))?;
    let row: Workspace =
        serde_json::from_value(value.get("workspace").cloned().unwrap_or(value))
            .map_err(|_| failed("blocked", "Invalid cloud workspace metadata.", false))?;
    if row.rpc_protocol.as_deref() != Some(zuse_protocol::cloud_gateway::RUST_RPC) {
        return Err(failed(
            "blocked",
            "This workspace requires a runtime update before connecting.",
            false,
        ));
    }
    if row.archived()
        || matches!(row.state.as_str(), "deleted" | "deleting")
        || row.desired_state == "deleted"
    {
        return Err(failed(
            "blocked",
            "Unarchive this thread before connecting.",
            false,
        ));
    }
    if row.state == "failed" {
        return Err(failed(
            "failed",
            format!(
                "Cloud startup failed ({}). Retry after recovering the workspace.",
                row.status_code
            ),
            false,
        ));
    }
    if row.state == "paused" || row.desired_state == "paused" {
        api.request(
            &token,
            &["v1", "cloud", "workspaces", workspace, "resume"],
            Some(json!({"workspaceId":workspace,"commandId":resume})),
        )
        .await
        .map_err(|e| failed("reconnecting", e.to_string(), true))?;
        return Err(failed("starting", "Resuming cloud workspace…", true));
    }
    if row.state != "ready" {
        return Err(failed(
            "starting",
            format!("Starting cloud workspace · {}", row.status_code),
            true,
        ));
    }
    let value=api.request(&token,&["v1","cloud","workspaces",workspace,"gateway","ticket"],Some(json!({"protocol":zuse_protocol::cloud_gateway::RUST_PROTOCOL,"rpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC}))).await.map_err(|e|failed("reconnecting",e.to_string(),true))?;
    let grant: zuse_rpc::cloud::Grant = serde_json::from_value(value)
        .map_err(|_| failed("blocked", "Invalid cloud connection authorization.", false))?;
    let mut expected = api.origin.clone();
    let scheme = if expected.scheme() == "https" {
        "wss"
    } else {
        "ws"
    };
    expected
        .set_scheme(scheme)
        .map_err(|_| failed("blocked", "Invalid cloud connection origin.", false))?;
    expected
        .path_segments_mut()
        .map_err(|_| failed("blocked", "Invalid cloud connection origin.", false))?
        .clear()
        .extend(["v1", "cloud", "workspaces", workspace, "gateway"]);
    if grant.workspace_id != workspace || grant.ws_url != expected.as_str() {
        return Err(failed(
            "blocked",
            "Cloud connection authorization has the wrong destination.",
            false,
        ));
    }
    Ok(grant)
}
async fn run(
    api: Arc<CloudApi>,
    auth: Auth,
    account: String,
    workspace: String,
    control: Control,
    cancel: CancellationToken,
    retry: Arc<Notify>,
    status: watch::Sender<Status>,
) {
    let resume = uuid::Uuid::new_v4().to_string();
    let mut failures = 0u32;
    let mut changes = auth.watch_state();
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let result = tokio::select! {biased;_=cancel.cancelled()=>return,result=grant(&api,&auth,&account,&workspace,&resume)=>result};
        let failure = match result {
            Ok(grant) => {
                let channel =
                    zuse_rpc::cloud::connect_channel(grant, &workspace, cancel.child_token()).await;
                match channel {
                    Ok(channel) => match control.attach(channel) {
                        Ok(closed) => {
                            failures = 0;
                            status.send_replace(Status {
                                workspace_id: workspace.clone(),
                                state: "ready",
                                epoch: control.generation(),
                                message: String::new(),
                            });
                            loop {
                                tokio::select! {biased;
                                    _=cancel.cancelled()=>return,
                                    _=closed.cancelled()=>break,
                                    changed=changes.changed()=>{
                                        if changed.is_err()||auth.user_id().as_deref()!=Some(&account){control.close();status.send_replace(Status{workspace_id:workspace.clone(),state:"blocked",epoch:control.generation(),message:"The controller account changed. Select the workspace again.".into()});return;}
                                    }
                                }
                            }
                            failed(
                                "reconnecting",
                                "Cloud connection interrupted. Accepted work continues in the runtime.",
                                true,
                            )
                        }
                        Err(_) => return,
                    },
                    Err(e) => failed("reconnecting", e.to_string(), true),
                }
            }
            Err(error) => error,
        };
        let pending = failure.state == "starting";
        if !pending {
            failures = failures.saturating_add(1);
        }
        let retryable = failure.retry && failures < 8;
        status.send_replace(Status {
            workspace_id: workspace.clone(),
            state: if retryable { failure.state } else { "blocked" },
            epoch: control.generation(),
            message: failure.message,
        });
        if retryable {
            tokio::select! {biased;_=cancel.cancelled()=>return,_=retry.notified()=>{},_=tokio::time::sleep(Duration::from_millis(if pending{500}else{(500u64<<failures.min(5)).min(10000)}))=>{}}
        } else {
            tokio::select! {biased;_=cancel.cancelled()=>return,_=retry.notified()=>{failures=0;}}
        }
    }
}

pub async fn handle(auth: &Auth, method: &str, params: Value) -> Result<RpcReply, RpcError> {
    let api = CloudApi::shared().map_err(|e| RpcError::Failed(e.to_string()))?;
    if !auth.workos_enabled() && !api.loopback {
        return Err(RpcError::Failed(
            "Sign in to access cloud workspaces.".into(),
        ));
    }
    let account = auth
        .user_id()
        .ok_or_else(|| RpcError::Failed("Sign in to access cloud workspaces.".into()))?;
    let workspace = params["workspaceId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512)
        .ok_or_else(|| RpcError::BadParams("Select a cloud workspace.".into()))?
        .to_owned();
    if method == zuse_protocol::methods::CLOUD_ENVIRONMENT {
        let environment = api
            .environments
            .open(api.clone(), auth.clone(), account, workspace);
        let receiver = environment.status.clone();
        let stream = futures::stream::unfold(
            (environment, receiver, true),
            |(environment, mut receiver, first)| async move {
                if !first && receiver.changed().await.is_err() {
                    return None;
                }
                let value = serde_json::to_value(receiver.borrow().clone()).ok()?;
                Some((value, (environment, receiver, false)))
            },
        )
        .boxed();
        return Ok(RpcReply::Stream(stream));
    }
    let environment = api.environments.get(&account, &workspace).ok_or_else(|| {
        RpcError::Failed("Select the cloud environment before using its tools.".into())
    })?;
    let name = params["method"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or_else(|| RpcError::BadParams("Missing cloud RPC method.".into()))?;
    let id = params["requestId"]
        .as_u64()
        .ok_or_else(|| RpcError::BadParams("Missing cloud request ID.".into()))?;
    let forward = environment
        .client
        .forward(name, params["params"].clone())
        .await?;
    let stream = futures::stream::unfold(
        (environment, forward),
        move |(environment, mut forward)| async move {
            let mut frame = forward.recv().await?;
            frame.id = id;
            Some((serde_json::to_value(frame).ok()?, (environment, forward)))
        },
    )
    .boxed();
    Ok(RpcReply::Stream(stream))
}
