//! Connection setup shared by native and web. Secrets are cleared after submission.
use super::agent_login_binding::Binding;
use super::presentation::MenuItem;
use super::*;
use crate::cloud_settings::kit;
use crate::tooltip::TooltipExt as _;
use zuse_protocol::{cloud_computers::AgentId, cloud_connections::*};

pub(super) struct AgentView {
    connections: Vec<AgentConnection>,
    edit: Option<(AgentId, ConnectionMethod)>,
    operation: Option<LoginOperation>,
    selection: Option<Binding>,
    managed_connection: Option<String>,
    supports_reconnect: bool,
    supports_reconfigure: bool,
    recovery_target: Option<(Owner, String, String)>,
    recovery_open: bool,
    login_binding: Option<Binding>,
    secret: Entity<TextInput>,
    pub error: Option<String>,
    pub poll: Option<Task<()>>,
    polls: u32,
    /// Connections hidden while their disconnect is in flight, so the row
    /// leaves at once and only returns if the server refuses.
    disconnecting: BTreeSet<String>,
    /// The idempotency key of a sign-in cancelled before its code arrived;
    /// the late reply is cancelled instead of reopening the row.
    abandoned: Option<String>,
    /// The device code was just copied: the copy button shows a check.
    copied: Option<Task<()>>,
    _submit: Subscription,
}
impl AgentView {
    /// Any connection the guide can treat as a usable coding agent.
    pub(super) fn any_connected(&self) -> bool {
        self.connections
            .iter()
            .any(|c| c.state == zuse_protocol::cloud_connections::ConnectionState::Connected)
    }
    /// Connected agents for the Overview, e.g. "Codex · you@example.com".
    pub(super) fn connected_summary(&self) -> Vec<String> {
        self.connections
            .iter()
            .filter(|c| c.state == ConnectionState::Connected)
            .map(|c| {
                let name = match c.agent {
                    AgentId::Claude => "Claude",
                    AgentId::Codex => "Codex",
                    AgentId::Grok => "Grok",
                };
                match c.account_label.as_deref().filter(|l| !l.is_empty()) {
                    Some(label) => format!("{name} · {label}"),
                    None => name.to_owned(),
                }
            })
            .collect()
    }
    pub fn new(window: &mut Window, cx: &mut Context<RepositoryPanel>) -> Self {
        let secret = cx.new(|cx| TextInput::password("Paste your credential", window, cx));
        cx.observe(&secret, |_, _, cx| cx.notify()).detach();
        let submit = cx.subscribe(&secret, |this, _, _: &crate::input::Submitted, cx| {
            this.submit_agent(cx)
        });
        Self {
            connections: vec![],
            edit: None,
            operation: None,
            selection: None,
            managed_connection: None,
            supports_reconnect: false,
            supports_reconfigure: false,
            recovery_target: None,
            recovery_open: false,
            login_binding: None,
            secret,
            error: None,
            poll: None,
            polls: 0,
            disconnecting: BTreeSet::new(),
            abandoned: None,
            copied: None,
            _submit: submit,
        }
    }
    pub fn reset(&mut self, cx: &mut Context<RepositoryPanel>) {
        self.connections.clear();
        self.edit = None;
        self.operation = None;
        self.selection = None;
        self.managed_connection = None;
        self.supports_reconnect = false;
        self.supports_reconfigure = false;
        self.recovery_target = None;
        self.recovery_open = false;
        self.login_binding = None;
        self.error = None;
        self.poll = None;
        self.polls = 0;
        self.disconnecting.clear();
        self.abandoned = None;
        self.copied = None;
        self.secret
            .update(cx, |input, cx| input.set_value(String::new(), cx));
    }
}
impl AgentView {
    /// Closes the inline connect form without touching any sign-in state.
    fn close_edit(&mut self, cx: &mut Context<RepositoryPanel>) {
        self.edit = None;
        self.selection = None;
        self.managed_connection = None;
        self.recovery_open = false;
        self.recovery_target = None;
        self.copied = None;
        self.secret
            .update(cx, |input, cx| input.set_value(String::new(), cx));
    }
    /// Optimistically records a finished sign-in as a connected account.
    fn mark_connected(&mut self, operation: &LoginOperation) {
        if let Some(row) = self
            .connections
            .iter_mut()
            .find(|c| c.connection_id == operation.connection_id)
        {
            row.state = ConnectionState::Connected;
            row.error_code = None;
            return;
        }
        let method = self
            .edit
            .filter(|(agent, _)| *agent == operation.agent)
            .map(|(_, method)| method)
            .unwrap_or(ConnectionMethod::DeviceCode);
        self.connections.push(AgentConnection {
            connection_id: operation.connection_id.clone(),
            agent: operation.agent,
            method,
            state: ConnectionState::Connected,
            account_label: None,
            position: self.connections.len().try_into().unwrap_or(i32::MAX),
            enabled: true,
            alias: None,
            expires_at: None,
            version: 0,
            error_code: None,
            pending_login_operation_id: None,
            credential_update: None,
            credential_update_key: None,
        });
    }
    /// A request failed: undo what the row showed optimistically.
    pub(super) fn request_failed(&mut self, kind: &str) {
        match kind {
            "agent-disconnect" => self.disconnecting.clear(),
            "agent-login" if self.abandoned.take().is_some() => {
                self.login_binding = None;
                self.operation = None;
            }
            _ => {}
        }
    }
}
impl RepositoryPanel {
    pub(crate) fn focus_agent_connection(
        &mut self,
        owner: Owner,
        agent: String,
        connection: String,
        cx: &mut Context<Self>,
    ) {
        if self.owner.as_ref() != Some(&owner) {
            self.agents.error = Some(
                "The account changed. Reopen this computer before reconnecting its coding agent."
                    .into(),
            );
            cx.notify();
            return;
        }
        self.agents.recovery_open = true;
        self.agents.recovery_target = Some((owner, agent, connection));
        self.notice = Some("Checking this task's selected coding agent…".into());
        if !self
            .pending
            .values()
            .any(|request| request.kind == "agents")
        {
            self.call("agents", methods::CLOUD_AGENT_CONNECTIONS, json!({}), cx);
        }
        cx.notify();
    }
    pub(super) fn agent_recovery_open(&self) -> bool {
        self.agents.recovery_open
    }
    fn apply_agent_recovery(&mut self, cx: &mut Context<Self>) {
        let Some((owner, agent, id)) = self.agents.recovery_target.take() else {
            return;
        };
        let selection = super::agent_recovery::selected(
            self.owner.as_ref(),
            &owner,
            &agent,
            &id,
            &self.agents.connections,
        )
        .cloned();
        self.notice = None;
        match selection {
            Ok(connection) => self.select_agent_target(
                connection.agent,
                Some(connection.connection_id),
                Some(owner),
                cx,
            ),
            Err(message) => {
                if !self.unfinished_agent_login() {
                    self.agents.edit = None;
                    self.agents.selection = None;
                    self.agents.managed_connection = None;
                }
                self.agents.error = Some(message.into());
                cx.notify();
            }
        }
    }
    pub fn set_account_feed_live(&mut self, live: bool, cx: &mut Context<Self>) {
        let was = std::mem::replace(&mut self.account_feed_live, live);
        if live {
            self.agents.poll = None;
            self.github_poll = None;
            self.image.stop_polling();
            self.machine.stop_polling();
            // Hints sent while disconnected were missed: read once now.
            if !was && self.loaded {
                self.machine_image_changed(cx);
            }
        } else {
            self.schedule_agent_poll(cx);
            if was {
                self.schedule_image_poll(cx);
                self.schedule_machine_poll(cx);
            }
        }
    }
    /// A build, image or recipe changed on the server (`machine-image` hint).
    fn machine_image_changed(&mut self, cx: &mut Context<Self>) {
        self.refresh_image(cx);
        if !self.pending.values().any(|p| p.kind.starts_with("machine")) {
            self.load_machine(cx);
        }
    }
    pub fn account_event(&mut self, kind: &str, resource: &str, cx: &mut Context<Self>) {
        match kind {
            "authentication" => {
                if self.agents.login_binding.is_some()
                    && self.agents.operation.is_none()
                    && !self.pending.values().any(|p| p.kind == "agents")
                {
                    self.call("agents", methods::CLOUD_AGENT_CONNECTIONS, json!({}), cx);
                }
                if let Some(id) = self
                    .agents
                    .operation
                    .as_ref()
                    .filter(|operation| operation.operation_id == resource)
                    .map(|operation| operation.operation_id.clone())
                {
                    if !self
                        .pending
                        .values()
                        .any(|pending| pending.kind == "agent-operation")
                    {
                        self.call(
                            "agent-operation",
                            methods::CLOUD_AGENT_LOGIN_STATUS,
                            json!({"operationId":id}),
                            cx,
                        );
                    }
                }
            }
            "connection" => {
                if !self
                    .pending
                    .values()
                    .any(|pending| pending.kind == "agents")
                {
                    self.call("agents", methods::CLOUD_AGENT_CONNECTIONS, json!({}), cx);
                }
            }
            "github" => {
                if self.github_operation.as_ref().is_some_and(|operation| {
                    !matches!(
                        operation["state"].as_str(),
                        Some("complete" | "cancelled" | "failed" | "expired")
                    )
                }) {
                    if !self
                        .pending
                        .values()
                        .any(|pending| pending.kind == "github-status")
                    {
                        self.github_action("github-status", "status", None, cx);
                    }
                } else if !self
                    .pending
                    .values()
                    .any(|pending| pending.kind == "github")
                {
                    self.call("github", methods::CLOUD_GITHUB, json!({}), cx);
                }
            }
            "environment" => self.environment_event(resource, cx),
            "machine-image" if self.loaded => self.machine_image_changed(cx),
            _ => {}
        }
    }
    pub(super) fn receive_agents(&mut self, kind: &str, data: Value, cx: &mut Context<Self>) {
        self.agents.error = None;
        match kind {
            "agents" => {
                match serde_json::from_value::<Vec<AgentConnection>>(data["connections"].clone()) {
                    Ok(connections) => {
                        // A hidden row stays hidden until the server agrees
                        // it is gone; a stale reply can't bring it back.
                        self.agents.disconnecting.retain(|id| {
                            connections.iter().any(|c| {
                                &c.connection_id == id && c.state != ConnectionState::Revoked
                            })
                        });
                        self.agents.connections = connections;
                        self.agents.supports_reconnect = data["supportsDeviceReconnect"] == true;
                        self.agents.supports_reconfigure =
                            data["supportsCredentialReconfiguration"] == true;
                        self.apply_agent_recovery(cx);
                        let proposal = self
                            .owner
                            .as_ref()
                            .zip(self.agents.login_binding.as_ref())
                            .and_then(|(owner, binding)| {
                                self.agents
                                    .connections
                                    .iter()
                                    .find(|row| {
                                        binding.matches(
                                            owner,
                                            row.agent.as_str(),
                                            Some(&row.connection_id),
                                        ) && row.credential_update_key.as_deref()
                                            == Some(binding.idempotency_key.as_str())
                                    })
                                    .and_then(|row| row.credential_update.clone())
                            })
                            .filter(|operation| {
                                self.agents.operation.as_ref().is_none_or(|current| {
                                    current.operation_id != operation.operation_id
                                        || current.state != operation.state
                                })
                            });
                        if let Some(operation) = proposal {
                            self.receive_agents(
                                "agent-login",
                                serde_json::to_value(operation).unwrap_or(Value::Null),
                                cx,
                            );
                        }
                    }
                    Err(_) => {
                        self.agents.error = Some(
                            "Agent connections could not be verified. Refresh to retry.".into(),
                        )
                    }
                }
            }
            "agent-configure" => match serde_json::from_value::<AgentConnection>(data) {
                Ok(connection) => {
                    let proposal = connection.credential_update.clone();
                    if proposal.is_some()
                        && !self
                            .owner
                            .as_ref()
                            .zip(self.agents.login_binding.as_ref())
                            .is_some_and(|(owner, binding)| {
                                binding.matches(
                                    owner,
                                    connection.agent.as_str(),
                                    Some(&connection.connection_id),
                                ) && connection.credential_update_key.as_deref()
                                    == Some(binding.idempotency_key.as_str())
                                    && self.agents.edit
                                        == Some((connection.agent, connection.method))
                            })
                    {
                        self.agents.error=Some("Credential verification did not match the selected connection. Refresh before continuing.".into());
                        cx.notify();
                        return;
                    }
                    self.agents
                        .connections
                        .retain(|c| c.connection_id != connection.connection_id);
                    self.agents.connections.push(connection);
                    if let Some(operation) = proposal {
                        self.receive_agents(
                            "agent-login",
                            serde_json::to_value(operation).unwrap_or(Value::Null),
                            cx,
                        );
                    } else {
                        self.agents.edit = None;
                        self.agents.login_binding = None;
                    }
                    self.agents.polls = 0;
                }
                Err(_) => {
                    self.agents.error = Some(
                        "The connection result could not be verified. Refresh before retrying."
                            .into(),
                    )
                }
            },
            "agent-login" | "agent-operation" | "agent-cancel" => {
                match serde_json::from_value::<LoginOperation>(data) {
                    Ok(operation) => {
                        let accepted = self
                            .owner
                            .as_ref()
                            .zip(self.agents.login_binding.as_mut())
                            .is_some_and(|(owner, binding)| {
                                if kind == "agent-login" {
                                    binding.accept(
                                        owner,
                                        operation.agent.as_str(),
                                        &operation.connection_id,
                                        &operation.operation_id,
                                    )
                                } else {
                                    binding.accept_update(
                                        owner,
                                        operation.agent.as_str(),
                                        &operation.connection_id,
                                        &operation.operation_id,
                                    )
                                }
                            });
                        if !accepted {
                            if kind != "agent-login" {
                                return;
                            }
                            self.agents.error=Some("Sign-in response did not match the selected account and connection. Refresh before continuing.".into());
                            cx.notify();
                            return;
                        }
                        if self.agents.operation.as_ref().is_some_and(|current| {
                            current.operation_id == operation.operation_id
                                && !matches!(
                                    current.state,
                                    LoginState::Queued | LoginState::Authorizing
                                )
                        }) && matches!(
                            operation.state,
                            LoginState::Queued | LoginState::Authorizing
                        ) {
                            return;
                        }
                        if kind == "agent-login"
                            && self.agents.abandoned.is_some()
                            && self.agents.abandoned.as_deref()
                                == self
                                    .agents
                                    .login_binding
                                    .as_ref()
                                    .map(|b| b.idempotency_key.as_str())
                        {
                            // Cancelled before the code arrived: cancel it
                            // on the server too and keep the row closed.
                            self.agents.abandoned = None;
                            let mut operation = operation;
                            if matches!(
                                operation.state,
                                LoginState::Queued | LoginState::Authorizing
                            ) {
                                self.call(
                                    "agent-cancel",
                                    methods::CLOUD_AGENT_LOGIN_CANCEL,
                                    json!({"operationId":operation.operation_id}),
                                    cx,
                                );
                                operation.state = LoginState::Cancelled;
                            }
                            self.agents.operation = Some(operation);
                            cx.notify();
                            return;
                        }
                        if kind == "agent-login" {
                            if self.agents.edit
                                == Some((operation.agent, ConnectionMethod::DeviceCode))
                            {
                                self.agents.selection = self.agents.login_binding.clone();
                                self.agents.managed_connection =
                                    Some(operation.connection_id.clone());
                            }
                            self.agents.polls = 0;
                        }
                        if operation.state == LoginState::Connected {
                            self.call("agents", methods::CLOUD_AGENT_CONNECTIONS, json!({}), cx);
                            // Show it connected now; the reload fills in
                            // the account name.
                            self.agents.mark_connected(&operation);
                            if self
                                .agents
                                .edit
                                .is_some_and(|(agent, _)| agent == operation.agent)
                            {
                                self.agents.close_edit(cx);
                            }
                        }
                        self.agents.operation = Some(operation);
                    }
                    Err(_) => {
                        self.agents.error = Some(
                            "The sign-in result could not be verified. Refresh before retrying."
                                .into(),
                        )
                    }
                }
            }
            "agent-disconnect" => {
                self.call("agents", methods::CLOUD_AGENT_CONNECTIONS, json!({}), cx);
            }
            _ => {}
        }
        self.schedule_agent_poll(cx);
        cx.notify();
    }
    fn schedule_agent_poll(&mut self, cx: &mut Context<Self>) {
        if self.account_feed_live {
            return;
        }
        let login = self
            .agents
            .operation
            .as_ref()
            .is_some_and(|o| matches!(o.state, LoginState::Queued | LoginState::Authorizing));
        let verification = self
            .agents
            .connections
            .iter()
            .any(|c| c.state == ConnectionState::Pending);
        if (!login && !verification)
            || self.agents.polls >= 450
            || self.agents.poll.is_some()
            || self.agents.error.is_some()
        {
            return;
        }
        self.agents.poll = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(2))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.agents.poll = None;
                this.agents.polls += 1;
                if let Some(id) = this
                    .agents
                    .operation
                    .as_ref()
                    .filter(|o| matches!(o.state, LoginState::Queued | LoginState::Authorizing))
                    .map(|o| o.operation_id.clone())
                {
                    this.call(
                        "agent-operation",
                        methods::CLOUD_AGENT_LOGIN_STATUS,
                        json!({"operationId":id}),
                        cx,
                    );
                } else if this
                    .agents
                    .connections
                    .iter()
                    .any(|c| c.state == ConnectionState::Pending)
                {
                    this.call("agents", methods::CLOUD_AGENT_CONNECTIONS, json!({}), cx);
                }
            });
        }));
    }
    fn unfinished_agent_login(&self) -> bool {
        self.agents.login_binding.is_some()
            && self
                .agents
                .operation
                .as_ref()
                .is_none_or(|op| matches!(op.state, LoginState::Queued | LoginState::Authorizing))
    }
    fn select_agent_target(
        &mut self,
        agent: AgentId,
        connection: Option<String>,
        expected_owner: Option<Owner>,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self
            .owner
            .clone()
            .filter(|owner| Some(owner) == expected_owner.as_ref())
        else {
            return;
        };
        if self.unfinished_agent_login()
            && self.agents.login_binding.as_ref().is_some_and(|binding| {
                !binding.matches(&owner, agent.as_str(), connection.as_deref())
            })
        {
            self.agents.error = Some(
                "Finish or cancel the existing sign-in before starting another account connection."
                    .into(),
            );
            cx.notify();
            return;
        }
        let method = connection
            .as_ref()
            .and_then(|id| {
                self.agents
                    .connections
                    .iter()
                    .find(|c| {
                        &c.connection_id == id
                            && c.agent == agent
                            && c.state != ConnectionState::Revoked
                    })
                    .map(|c| c.method)
            })
            .unwrap_or(if agent == AgentId::Claude {
                ConnectionMethod::SetupToken
            } else {
                ConnectionMethod::DeviceCode
            });
        if connection.as_ref().is_some_and(|id| {
            !self.agents.connections.iter().any(|c| {
                &c.connection_id == id
                    && c.agent == agent
                    && c.method == method
                    && c.state != ConnectionState::Revoked
            })
        }) && !self.unfinished_agent_login()
        {
            self.agents.error=Some("This connection is no longer available in the selected account. Refresh to continue.".into());
            cx.notify();
            return;
        }
        if !self.unfinished_agent_login() {
            self.agents.operation = None;
            self.agents.login_binding = None;
        }
        self.agents.managed_connection = connection.clone();
        self.agents.selection = Some(
            self.agents
                .login_binding
                .as_ref()
                .filter(|binding| {
                    self.unfinished_agent_login()
                        && binding.matches(&owner, agent.as_str(), connection.as_deref())
                })
                .cloned()
                .unwrap_or_else(|| Binding {
                    owner,
                    agent: agent.as_str().into(),
                    connection,
                    idempotency_key: uuid::Uuid::new_v4().to_string(),
                    operation: None,
                }),
        );
        self.agents.edit = Some((agent, method));
        self.agents
            .secret
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.agents.error = None;
        cx.notify();
    }
    fn submit_agent(&mut self, cx: &mut Context<Self>) {
        let Some((agent, method)) = self.agents.edit else {
            return;
        };
        if self
            .pending
            .values()
            .any(|p| matches!(p.kind, "agent-configure" | "agent-login"))
        {
            return;
        }
        self.agents.error = None;
        if method == ConnectionMethod::DeviceCode {
            let Some(owner) = self.owner.as_ref() else {
                return;
            };
            let Some(binding) = self.agents.selection.clone() else {
                return;
            };
            if !binding.can_submit(self.agents.supports_reconnect) {
                self.agents.error=Some("This server needs an update before an existing connection can be reauthorized safely. Add another account creates a separate connection.".into());
                cx.notify();
                return;
            }
            if self.unfinished_agent_login()
                && self.agents.login_binding.as_ref().is_some_and(|pending| {
                    !pending.matches(owner, agent.as_str(), binding.connection.as_deref())
                })
            {
                self.agents.error = Some(
                    "Finish or cancel the existing sign-in before reconnecting another account."
                        .into(),
                );
                cx.notify();
                return;
            }
            let existing = self
                .agents
                .operation
                .as_ref()
                .filter(|o| {
                    o.agent == agent
                        && matches!(o.state, LoginState::Queued | LoginState::Authorizing)
                })
                .cloned();
            if existing.is_some() {
                cx.notify();
                return;
            }
            let connection = match binding.request_connection(
                owner,
                agent.as_str(),
                self.agents.connections.iter().map(|c| {
                    (
                        c.connection_id.as_str(),
                        c.agent.as_str(),
                        c.method.as_str(),
                    )
                }),
            ) {
                Ok(id) => id.map(str::to_owned),
                Err(message) => {
                    self.agents.error = Some(message.into());
                    cx.notify();
                    return;
                }
            };
            self.agents.login_binding = Some(binding.clone());
            self.agents.operation = None;
            self.call("agent-login",methods::CLOUD_AGENT_LOGIN,json!({"agent":agent,"connectionId":connection,"idempotencyKey":binding.idempotency_key}),cx);
        } else {
            let secret = self.agents.secret.read(cx).value(cx);
            if secret.trim().is_empty() {
                return;
            }
            let Some(owner) = self.owner.as_ref() else {
                return;
            };
            let Some(binding) = self.agents.selection.clone() else {
                return;
            };
            if !binding.can_submit(self.agents.supports_reconfigure) {
                self.agents.error=Some("This server needs an update before this existing credential can be replaced safely.".into());
                cx.notify();
                return;
            }
            if self.unfinished_agent_login() {
                self.agents.error=Some("Verification may already be in progress. Refresh its status before submitting another credential.".into());
                cx.notify();
                return;
            }
            let mut binding = binding;
            if self
                .agents
                .operation
                .as_ref()
                .is_some_and(|op| !matches!(op.state, LoginState::Queued | LoginState::Authorizing))
            {
                binding.idempotency_key = uuid::Uuid::new_v4().to_string();
                binding.operation = None;
                self.agents.selection = Some(binding.clone());
            }
            let connection = match binding.request_configuration(
                owner,
                agent.as_str(),
                method.as_str(),
                self.agents
                    .connections
                    .iter()
                    .filter(|c| c.state != ConnectionState::Revoked)
                    .map(|c| {
                        (
                            c.connection_id.as_str(),
                            c.agent.as_str(),
                            c.method.as_str(),
                        )
                    }),
            ) {
                Ok(id) => id.map(str::to_owned),
                Err(message) => {
                    self.agents.error = Some(message.into());
                    cx.notify();
                    return;
                }
            };
            self.agents.login_binding = connection.as_ref().map(|_| binding.clone());
            self.agents.operation = None;
            self.call("agent-configure",methods::CLOUD_AGENT_CONFIGURE,
                json!({"agent":agent,"method":method,"secret":secret,"connectionId":connection,"idempotencyKey":binding.idempotency_key}),cx);
            self.agents
                .secret
                .update(cx, |input, cx| input.set_value(String::new(), cx));
        }
        cx.notify();
    }
}

