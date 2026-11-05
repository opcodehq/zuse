//! Shared interactive cloud transport. Cached projections never acquire one.
use super::*;
use futures::StreamExt;
use std::{
    collections::HashMap,
    sync::{
        Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
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
    resume_requested: Arc<AtomicBool>,
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
        self.get_scoped(account, None, workspace)
    }
    fn get_scoped(
        &self,
        account: &str,
        organization: Option<&str>,
        workspace: &str,
    ) -> Option<Arc<Environment>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(scope_key(account, organization), workspace.into()))
            .and_then(Weak::upgrade)
    }
    pub(super) fn retire_scoped(&self, account: &str, organization: Option<&str>, workspace: &str) {
        let environment = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(scope_key(account, organization), workspace.into()))
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
        interactive_resume: bool,
    ) -> Arc<Environment> {
        let mut entries = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| entry.strong_count() > 0);
        let organization = auth.state().org_id().map(str::to_owned);
        let key = (
            scope_key(&account, organization.as_deref()),
            workspace.clone(),
        );
        if let Some(old) = entries.get(&key).and_then(Weak::upgrade) {
            let state = old.status.borrow().state;
            if state == "ready" {
                old.resume_requested.store(false, Ordering::Release);
            }
            if state != "ready" {
                // A passive reopen may retry transport, but only an explicit
                // request may queue permission to resume compute.
                if interactive_resume {
                    old.resume_requested.store(true, Ordering::Release);
                }
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
        let resume_requested = Arc::new(AtomicBool::new(interactive_resume));
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
            organization,
            workspace,
            control.clone(),
            cancel.clone(),
            retry.clone(),
            resume_requested.clone(),
            status.clone(),
        ));
        let environment = Arc::new(Environment {
            client: Arc::new(client),
            control,
            cancel,
            retry,
            resume_requested,
            status: receiver,
            status_send: status,
            task,
        });
        entries.insert(key, Arc::downgrade(&environment));
        environment
    }
}
fn scope_key(account: &str, organization: Option<&str>) -> String {
    json!([account, organization]).to_string()
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
    organization: Option<&str>,
) -> Result<zuse_rpc::cloud::Grant, Failure> {
    if auth.user_id().as_deref() != Some(account) || auth.state().org_id() != organization {
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
    if auth.user_id().as_deref() != Some(account) || auth.state().org_id() != organization {
        return Err(failed(
            "blocked",
            "The controller account or organization changed. Select the workspace again.",
            false,
        ));
    }
    let value = api
        .request_scoped(
            &token,
            &["v1", "cloud", "workspaces", workspace],
            None,
            organization,
            &[],
        )
        .await
        .map_err(|e| failed("reconnecting", e.to_string(), true))?;
    if auth.user_id().as_deref() != Some(account) || auth.state().org_id() != organization {
        return Err(failed(
            "blocked",
            "The controller account or organization changed. Select the workspace again.",
            false,
        ));
    }
    let metadata = value.get("workspace").unwrap_or(&value);
    let diagnostics_ready = metadata["runtimeIngressReady"] == true;
    let row: Workspace = serde_json::from_value(metadata.clone())
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
    if let Some(message) = match row.status_code.as_str() {
        "repository_preparation_failed" => Some(
            "Repository syncing failed. Check repository access before retrying this computer.",
        ),
        "environment_setup_failed" => Some(
            "Environment setup failed. Review the project's Environment settings before starting a new task.",
        ),
        _ => None,
    } {
        return Err(failed("failed", message, false));
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
        api.request_scoped(
            &token,
            &["v1", "cloud", "workspaces", workspace, "resume"],
            Some(json!({"workspaceId":workspace,"commandId":resume})),
            organization,
            &[],
        )
        .await
        .map_err(|e| failed("reconnecting", e.to_string(), true))?;
        return Err(failed("starting", "Resuming cloud workspace…", true));
    }
    if row.state != "ready" && !diagnostics_ready {
        let message = match row.status_code.as_str() {
            "repository-syncing" => "Syncing repository…".to_owned(),
            "environment-setup" => "Preparing environment…".to_owned(),
            "allocating" | "provisioning-queued" => "Starting computer…".to_owned(),
            "runtime-starting" | "bootstrapping" => "Connecting runtime…".to_owned(),
            _ => "Preparing cloud computer…".to_owned(),
        };
        return Err(failed("starting", message, true));
    }
    let value=api.request_scoped(&token,&["v1","cloud","workspaces",workspace,"gateway","ticket"],Some(json!({"protocol":zuse_protocol::cloud_gateway::RUST_PROTOCOL,"rpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC})),organization,&[]).await.map_err(|e|failed("reconnecting",e.to_string(),true))?;
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
    if auth.user_id().as_deref() != Some(account) || auth.state().org_id() != organization {
        return Err(failed(
            "blocked",
            "The controller account or organization changed. Select the workspace again.",
            false,
        ));
    }
    Ok(grant)
}
async fn run(
    api: Arc<CloudApi>,
    auth: Auth,
    account: String,
    organization: Option<String>,
    workspace: String,
    control: Control,
    cancel: CancellationToken,
    retry: Arc<Notify>,
    resume_requested: Arc<AtomicBool>,
    status: watch::Sender<Status>,
) {
    let mut resume = uuid::Uuid::new_v4().to_string();
    let mut failures = 0u32;
    let mut interactive_resume = resume_requested.swap(false, Ordering::AcqRel);
    let mut changes = auth.watch_state();
    loop {
        if cancel.is_cancelled() {
            return;
        }
        let result = tokio::select! {biased;_=cancel.cancelled()=>return,result=grant(&api,&auth,&account,&workspace,&resume,interactive_resume,organization.as_deref())=>result};
        let failure = match result {
            Ok(grant) => {
                interactive_resume = false;
                resume_requested.store(false, Ordering::Release);
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
                            resume_requested.store(false, Ordering::Release);
                            loop {
                                tokio::select! {biased;
                                    _=cancel.cancelled()=>return,
                                    _=closed.cancelled()=>{resume_requested.store(false, Ordering::Release);break;},
                                    changed=changes.changed()=>{
                                        if changed.is_err()||auth.user_id().as_deref()!=Some(&account)||auth.state().org_id()!=organization.as_deref(){control.close();status.send_replace(Status{workspace_id:workspace.clone(),state:"blocked",epoch:control.generation(),message:"The controller account changed. Select the workspace again.".into()});return;}
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
            tokio::select! {biased;_=cancel.cancelled()=>return,_=retry.notified()=>{interactive_resume=resume_requested.swap(false, Ordering::AcqRel);if interactive_resume{resume=uuid::Uuid::new_v4().to_string();}},_=tokio::time::sleep(Duration::from_millis(if pending{500}else{(500u64<<failures.min(5)).min(10000)}))=>{}}
        } else {
            tokio::select! {biased;_=cancel.cancelled()=>return,_=retry.notified()=>{failures=0;interactive_resume=resume_requested.swap(false, Ordering::AcqRel);if interactive_resume{resume=uuid::Uuid::new_v4().to_string();}}}
        }
    }
}

pub async fn handle(auth: &Auth, method: &str, params: Value) -> Result<RpcReply, RpcError> {
    let api = CloudApi::shared().map_err(|e| RpcError::Failed(e.to_string()))?;
    handle_with_api(auth, method, params, api).await
}
pub(crate) async fn handle_with_api(
    auth: &Auth,
    method: &str,
    params: Value,
    api: Arc<CloudApi>,
) -> Result<RpcReply, RpcError> {
    if !auth.workos_enabled() && !api.loopback {
        return Err(RpcError::Failed(
            "Sign in to access cloud workspaces.".into(),
        ));
    }
    let account = auth
        .user_id()
        .ok_or_else(|| RpcError::Failed("Sign in to access cloud workspaces.".into()))?;
    let organization = auth.state().org_id().map(str::to_owned);
    if params["expectedOrganizationId"].as_str() != organization.as_deref()
        || params
            .get("expectedAccountId")
            .is_some_and(|v| v.as_str() != Some(account.as_str()))
        || (organization.is_some()
            && params["expectedAccountId"].as_str() != Some(account.as_str()))
    {
        return Err(RpcError::Failed(
            "The account or organization changed. Select the workspace again.".into(),
        ));
    }
    let workspace = params["workspaceId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512)
        .ok_or_else(|| RpcError::BadParams("Select a cloud workspace.".into()))?
        .to_owned();
    if method == zuse_protocol::methods::CLOUD_ENVIRONMENT {
        let interactive_resume = match params.get("interactiveResume") {
            None => true,
            Some(Value::Bool(value)) => *value,
            Some(_) => {
                return Err(RpcError::BadParams(
                    "interactiveResume must be a boolean.".into(),
                ));
            }
        };
        let environment = api.environments.open(
            api.clone(),
            auth.clone(),
            account,
            workspace,
            interactive_resume,
        );
        let stream = environment.updates();
        return Ok(RpcReply::Stream(stream));
    }
    let name = params["method"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or_else(|| RpcError::BadParams("Missing cloud RPC method.".into()))?;
    let id = params["requestId"]
        .as_u64()
        .ok_or_else(|| RpcError::BadParams("Missing cloud request ID.".into()))?;
    let mut command_params = params["params"].clone();
    if let (Some(logical), Some(object)) = (
        params["logicalCommandId"].as_str(),
        command_params.as_object_mut(),
    ) {
        object
            .entry("cloudCommandId")
            .or_insert_with(|| json!(logical));
    }
    if let Some(reply) = super::task_commands::maybe_forward(
        api.clone(),
        auth.clone(),
        account.clone(),
        organization.clone(),
        workspace.clone(),
        name,
        command_params,
        id,
    )
    .await?
    {
        return Ok(reply);
    }
    let environment = api
        .environments
        .get_scoped(&account, organization.as_deref(), &workspace)
        .ok_or_else(|| {
            RpcError::Failed("Select the cloud environment before using its tools.".into())
        })?;
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
    pause_with_api(auth, params, api).await
}
pub(crate) async fn pause_with_api(
    auth: &Auth,
    params: Value,
    api: Arc<CloudApi>,
) -> Result<Value> {
    ensure!(
        auth.workos_enabled() || api.loopback,
        "Sign in to access cloud workspaces."
    );
    let account = auth
        .user_id()
        .ok_or_else(|| anyhow::anyhow!("Sign in to access cloud workspaces."))?;
    let organization = auth.state().org_id().map(str::to_owned);
    ensure!(
        params["expectedAccountId"].as_str() == Some(account.as_str())
            && params["expectedOrganizationId"].as_str() == organization.as_deref(),
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
        auth.user_id().as_deref() == Some(account.as_str())
            && auth.state().org_id() == organization.as_deref(),
        "The account changed. Select the workspace again."
    );
    api.environments
        .retire_scoped(&account, organization.as_deref(), &workspace);
    tokio::spawn(async move {
        let value = api
            .request_scoped(
                &token,
                &["v1", "cloud", "workspaces", &workspace, "pause"],
                Some(json!({"workspaceId":workspace,"commandId":command})),
                organization.as_deref(),
                &[],
            )
            .await?;
        Ok(json!({"accountId":account,"organizationId":organization,"workspace":value}))
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
            resume_requested: Arc::new(AtomicBool::new(false)),
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
    async fn retained_connections_are_isolated_by_actor_and_organization() {
        let manager = Manager::default();
        let (client, control) = RpcClient::reconnecting();
        let (status_send, status) = watch::channel(Status {
            workspace_id: "same-computer".into(),
            state: "ready",
            epoch: 1,
            message: String::new(),
        });
        let environment = Arc::new(Environment {
            client: Arc::new(client),
            control,
            status,
            status_send,
            retry: Arc::new(Notify::new()),
            resume_requested: Arc::new(AtomicBool::new(false)),
            cancel: CancellationToken::new(),
            task: tokio::spawn(std::future::pending()),
        });
        manager.0.lock().unwrap().insert(
            (scope_key("actor", Some("org-a")), "same-computer".into()),
            Arc::downgrade(&environment),
        );
        assert!(
            manager
                .get_scoped("actor", Some("org-a"), "same-computer")
                .is_some()
        );
        assert!(manager.get("actor", "same-computer").is_none());
        assert!(
            manager
                .get_scoped("actor", Some("org-b"), "same-computer")
                .is_none()
        );
        assert!(
            manager
                .get_scoped("another-actor", Some("org-a"), "same-computer")
                .is_none()
        );
        manager.retire_scoped("actor", Some("org-b"), "same-computer");
        assert!(!environment.cancel.is_cancelled());
        manager.retire_scoped("actor", Some("org-a"), "same-computer");
        assert!(environment.cancel.is_cancelled());
        assert!(
            manager
                .get_scoped("actor", Some("org-a"), "same-computer")
                .is_none()
        );
    }
    #[tokio::test]
    async fn passive_ready_reopen_and_retry_never_resume_a_paused_computer() {
        use std::sync::atomic::AtomicUsize;
        for pause_at_ticket in [false, true] {
            let resumes = Arc::new(AtomicUsize::new(0));
            let tickets = Arc::new(AtomicUsize::new(0));
            let reads = Arc::new(AtomicUsize::new(0));
            let paused = Arc::new(AtomicBool::new(!pause_at_ticket));
            let ready = json!({"workspaceId":"passive-workspace","chatId":"thread","initialSessionId":"session","branch":"feature","state":"ready","desiredState":"ready","statusCode":"ready","revision":3,"rpcProtocol":"zuse-rpc-v1"});
            let history = ready.clone();
            let metadata = ready.clone();
            let metadata_paused = paused.clone();
            let read_count = reads.clone();
            let ticket_paused = paused.clone();
            let ticket_count = tickets.clone();
            let resume_count = resumes.clone();
            let app = Router::new()
                .route(
                    "/v1/cloud/workspaces/passive-workspace/history",
                    get(move || {
                        let row = history.clone();
                        async move { Json(json!({"workspace":row})) }
                    }),
                )
                .route(
                    "/v1/cloud/workspaces/passive-workspace",
                    get(move || {
                        let mut row = metadata.clone();
                        let paused = metadata_paused.clone();
                        let count = read_count.clone();
                        async move {
                            count.fetch_add(1, Ordering::SeqCst);
                            if paused.load(Ordering::SeqCst) {
                                row["state"] = json!("paused");
                                row["desiredState"] = json!("paused");
                                row["statusCode"] = json!("paused");
                            }
                            Json(row)
                        }
                    }),
                )
                .route(
                    "/v1/cloud/workspaces/passive-workspace/gateway/ticket",
                    post(move || {
                        let paused = ticket_paused.clone();
                        let count = ticket_count.clone();
                        async move {
                            count.fetch_add(1, Ordering::SeqCst);
                            paused.store(true, Ordering::SeqCst);
                            (
                                axum::http::StatusCode::CONFLICT,
                                Json(json!({"code":"workspace_paused"})),
                            )
                        }
                    }),
                )
                .route(
                    "/v1/cloud/workspaces/passive-workspace/resume",
                    post(move |Json(body): Json<Value>| {
                        let count = resume_count.clone();
                        async move {
                            assert_eq!(body["workspaceId"], "passive-workspace");
                            assert!(body["commandId"].is_string());
                            count.fetch_add(1, Ordering::SeqCst);
                            Json(json!({}))
                        }
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let _server = tokio_util::task::AbortOnDropHandle::new(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }));
            let data = tempfile::tempdir().unwrap();
            let mut config = crate::auth::AuthConfig::new(&origin, data.path());
            config.workos_client_id = None;
            let auth = Auth::new(config);
            let account = auth.user_id().unwrap();
            let api = Arc::new(CloudApi::new(&origin).unwrap());
            // The UI's authoritative history can be Ready immediately before a
            // Pause. Opening that saved thread is still a passive operation.
            let token = auth.access_token().await.unwrap();
            let head = api
                .request_scoped(
                    &token,
                    &["v1", "cloud", "workspaces", "passive-workspace", "history"],
                    None,
                    None,
                    &[],
                )
                .await
                .unwrap();
            assert_eq!(head["workspace"]["state"], "ready");
            let environment = api.environments.open(
                api.clone(),
                auth.clone(),
                account.clone(),
                "passive-workspace".into(),
                false,
            );
            let wait_paused = |environment: Arc<Environment>| async move {
                let mut status = environment.status.clone();
                tokio::time::timeout(Duration::from_secs(4), async {
                    loop {
                        if status.borrow_and_update().state == "paused" {
                            break;
                        }
                        status.changed().await.unwrap();
                    }
                })
                .await
                .expect("passive attachment settles Paused without lifecycle work");
            };
            wait_paused(environment.clone()).await;
            assert_eq!(resumes.load(Ordering::SeqCst), 0);
            assert_eq!(tickets.load(Ordering::SeqCst), usize::from(pause_at_ticket));
            let old_reads = reads.load(Ordering::SeqCst);
            let reopened = api.environments.open(
                api.clone(),
                auth.clone(),
                account.clone(),
                "passive-workspace".into(),
                false,
            );
            assert!(Arc::ptr_eq(&environment, &reopened));
            wait_paused(reopened.clone()).await;
            assert!(
                reads.load(Ordering::SeqCst) > old_reads,
                "passive retry refreshes metadata"
            );
            assert_eq!(
                resumes.load(Ordering::SeqCst),
                0,
                "retry Notify must never upgrade passive intent"
            );
            let explicit = api.environments.open(
                api.clone(),
                auth.clone(),
                account,
                "passive-workspace".into(),
                true,
            );
            let mut status = explicit.status.clone();
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if status.borrow_and_update().state == "starting" {
                        break;
                    }
                    status.changed().await.unwrap();
                }
            })
            .await
            .expect("explicit Resume still admits lifecycle work");
            assert_eq!(resumes.load(Ordering::SeqCst), 1);
            environment.cancel.cancel();
            environment.control.close();
        }
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
            None,
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
            None,
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

#[cfg(test)]
mod foreground_tests;
