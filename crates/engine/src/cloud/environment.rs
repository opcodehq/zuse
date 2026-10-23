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

pub const PAUSED_MESSAGE: &str = "Cloud workspace is paused. Connect to resume.";
pub const PAUSING_MESSAGE: &str =
    "Cloud workspace is pausing. Open saved history to check its state.";

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
    status_send: watch::Sender<Status>,
    retry: Arc<Notify>,
    cancel: CancellationToken,
    control: Control,
    task: tokio::task::JoinHandle<()>,
}
impl Environment {
    fn updates(self: &Arc<Self>) -> futures::stream::BoxStream<'static, Value> {
        let receiver = self.status.clone();
        futures::stream::unfold(
            (self.clone(), receiver, true),
            |(environment, mut receiver, first)| async move {
                if !first && receiver.changed().await.is_err() {
                    return None;
                }
                // A retained connection can already have an unseen ready value.
                // Consume that version with the initial snapshot as well.
                let value = serde_json::to_value(receiver.borrow_and_update().clone()).ok()?;
                Some((value, (environment, receiver, false)))
            },
        )
        .boxed()
    }
    pub(super) fn ready_client(&self) -> Option<Arc<RpcClient>> {
        (self.status.borrow().state == "ready").then(|| self.client.clone())
    }
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
    pub(super) fn get(&self, account: &str, workspace: &str) -> Option<Arc<Environment>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(account.into(), workspace.into()))
            .and_then(Weak::upgrade)
    }
    fn retire(&self, account: &str, workspace: &str) {
        let environment = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(account.into(), workspace.into()))
            .and_then(|v| v.upgrade());
        if let Some(environment) = environment {
            environment.status_send.send_replace(Status {
                workspace_id: workspace.into(),
                state: "blocked",
                epoch: environment.control.generation(),
                message: PAUSING_MESSAGE.into(),
            });
            environment.cancel.cancel();
            environment.control.close();
            environment.task.abort();
        }
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
            let state = old.status.borrow().state;
            if matches!(state, "paused" | "blocked" | "failed" | "reconnecting") {
                old.status_send.send_replace(Status {
                    workspace_id: workspace.clone(),
                    state: "connecting",
                    epoch: old.control.generation(),
                    message: "Connecting to cloud workspace…".into(),
                });
                old.retry.notify_one();
            }
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
            status.clone(),
        ));
        let environment = Arc::new(Environment {
            client: Arc::new(client),
            control,
            cancel,
            retry,
            status: receiver,
            status_send: status,
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
    interactive_resume: bool,
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
        if !interactive_resume {
            return Err(failed("paused", PAUSED_MESSAGE, false));
        }
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
    let mut resume = uuid::Uuid::new_v4().to_string();
    let mut failures = 0u32;
    let mut interactive_resume = true;
    let mut changes = auth.watch_state();
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let result = tokio::select! {biased;_=cancel.cancelled()=>return,result=grant(&api,&auth,&account,&workspace,&resume,interactive_resume)=>result};
        let failure = match result {
            Ok(grant) => {
                interactive_resume = false;
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
            state: if retryable || failure.state == "paused" {
                failure.state
            } else {
                "blocked"
            },
            epoch: control.generation(),
            message: failure.message,
        });
        if retryable {
            tokio::select! {biased;_=cancel.cancelled()=>return,_=retry.notified()=>{interactive_resume=true;resume=uuid::Uuid::new_v4().to_string();},_=tokio::time::sleep(Duration::from_millis(if pending{500}else{(500u64<<failures.min(5)).min(10000)}))=>{}}
        } else {
            tokio::select! {biased;_=cancel.cancelled()=>return,_=retry.notified()=>{failures=0;interactive_resume=true;resume=uuid::Uuid::new_v4().to_string();}}
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
        let stream = environment.updates();
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

/// Pause is admitted on the controller and outlives a disappearing UI waiter.
pub async fn pause(auth: &Auth, params: Value) -> Result<Value> {
    let api = CloudApi::shared()?;
    ensure!(
        auth.workos_enabled() || api.loopback,
        "Sign in to access cloud workspaces."
    );
    let account = auth
        .user_id()
        .ok_or_else(|| anyhow::anyhow!("Sign in to access cloud workspaces."))?;
    ensure!(
        params["expectedAccountId"].as_str() == Some(account.as_str())
            && auth.state().org_id().is_none(),
        "The account changed. Select the workspace again."
    );
    let workspace = params["workspaceId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512)
        .ok_or_else(|| anyhow::anyhow!("Select a workspace."))?
        .to_owned();
    let command = params["commandId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or_else(|| anyhow::anyhow!("Pause request is missing."))?
        .to_owned();
    let token = auth
        .access_token()
        .await
        .map_err(|_| anyhow::anyhow!("Sign in to access cloud workspaces."))?;
    ensure!(
        auth.user_id().as_deref() == Some(account.as_str()) && auth.state().org_id().is_none(),
        "The account changed. Select the workspace again."
    );
    api.environments.retire(&account, &workspace);
    tokio::spawn(async move {
        let value = api
            .request(
                &token,
                &["v1", "cloud", "workspaces", &workspace, "pause"],
                Some(json!({"workspaceId":workspace,"commandId":command})),
            )
            .await?;
        Ok(json!({"accountId":account,"workspace":value}))
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!("Pause was interrupted. Retry the same request to check its state.")
    })?
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    #[tokio::test]
    async fn retained_ready_connection_has_one_initial_status_then_real_changes() {
        let (client, control) = RpcClient::reconnecting();
        let (send, status) = watch::channel(Status {
            workspace_id: "retained".into(),
            state: "connecting",
            epoch: 0,
            message: String::new(),
        });
        send.send_replace(Status {
            workspace_id: "retained".into(),
            state: "ready",
            epoch: 1,
            message: String::new(),
        });
        let environment = Arc::new(Environment {
            client: Arc::new(client),
            control,
            status,
            status_send: send.clone(),
            retry: Arc::new(Notify::new()),
            cancel: CancellationToken::new(),
            task: tokio::spawn(std::future::pending()),
        });
        let mut stream = environment.updates();
        assert_eq!(stream.next().await.unwrap()["state"], "ready");
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), stream.next())
                .await
                .is_err()
        );
        send.send_replace(Status {
            workspace_id: "retained".into(),
            state: "blocked",
            epoch: 1,
            message: PAUSING_MESSAGE.into(),
        });
        assert_eq!(stream.next().await.unwrap()["state"], "blocked");
    }
    #[tokio::test]
    async fn paused_workspace_requires_interactive_resume() {
        let resumes = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let captured = resumes.clone();
        let row = json!({"workspaceId":"paused-workspace","chatId":"thread","initialSessionId":"session","branch":"feature","state":"paused","desiredState":"paused","statusCode":"paused","revision":3,"rpcProtocol":"zuse-rpc-v1"});
        let app = Router::new()
            .route(
                "/v1/cloud/workspaces/paused-workspace",
                get(move || {
                    let row = row.clone();
                    async move { Json(row) }
                }),
            )
            .route(
                "/v1/cloud/workspaces/paused-workspace/resume",
                post(move |Json(body): Json<Value>| {
                    let captured = captured.clone();
                    async move {
                        captured.lock().unwrap().push(body);
                        Json(json!({}))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let data = tempfile::tempdir().unwrap();
        let mut config = crate::auth::AuthConfig::new(&origin, data.path());
        config.workos_client_id = None;
        let auth = Auth::new(config);
        let account = auth.user_id().unwrap();
        let api = CloudApi::new(&origin).unwrap();
        let result = grant(
            &api,
            &auth,
            &account,
            "paused-workspace",
            "resume-once",
            false,
        )
        .await;
        assert!(matches!(
            result,
            Err(Failure {
                state: "paused",
                retry: false,
                ..
            })
        ));
        assert!(
            resumes.lock().unwrap().is_empty(),
            "A background reconnect cannot wake compute"
        );
        let result = grant(
            &api,
            &auth,
            &account,
            "paused-workspace",
            "resume-once",
            true,
        )
        .await;
        assert!(matches!(
            result,
            Err(Failure {
                state: "starting",
                ..
            })
        ));
        assert_eq!(
            *resumes.lock().unwrap(),
            vec![json!({"workspaceId":"paused-workspace","commandId":"resume-once"})]
        );
        server.abort();
    }
}