/// The cloud coding agents, in the order the page lists them.
const AGENTS: [AgentId; 3] = [AgentId::Claude, AgentId::Codex, AgentId::Grok];

fn agent_name(agent: AgentId) -> &'static str {
    match agent {
        AgentId::Claude => "Claude",
        AgentId::Codex => "Codex",
        AgentId::Grok => "Grok",
    }
}

/// The Codex mark keeps its gradient: gpui's SVG element paints one colour,
/// so it renders as an image.
fn codex_mark() -> &'static std::sync::Arc<Image> {
    static IMAGE: std::sync::OnceLock<std::sync::Arc<Image>> = std::sync::OnceLock::new();
    IMAGE.get_or_init(|| {
        let svg = include_str!("../../../ui/assets/icons/codex-mark.svg")
            .replace("fill=\"currentColor\"", "fill=\"url(#codex-gradient)\"")
            .replace("<path", "<defs><linearGradient id=\"codex-gradient\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop stop-color=\"#b9adff\"/><stop offset=\"0.5\" stop-color=\"#8094ff\"/><stop offset=\"1\" stop-color=\"#453cf5\"/></linearGradient></defs><path");
        std::sync::Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes()))
    })
}

/// Row avatar size, and the indent that lines nested content up with names.
const TILE: f32 = 26.;
const NESTED: f32 = s::ROW_PAD_X + TILE + s::ROW_GAP;

