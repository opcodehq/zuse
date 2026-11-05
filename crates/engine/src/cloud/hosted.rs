//! Account-isolated cloud controller. No EngineCore, local filesystem, desktop
//! terminal, local agent, or shared desktop account is exposed by this service.
use crate::{
    account::Account,
    cloud::{self, CloudApi, identity::CloudIdentity},
};
use futures::{StreamExt, stream::BoxStream};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio_util::sync::CancellationToken;
use zuse_protocol::methods;
use zuse_rpc::{RpcError, RpcReply, RpcService};

type Scope = (Option<String>, Option<String>);
pub struct Factory {
    api_origin: String,
    public_client_id: String,
    organizations: Arc<Vec<String>>,
    login_slots: Arc<Semaphore>,
    shared_requests: Arc<Semaphore>,
    shared_controls: Arc<Semaphore>,
    #[cfg(any(test, feature = "test-support"))]
    identity_override: Option<String>,
}
impl Factory {
    pub fn new(
        api_origin: &str,
        public_client_id: &str,
        organizations: Vec<String>,
    ) -> anyhow::Result<Self> {
        // Both constructors enforce the same credential destination constraints
        // used by the native account and cloud contracts.
        let _ = Account::ephemeral(api_origin, public_client_id)?;
        let _ = CloudApi::hosted(api_origin)?;
        anyhow::ensure!(
            organizations.len() <= 32
                && organizations.iter().all(|id| !id.is_empty()
                    && id.len() <= 128
                    && !id.chars().any(char::is_control)),
            "invalid hosted organization bounds"
        );
        Ok(Self {
            api_origin: api_origin.into(),
            public_client_id: public_client_id.into(),
            organizations: Arc::new(organizations),
            login_slots: Arc::new(Semaphore::new(4)),
            shared_requests: Arc::new(Semaphore::new(32)),
            shared_controls: Arc::new(Semaphore::new(4)),
            #[cfg(any(test, feature = "test-support"))]
            identity_override: None,
        })
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn fixture(
        api_origin: &str,
        identity_origin: &str,
        public_client_id: &str,
        organizations: Vec<String>,
    ) -> anyhow::Result<Self> {
        let _ = Account::ephemeral_fixture(api_origin, identity_origin, public_client_id)?;
        // Only this cfg-gated fixture may use loopback HTTP. The production
        // constructor still requires HTTPS and never accepts an identity override.
        let mut factory = Self::new("https://fixture.invalid", public_client_id, organizations)?;
        factory.api_origin = api_origin.into();
        factory.identity_override = Some(identity_origin.into());
        Ok(factory)
    }
}
impl zuse_serve::hosted::Factory for Factory {
    fn create(
        &self,
        public_controller_id: &str,
    ) -> anyhow::Result<zuse_serve::hosted::SessionService> {
        #[cfg(any(test, feature = "test-support"))]
        let account = match &self.identity_override {
            Some(identity) => {
                Account::ephemeral_fixture(&self.api_origin, identity, &self.public_client_id)?
            }
            None => Account::ephemeral(&self.api_origin, &self.public_client_id)?,
        };
        #[cfg(not(any(test, feature = "test-support")))]
        let account = Account::ephemeral(&self.api_origin, &self.public_client_id)?;
        let api = Arc::new(CloudApi::hosted(&self.api_origin)?);
        let (scope, receiver) = watch::channel((None, None));
        let auth = CloudIdentity::hosted(account.clone(), receiver);
        let closed = CancellationToken::new();
        let service = Arc::new(Controller {
            account: account.clone(),
            api,
            auth,
            scope,
            controller_id: public_controller_id.into(),
            organizations: self.organizations.clone(),
            requests: Arc::new(Semaphore::new(32)),
            login_slots: self.login_slots.clone(),
            shared_requests: self.shared_requests.clone(),
            shared_controls: self.shared_controls.clone(),
            control_requests: Arc::new(Semaphore::new(2)),
            last_login: std::sync::Mutex::new(None),
            closed: closed.clone(),
        });
        // A terminal account revocation closes this session's subscriptions; it
        // cannot log out or retire a different browser's account.
        let mut status = account.watch();
        let lifetime = closed.clone();
        tokio::spawn(async move {
            let mut had_user = false;
            loop {
                let signed_in = status.borrow_and_update().user.is_some();
                if had_user && !signed_in {
                    lifetime.cancel();
                    return;
                }
                had_user |= signed_in;
                tokio::select! { _ = lifetime.cancelled() => return, changed = status.changed() => if changed.is_err() { return; } }
            }
        });
        let retire = account.clone();
        let authenticated = account;
        Ok(zuse_serve::hosted::SessionService {
            rpc: service,
            retire: Arc::new(move || retire.retire()),
            authenticated: Arc::new(move || authenticated.status().user.is_some()),
            closed,
        })
    }
}
struct Controller {
    control_requests: Arc<Semaphore>,
    account: Account,
    api: Arc<CloudApi>,
    auth: CloudIdentity,
    scope: watch::Sender<Scope>,
    controller_id: String,
    organizations: Arc<Vec<String>>,
    requests: Arc<Semaphore>,
    login_slots: Arc<Semaphore>,
    shared_requests: Arc<Semaphore>,
    shared_controls: Arc<Semaphore>,
    last_login: std::sync::Mutex<Option<std::time::Instant>>,
    closed: CancellationToken,
}
impl Controller {
    fn current(&self) -> Scope {
        (
            self.auth.user_id(),
            self.auth.state().org_id().map(str::to_owned),
        )
    }
    fn bounded(
        &self,
        reply: RpcReply,
        permit: (OwnedSemaphorePermit, OwnedSemaphorePermit),
        scope: Option<Scope>,
    ) -> RpcReply {
        let until = {
            let closed = self.closed.clone();
            let auth = self.auth.clone();
            async move {
                if let Some(expected) = scope {
                    let mut changed = auth.watch_state();
                    loop {
                        if (auth.user_id(), auth.state().org_id().map(str::to_owned)) != expected {
                            return;
                        }
                        tokio::select! { _ = closed.cancelled() => return, result = changed.changed() => if result.is_err() { return; } }
                    }
                } else {
                    closed.cancelled().await;
                }
            }
        };
        fn guard<T: Send + 'static>(
            stream: BoxStream<'static, T>,
            permit: (OwnedSemaphorePermit, OwnedSemaphorePermit),
            until: impl std::future::Future<Output = ()> + Send + 'static,
        ) -> BoxStream<'static, T> {
            Box::pin(stream.take_until(until).map(move |value| {
                let _ = &permit;
                value
            }))
        }
        match reply {
            RpcReply::Value(value) => RpcReply::Value(value),
            RpcReply::Stream(stream) => RpcReply::Stream(guard(stream, permit, until)),
            RpcReply::TypedStream(stream) => RpcReply::TypedStream(guard(stream, permit, until)),
        }
    }
    async fn select_organization(&self, params: &Value) -> Result<RpcReply, RpcError> {
        let organization = match params.get("organizationId") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) if self.organizations.contains(id) => Some(id.clone()),
            _ => {
                return Err(RpcError::BadParams(
                    "Choose an available organization.".into(),
                ));
            }
        };
        let owner = self
            .auth
            .user_id()
            .ok_or_else(|| RpcError::Failed("Sign in to your Zuse account first.".into()))?;
        let credential = self
            .account
            .credential()
            .await
            .map_err(|e| RpcError::Failed(e.to_string()))?;
        // The existing API's membership and scope-admission gates remain the
        // authority. A failed check leaves the prior selection intact.
        self.api
            .request_scoped(
                &credential.access_token,
                &["v1", "cloud", "context"],
                None,
                organization.as_deref(),
                &[],
            )
            .await
            .map_err(cloud::rpc_error)?;
        if self.auth.user_id().as_deref() != Some(&owner) {
            return Err(RpcError::Capability("workspace_access_denied".into()));
        }
        self.scope.send_replace((Some(owner), organization));
        RpcReply::value(&self.auth.state())
    }
}
#[async_trait::async_trait]
impl RpcService for Controller {
    async fn handle(&self, method: &str, mut params: Value) -> Result<RpcReply, RpcError> {
        if self.closed.is_cancelled() {
            return Err(RpcError::Closed);
        }
        // Data streams keep their permits for their full lifetime. A separate
        // small lane keeps Stop/queue and sign-out reachable under that load.
        let control = matches!(
            method,
            methods::AUTH_STATUS
                | methods::CLOUD_ACCOUNT_SIGN_IN
                | methods::CLOUD_ACCOUNT_SIGN_OUT
                | methods::SELECT_ORG
                | methods::CLOUD_TASK_COMMAND
                | methods::CLOUD_PAUSE
                | methods::CLOUD_DELETE
        );
        let (local, shared) = if control {
            (&self.control_requests, &self.shared_controls)
        } else {
            (&self.requests, &self.shared_requests)
        };
        let permit = local.clone().try_acquire_owned().map_err(|_| {
            RpcError::Failed(
                "Too many active browser requests. Close an unused conversation or retry shortly."
                    .into(),
            )
        })?;
        let global = shared.clone().try_acquire_owned().map_err(|_| {
            RpcError::Failed(
                "The cloud controller is busy. Close an unused conversation or retry shortly."
                    .into(),
            )
        })?;
        let permit = (permit, global);
        let before = self.current();
        let mut scoped = false;
        let reply = match method {
            methods::LOCAL_DEVICE | methods::ENGINE_INFO => RpcReply::value(&json!({"deviceId":self.controller_id,"workspaceScope":"local","capabilities":["cloud-controller-v1"]}))?,
            methods::AUTH_STATUS => RpcReply::value(&self.auth.state())?,
            methods::CLOUD_ACCOUNT_STATUS => RpcReply::Stream(self.auth.status_stream()),
            methods::CLOUD_ACCOUNT_SIGN_IN => {
                if before.0.is_some() { return Err(RpcError::BadParams("Sign out before signing in with another account.".into())); }
                let _slot = self.login_slots.clone().try_acquire_owned().map_err(|_| RpcError::Failed("Sign-in is busy. Retry shortly.".into()))?;
                {
                    let mut last = self.last_login.lock().map_err(|_| RpcError::Failed("Sign-in is unavailable.".into()))?;
                    if last.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(15)) { return Err(RpcError::Failed("A sign-in was just started. Use its verification link before starting another.".into())); }
                    *last = Some(std::time::Instant::now());
                }
                RpcReply::value(&self.account.sign_in().await.map_err(|e| RpcError::Failed(e.to_string()))?)?
            }
            methods::CLOUD_ACCOUNT_SIGN_OUT => { self.account.sign_out().map_err(|e| RpcError::Failed(e.to_string()))?; self.closed.cancel(); RpcReply::value(&json!({"ok":true}))? }
            methods::SELECT_ORG => return self.select_organization(&params).await,
            // This controller owns no local projects, agents or device registry.
            methods::WATCH_CHATS | methods::WATCH_SESSIONS | methods::WATCH_SPACES | methods::WATCH_DEVICES | methods::LIST_REPOS | methods::LIST_AGENT_ACCOUNTS | methods::LIST_HARNESSES | methods::LIST_MODELS | methods::LIST_PAIRINGS | methods::WATCH_HARNESS_UPDATES => RpcReply::value(&json!([]))?,
            methods::GET_TITLE_SETTINGS | methods::GET_WORKTREE_SETTINGS => RpcReply::value(&json!({"enabled":false}))?,
            methods::BROWSER_ACCESS_STATUS | methods::RELAY_ACCESS_STATUS => RpcReply::value(&json!({"running":false,"available":false}))?,
            methods::PLUGINS => RpcReply::value(&json!({"plugins":[],"available":false}))?,
            _ if allowed(method) => {
                if before.0.is_none() { return Err(RpcError::Failed("Sign in to your Zuse account first.".into())); }
                if params.get("targetDeviceId").is_some_and(|v| !v.is_null()) { return Err(RpcError::BadParams("Choose a cloud conversation on this controller.".into())); }
                scoped = true;
                match method {
                    methods::CLOUD_EVENTS => cloud::events::handle_with_api(&self.auth, params, self.api.clone()).await?,
                    methods::CLOUD_ENVIRONMENT | methods::CLOUD_RPC => {
                        if method == methods::CLOUD_ENVIRONMENT && params.get("interactiveResume").is_none() { params["interactiveResume"] = json!(false); }
                        cloud::environment::handle_with_api(&self.auth, method, params, self.api.clone()).await?
                    }
                    methods::CLOUD_PAUSE => RpcReply::value(&cloud::environment::pause_with_api(&self.auth, params, self.api.clone()).await.map_err(cloud::rpc_error)?)?,
                    _ => {
                        if method == methods::CLOUD_CREATE && params["attachmentTransfers"].as_array().is_some_and(|v| !v.is_empty()) { return Err(RpcError::BadParams("Upload attachments directly to the selected cloud computer; local file transfers are unavailable on this controller.".into())); }
                        RpcReply::value(&self.api.handle(&self.auth, method, params).await.map_err(cloud::rpc_error)?)?
                    }
                }
            }
            _ => return Err(RpcError::BadParams("This controller supports cloud conversations only. Choose a cloud project; local files, terminals and local agent commands are unavailable.".into())),
        };
        if scoped && self.current() != before {
            return Err(RpcError::Capability("workspace_access_denied".into()));
        }
        Ok(self.bounded(reply, permit, scoped.then_some(before)))
    }
}
fn allowed(method: &str) -> bool {
    matches!(
        method,
        methods::CLOUD_CONTEXT
            | methods::CLOUD_WORKSPACES
            | methods::CLOUD_WORKSPACE
            | methods::CLOUD_HISTORY
            | methods::CLOUD_EVENTS
            | methods::CLOUD_ENVIRONMENT
            | methods::CLOUD_RPC
            | methods::CLOUD_PAUSE
            | methods::CLOUD_CREATE
            | methods::CLOUD_DELETE
            | methods::CLOUD_UNARCHIVE
            | methods::CLOUD_TASK_COMMAND
            | methods::CLOUD_TASK_ASSET
            | methods::CLOUD_COMPUTER_RECREATION
            | methods::CLOUD_ALLOCATION_RETRY
            | methods::CLOUD_RUNTIME_REPAIR
            | methods::CLOUD_PROJECTS
            | methods::CLOUD_PROJECT_CONNECT
            | methods::CLOUD_PROJECT_REMOVE
            | methods::CLOUD_PROJECT_ENVIRONMENT
            | methods::CLOUD_PROJECT_ENVIRONMENT_SAVE
            | methods::CLOUD_PROJECT_SECRETS
            | methods::CLOUD_PROJECT_SECRET_SAVE
            | methods::CLOUD_PROJECT_SECRET_REVOKE
            | methods::CLOUD_GITHUB
            | methods::CLOUD_GITHUB_INSTALL
            | methods::CLOUD_CAPABILITIES
            | methods::CLOUD_AGENT_CONNECTIONS
            | methods::CLOUD_AGENT_CONFIGURE
            | methods::CLOUD_AGENT_LOGIN
            | methods::CLOUD_AGENT_LOGIN_STATUS
            | methods::CLOUD_AGENT_LOGIN_CANCEL
            | methods::CLOUD_AGENT_DISCONNECT
            | methods::CLOUD_BILLING_SUMMARY
            | methods::CLOUD_BILLING_USAGE
            | methods::CLOUD_BILLING_CAP
            | methods::CLOUD_PREPAID_BALANCE
            | methods::CLOUD_PREPAID_CHECKOUT
            | methods::CLOUD_BILLING_ENTITLEMENTS
            | methods::CLOUD_BILLING_CHECKOUT
            | methods::CLOUD_BILLING_PORTAL
    )
}

#[cfg(test)]
#[path = "hosted_tests.rs"]
mod hosted_tests;