/// The agent's own mark in a round avatar, like account avatars elsewhere.
fn agent_tile(agent: AgentId) -> Div {
    let mark = match agent {
        AgentId::Codex => img(codex_mark().clone())
            .size(px(14.))
            .flex_none()
            .into_any_element(),
        AgentId::Claude => icon("claude-mark")
            .size(px(14.))
            .text_color(gpui::rgb(0xD97757))
            .into_any_element(),
        AgentId::Grok => icon("grok-mark")
            .size(px(14.))
            .text_color(palette::rgb(TEXT))
            .into_any_element(),
    };
    div()
        .flex_none()
        .size(px(TILE))
        .rounded_full()
        .bg(s::frame_bg())
        .flex()
        .items_center()
        .justify_center()
        .child(mark)
}

/// What an account is called: its label, else how it signed in.
fn account_name(connection: &AgentConnection) -> String {
    connection
        .alias
        .as_ref()
        .filter(|label| !label.trim().is_empty())
        .or_else(|| {
            connection
                .account_label
                .as_ref()
                .filter(|label| !label.trim().is_empty())
        })
        .cloned()
        .unwrap_or_else(|| {
            match connection.method {
                ConnectionMethod::ApiKey => "API key",
                ConnectionMethod::SetupToken => "Setup token",
                ConnectionMethod::DeviceCode => "Signed in",
            }
            .into()
        })
}

fn mini_spinner(id: impl Into<ElementId>) -> AnyElement {
    icon("loading")
        .size(px(12.))
        .text_color(palette::rgb(MUTED))
        .with_animation(
            id,
            Animation::new(std::time::Duration::from_millis(1000)).repeat(),
            |svg, t| svg.with_transformation(Transformation::rotate(percentage(t))),
        )
        .into_any_element()
}

fn quiet_text(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(palette::rgb(MUTED))
        .child(text.into())
}

/// A connection's status, on the right of its row. Revoked shows nothing.
fn connection_status(id: &str, state: ConnectionState) -> Option<Div> {
    let (dot, label): (Option<Hsla>, &str) = match state {
        ConnectionState::Connected => (
            Some(palette::tone(
                hsla(133.9 / 360., 0.606, 0.488, 1.),
                hsla(131.4 / 360., 0.734, 0.31, 1.),
            )),
            "Connected",
        ),
        ConnectionState::Pending => (None, "Verifying"),
        ConnectionState::ReconnectRequired => (
            Some(palette::tone(
                hsla(32. / 360., 0.92, 0.6, 1.),
                hsla(26. / 360., 0.9, 0.42, 1.),
            )),
            "Signed out",
        ),
        ConnectionState::Error => (Some(s::destructive_text()), "Failed"),
        ConnectionState::Revoked => return None,
    };
    let lead = match dot {
        Some(dot) => div()
            .flex_none()
            .size(px(6.))
            .rounded_full()
            .bg(dot)
            .into_any_element(),
        None => mini_spinner(SharedString::from(format!("agent-verifying-{id}"))),
    };
    Some(
        row()
            .flex_none()
            .gap(px(6.))
            .child(lead)
            .child(quiet_text(label)),
    )
}

impl RepositoryPanel {
    /// Prepares the inline form after it opens: device sign-in starts at
    /// once; credential forms get the right placeholder and focus.
    fn agent_form_opened(&mut self, agent: AgentId, cx: &mut Context<Self>) {
        match self.agents.edit {
            Some((open, ConnectionMethod::DeviceCode)) if open == agent => {
                let allowed = self
                    .agents
                    .selection
                    .as_ref()
                    .is_some_and(|binding| binding.can_submit(self.agents.supports_reconnect));
                if allowed && !self.unfinished_agent_login() {
                    self.submit_agent(cx);
                }
            }
            Some((open, method)) if open == agent => {
                let placeholder = if method == ConnectionMethod::SetupToken {
                    "Paste the setup token"
                } else {
                    "Paste an API key"
                };
                self.agents.secret.update(cx, |input, cx| {
                    input.set_placeholder(placeholder.into(), cx);
                    input.focus(cx);
                });
            }
            _ => {}
        }
        cx.notify();
    }
    /// "Connect" and "Add another account": a new connection, signed in the
    /// agent's default way.
    fn connect_agent(&mut self, agent: AgentId, owner: Option<Owner>, cx: &mut Context<Self>) {
        if self.owner != owner {
            return;
        }
        self.cancel_agent_edit(cx);
        self.select_agent_target(agent, None, owner, cx);
        self.agent_form_opened(agent, cx);
    }
    /// Signs an existing connection in again, keeping its identity so tasks
    /// pinned to it carry on.
    fn reconnect_agent(
        &mut self,
        agent: AgentId,
        connection: String,
        owner: Option<Owner>,
        cx: &mut Context<Self>,
    ) {
        if self.owner != owner {
            return;
        }
        self.cancel_agent_edit(cx);
        self.select_agent_target(agent, Some(connection), owner, cx);
        self.agent_form_opened(agent, cx);
    }
    /// Swaps the open form to another way of connecting, as a new connection.
    fn switch_agent_method(
        &mut self,
        agent: AgentId,
        method: ConnectionMethod,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self.owner.clone() else {
            return;
        };
        if self.agents.abandoned.is_some() {
            return;
        }
        if self.agents.edit.is_some_and(|(open, _)| open != agent) {
            self.cancel_agent_edit(cx);
        } else {
            self.cancel_agent_login(cx);
        }
        self.agents.recovery_target = None;
        self.agents.recovery_open = false;
        self.agents.selection = Some(Binding {
            owner,
            agent: agent.as_str().into(),
            connection: None,
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            operation: None,
        });
        self.agents.login_binding = None;
        self.agents.operation = None;
        self.agents.managed_connection = None;
        self.agents.error = None;
        self.agents.edit = Some((agent, method));
        self.agents
            .secret
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.agent_form_opened(agent, cx);
    }
    /// Stops a sign-in in progress: on the server if it has started, or as
    /// soon as its code arrives if not.
    fn cancel_agent_login(&mut self, cx: &mut Context<Self>) {
        let started = self
            .agents
            .operation
            .as_mut()
            .filter(|o| matches!(o.state, LoginState::Queued | LoginState::Authorizing))
            .map(|operation| {
                operation.state = LoginState::Cancelled;
                operation.operation_id.clone()
            });
        if let Some(id) = started {
            self.call(
                "agent-cancel",
                methods::CLOUD_AGENT_LOGIN_CANCEL,
                json!({"operationId":id}),
                cx,
            );
        } else if self
            .pending
            .values()
            .any(|pending| pending.kind == "agent-login")
        {
            self.agents.abandoned = self
                .agents
                .login_binding
                .as_ref()
                .map(|binding| binding.idempotency_key.clone());
        }
    }
    /// Closes the open form, cancelling any sign-in it started.
    fn cancel_agent_edit(&mut self, cx: &mut Context<Self>) {
        if self.agents.edit.is_none() {
            return;
        }
        self.cancel_agent_login(cx);
        self.agents.close_edit(cx);
        self.agents.error = None;
        cx.notify();
    }
    fn disconnect_agent(
        &mut self,
        connection: String,
        owner: Option<Owner>,
        cx: &mut Context<Self>,
    ) {
        if self.owner != owner {
            return;
        }
        if self.agents.managed_connection.as_deref() == Some(connection.as_str()) {
            self.cancel_agent_edit(cx);
        }
        self.agents.disconnecting.insert(connection.clone());
        self.call(
            "agent-disconnect",
            methods::CLOUD_AGENT_DISCONNECT,
            json!({"connectionId":connection}),
            cx,
        );
        cx.notify();
    }
    fn copy_agent_code(&mut self, code: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.agents.copied = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.agents.copied = None;
                cx.notify();
            });
        }));
        cx.notify();
    }
    /// Whether this connection can be signed in again in place.
    fn can_reconnect(&self, connection: &AgentConnection) -> bool {
        if connection.method == ConnectionMethod::DeviceCode {
            self.agents.supports_reconnect
        } else {
            self.agents.supports_reconfigure
        }
    }

    pub(super) fn render_agents(&self, _window: &Window, cx: &mut Context<Self>) -> Div {
        let busy = self
            .pending
            .values()
            .any(|p| p.kind.starts_with("agent") && p.kind != "agents");
        let mut body = column();
        for (i, agent) in AGENTS.into_iter().enumerate() {
            body = body.child(self.agent_block(i == 0, agent, busy, cx));
        }
        if self.agents.edit.is_none()
            && let Some(error) = &self.agents.error
        {
            body = body.child(
                div()
                    .px(px(s::ROW_PAD_X))
                    .py(px(10.))
                    .border_t_1()
                    .border_color(palette::rgb(BORDER))
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(s::destructive_text())
                    .child(error.clone()),
            );
        }
        kit::section(
            "Coding agents",
            "Cloud chats use your own subscription or API key. Connect once, then pick the account per chat. Agents on this computer are under Providers & accounts.",
            Some(
                cs::icon_button("refresh-cloud-agents", "refresh", "Refresh", busy, false)
                    .tip("Refresh")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.agents.error = None;
                        this.call("agents", methods::CLOUD_AGENT_CONNECTIONS, json!({}), cx);
                    }))
                    .into_any_element(),
            ),
            body,
        )
    }

    /// One agent: its row, any extra accounts under it, and the inline
    /// connect form when it is open.
    fn agent_block(&self, first: bool, agent: AgentId, busy: bool, cx: &mut Context<Self>) -> Div {
        let owner = self.owner.clone();
        let accounts: Vec<&AgentConnection> = self
            .agents
            .connections
            .iter()
            .filter(|c| {
                c.agent == agent
                    && c.state != ConnectionState::Revoked
                    && !self.agents.disconnecting.contains(&c.connection_id)
            })
            .collect();
        let open = self.agents.edit.is_some_and(|(open, _)| open == agent);
        let other_open = self.agents.edit.is_some() && !open;
        let target = self
            .agents
            .selection
            .as_ref()
            .and_then(|binding| binding.connection.clone());

        let (description, trailing) = match accounts.as_slice() {
            [] => (
                String::new(),
                (!open).then(|| {
                    // One primary action at a time: another agent's open
                    // form owns it.
                    let id = SharedString::from(format!("cloud-agent-{}", agent.as_str()));
                    let button = if other_open {
                        cs::button(id, "Connect", false, busy, false)
                    } else {
                        kit::primary_button(id, "Connect", !busy)
                    };
                    let owner = owner.clone();
                    button
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.connect_agent(agent, owner.clone(), cx)
                        }))
                        .into_any_element()
                }),
            ),
            [account] => (
                account_name(account),
                Some(self.account_trailing(agent, account, true, busy, cx)),
            ),
            // Each account row below carries its own status.
            many => (
                format!("{} accounts", many.len()),
                Some(self.agent_menu(agent, None, busy, cx)),
            ),
        };
        let mut block = column()
            .when(!first, |v| {
                v.border_t_1().border_color(palette::rgb(BORDER))
            })
            .child(kit::row_item(
                true,
                Some(agent_tile(agent).into_any_element()),
                agent_name(agent),
                description,
                trailing,
            ));
        let nested_target = accounts.len() > 1
            && target
                .as_deref()
                .is_some_and(|id| accounts.iter().any(|c| c.connection_id == id));
        if open && !nested_target {
            block = block.child(self.agent_form(agent, busy, cx));
        }
        if accounts.len() > 1 {
            for account in &accounts {
                block = block.child(
                    s::row(true)
                        .flex_nowrap()
                        .pl(px(NESTED))
                        .py(px(6.))
                        .min_h(px(40.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(palette::rgb(TEXT))
                                .child(account_name(account)),
                        )
                        .child(self.account_trailing(agent, account, false, busy, cx)),
                );
                if open && target.as_deref() == Some(account.connection_id.as_str()) {
                    block = block.child(self.agent_form(agent, busy, cx));
                }
            }
        }
        block
    }

    /// Status, a "Reconnect" key when the account needs one, and its menu.
    fn account_trailing(
        &self,
        agent: AgentId,
        account: &AgentConnection,
        only: bool,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = self.owner.clone();
        let id = account.connection_id.clone();
        let needs_sign_in = matches!(
            account.state,
            ConnectionState::ReconnectRequired | ConnectionState::Error
        ) && self.can_reconnect(account)
            && self.agents.edit.is_none();
        let menu = self.agent_menu(agent, Some((account, only)), busy, cx);
        let status = if account.error_code.as_deref() == Some("agent_already_connected") {
            Some(quiet_text("Already connected"))
        } else if account.state == ConnectionState::Connected && !account.enabled {
            Some(quiet_text("Disabled"))
        } else {
            connection_status(&id, account.state)
        };
        row()
            .gap(px(10.))
            .when_some(status, |v, s| v.child(s))
            .when(needs_sign_in, |v| {
                let id = id.clone();
                v.child(
                    cs::button(
                        SharedString::from(format!("agent-reconnect-{id}")),
                        "Reconnect",
                        false,
                        busy,
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.reconnect_agent(agent, id.clone(), owner.clone(), cx)
                    })),
                )
            })
            .child(menu)
            .into_any_element()
    }

    /// The "…" menu: per account (sign in again, disconnect) and, on the
    /// agent's own row, adding accounts.
    fn agent_menu(
        &self,
        agent: AgentId,
        account: Option<(&AgentConnection, bool)>,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut items = vec![];
        // The agent's own row adds accounts; nested account rows don't.
        if account.is_none_or(|(_, only)| only) {
            let owner = self.owner.clone();
            items.push(MenuItem {
                id: format!("agent-add-{}", agent.as_str()).into(),
                glyph: "plus",
                label: "Add another account".into(),
                danger: false,
                run: std::rc::Rc::new(move |this, _, cx| {
                    this.connect_agent(agent, owner.clone(), cx)
                }),
            });
            items.push(MenuItem {
                id: format!("agent-api-key-{}", agent.as_str()).into(),
                glyph: "key-minimalistic",
                label: "Use API key".into(),
                danger: false,
                run: std::rc::Rc::new(move |this, _, cx| {
                    this.switch_agent_method(agent, ConnectionMethod::ApiKey, cx)
                }),
            });
        }
        if let Some((account, _)) = account.filter(|(a, _)| self.can_reconnect(a)) {
            let id = account.connection_id.clone();
            let owner = self.owner.clone();
            items.push(MenuItem {
                id: format!("agent-reconnect-item-{id}").into(),
                glyph: "refresh",
                label: if account.method == ConnectionMethod::DeviceCode {
                    "Sign in again"
                } else {
                    "Replace credential"
                }
                .into(),
                danger: false,
                run: std::rc::Rc::new(move |this, _, cx| {
                    this.reconnect_agent(agent, id.clone(), owner.clone(), cx)
                }),
            });
        }
        if let Some((account, _)) = account {
            let id = account.connection_id.clone();
            let owner = self.owner.clone();
            items.push(MenuItem {
                id: format!("agent-disconnect-{id}").into(),
                glyph: "trash-bin-minimalistic",
                label: "Disconnect".into(),
                danger: true,
                run: std::rc::Rc::new(move |this, _, cx| {
                    this.disconnect_agent(id.clone(), owner.clone(), cx)
                }),
            });
        }
        let id = match account {
            Some((account, _)) => format!("agent-account-menu-{}", account.connection_id),
            None => format!("agent-menu-{}", agent.as_str()),
        };
        self.overflow_menu(
            id.into(),
            format!("{} account options", agent_name(agent)).into(),
            busy,
            items,
            cx,
        )
    }

    /// The inline connect form inside an agent's row.
    fn agent_form(&self, agent: AgentId, busy: bool, cx: &mut Context<Self>) -> Div {
        let Some((_, method)) = self.agents.edit else {
            return div();
        };
        let operation = self.agents.operation.as_ref().filter(|o| o.agent == agent);
        let waiting = operation
            .is_some_and(|o| matches!(o.state, LoginState::Queued | LoginState::Authorizing));
        let requesting = self
            .pending
            .values()
            .any(|p| matches!(p.kind, "agent-login" | "agent-configure"));
        let replacing = self
            .agents
            .selection
            .as_ref()
            .is_some_and(|binding| binding.connection.is_some());
        let mut form = column()
            .gap(px(10.))
            .pl(px(NESTED))
            .pr(px(s::ROW_PAD_X))
            .pb(px(12.));
        // The status line: an optional key and status on the left, the
        // text links on the right.
        let mut status: Option<Div> = None;
        let mut primary: Option<AnyElement> = None;

        if method == ConnectionMethod::DeviceCode {
            let code = operation
                .filter(|_| waiting)
                .and_then(|o| o.verification_code.clone());
            let url = operation
                .filter(|_| waiting)
                .and_then(|o| o.verification_url.clone())
                .filter(|u| u.starts_with("https://"));
            if code.is_some() || url.is_some() {
                let mut line = row().gap(px(12.)).flex_wrap();
                if let Some(code) = code {
                    let copied = self.agents.copied.is_some();
                    let copy = code.clone();
                    line = line.child(
                        row()
                            .flex_none()
                            .h(px(40.))
                            .pl(px(14.))
                            .pr(px(6.))
                            .gap(px(10.))
                            .rounded(px(9.))
                            .border_1()
                            .border_color(s::card_border())
                            .bg(s::frame_bg())
                            .child(
                                div()
                                    .font_family("Geist Mono")
                                    .text_size(px(18.))
                                    .line_height(px(24.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(palette::rgb(TEXT))
                                    .child(code),
                            )
                            .child(
                                cs::icon_button(
                                    "copy-agent-device-code",
                                    if copied { "check" } else { "copy" },
                                    "Copy code",
                                    false,
                                    false,
                                )
                                .tip(if copied { "Copied" } else { "Copy code" })
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.copy_agent_code(copy.clone(), cx),
                                )),
                            ),
                    );
                }
                if let Some(url) = url {
                    line = line.child(
                        kit::quiet_button("open-agent-verification", "Open sign-in page")
                            .gap(px(4.))
                            .text_color(palette::rgb(TEXT))
                            .child(
                                icon("arrow-up-right")
                                    .size(px(12.))
                                    .text_color(palette::rgb(MUTED)),
                            )
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    );
                }
                form = form.child(line);
                status = Some(
                    row()
                        .gap(px(6.))
                        .child(mini_spinner("agent-sign-in-wait"))
                        .child(quiet_text("Waiting for sign-in…")),
                );
            } else if waiting || requesting {
                status = Some(
                    row()
                        .gap(px(6.))
                        .child(mini_spinner("agent-sign-in-code"))
                        .child(quiet_text("Getting a sign-in code…")),
                );
            } else {
                let (note, label) = match operation.map(|o| o.state) {
                    Some(LoginState::Expired) => ("That code expired.", "Get a new code"),
                    Some(LoginState::Error) => ("", "Try again"),
                    _ => ("", "Start sign-in"),
                };
                if !note.is_empty() {
                    status = Some(quiet_text(note));
                }
                let allowed = self
                    .agents
                    .selection
                    .as_ref()
                    .is_some_and(|b| b.can_submit(self.agents.supports_reconnect));
                if allowed {
                    primary = Some(
                        kit::primary_button("submit-agent-connection", label, !busy)
                            .on_click(cx.listener(|this, _, _, cx| this.submit_agent(cx)))
                            .into_any_element(),
                    );
                } else {
                    status = Some(quiet_text(
                        "Update the server to sign this connection in again.",
                    ));
                }
            }
        } else {
            if method == ConnectionMethod::SetupToken {
                form = form.child(
                    row()
                        .gap(px(6.))
                        .flex_wrap()
                        .child(quiet_text("Run"))
                        .child(
                            row()
                                .id("copy-setup-token-command")
                                .h(px(20.))
                                .px(px(6.))
                                .gap(px(6.))
                                .rounded(px(5.))
                                .bg(s::frame_bg())
                                .cursor_pointer()
                                .child(
                                    div()
                                        .font_family("Geist Mono")
                                        .text_size(px(11.))
                                        .text_color(palette::rgb(TEXT))
                                        .child("claude setup-token"),
                                )
                                .child(icon("copy").size(px(11.)).text_color(palette::rgb(MUTED)))
                                .tip("Copy command")
                                .on_click(|_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        "claude setup-token".into(),
                                    ))
                                }),
                        )
                        .child(quiet_text("in a terminal, then paste the token.")),
                );
            }
            let filled = !self.agents.secret.read(cx).value(cx).trim().is_empty();
            let allowed = self
                .agents
                .selection
                .as_ref()
                .is_some_and(|b| b.can_submit(self.agents.supports_reconfigure));
            form = form.child(
                row()
                    .gap(px(8.))
                    .child(
                        s::field()
                            .min_w_0()
                            .px(px(10.))
                            .child(self.agents.secret.clone()),
                    )
                    .child(
                        kit::primary_button(
                            "submit-agent-connection",
                            if replacing { "Replace" } else { "Save" },
                            filled && allowed && !busy && !waiting,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.submit_agent(cx))),
                    ),
            );
            if waiting || requesting {
                status = Some(
                    row()
                        .gap(px(6.))
                        .child(mini_spinner("agent-credential-verify"))
                        .child(quiet_text("Verifying…")),
                );
            } else if replacing && !allowed {
                status = Some(quiet_text("Update the server to replace this credential."));
            }
        }

        // The other way to connect, as a small text link.
        let alternative = match (agent, method) {
            (_, ConnectionMethod::DeviceCode) | (AgentId::Claude, ConnectionMethod::SetupToken) => {
                Some(("Use an API key instead", ConnectionMethod::ApiKey))
            }
            (AgentId::Claude, _) => {
                Some(("Use a setup token instead", ConnectionMethod::SetupToken))
            }
            (_, ConnectionMethod::ApiKey) => {
                Some(("Sign in instead", ConnectionMethod::DeviceCode))
            }
            _ => None,
        };
        let mut links = row().flex_none().gap(px(2.)).ml_auto();
        if let Some((label, next)) = alternative.filter(|_| !requesting) {
            links = links.child(kit::quiet_button("agent-switch-method", label).on_click(
                cx.listener(move |this, _, _, cx| this.switch_agent_method(agent, next, cx)),
            ));
        }
        links = links.child(
            kit::quiet_button("close-agent-connection", "Cancel")
                .on_click(cx.listener(|this, _, _, cx| this.cancel_agent_edit(cx))),
        );
        form = form.child(
            row()
                .gap(px(10.))
                .min_h(px(28.))
                .when_some(primary, |v, p| v.child(p))
                .when_some(status, |v, s| v.child(s))
                .child(links),
        );
        let message = self.agents.error.clone().or_else(|| {
            operation
                .filter(|o| o.state == LoginState::Error)
                .map(|o| {
                    match o.error_code.as_deref() {
                        Some("auth_account_mismatch") => "That was a different account. Sign in with the original one, or add it as another account.",
                        Some("auth_identity_review_required") => "This connection needs a review before it can be reused. Add another account instead.",
                        Some("agent_already_connected") => "Already connected",
                        _ => "Sign-in failed.",
                    }
                    .to_owned()
                })
        });
        if let Some(message) = message {
            form = form.child(
                div()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(s::destructive_text())
                    .child(message),
            );
        }
        form
    }
}

#[cfg(test)]
mod tests {
    use super::{account_name, connection_status};
    use zuse_protocol::{cloud_computers::AgentId, cloud_connections::*};

    fn connection(method: ConnectionMethod, label: Option<&str>) -> AgentConnection {
        AgentConnection {
            connection_id: "c".into(),
            agent: AgentId::Codex,
            method,
            state: ConnectionState::Connected,
            account_label: label.map(Into::into),
            position: 0,
            enabled: true,
            alias: None,
            expires_at: None,
            version: 1,
            error_code: None,
            pending_login_operation_id: None,
            credential_update: None,
            credential_update_key: None,
        }
    }

    #[test]
    fn accounts_are_named_by_label_then_by_how_they_signed_in() {
        let named = connection(ConnectionMethod::DeviceCode, Some("you@example.com"));
        assert_eq!(account_name(&named), "you@example.com");
        let blank = connection(ConnectionMethod::ApiKey, Some("  "));
        assert_eq!(account_name(&blank), "API key");
        let token = connection(ConnectionMethod::SetupToken, None);
        assert_eq!(account_name(&token), "Setup token");
    }

    #[test]
    fn only_live_connections_show_a_status() {
        assert!(connection_status("c", ConnectionState::Revoked).is_none());
        for state in [
            ConnectionState::Connected,
            ConnectionState::Pending,
            ConnectionState::ReconnectRequired,
            ConnectionState::Error,
        ] {
            assert!(connection_status("c", state).is_some());
        }
    }
}
