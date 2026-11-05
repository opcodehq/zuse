//! Connection setup shared by native and web. Secrets are cleared after submission.
use super::agent_login_binding::Binding;
use super::*;
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
}
impl AgentView {
    pub fn new(window: &mut Window, cx: &mut Context<RepositoryPanel>) -> Self {
        let secret = cx.new(|cx| TextInput::password("Paste your credential", window, cx));
        cx.observe(&secret, |_, _, cx| cx.notify()).detach();
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
        self.secret
            .update(cx, |input, cx| input.set_value(String::new(), cx));
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
    pub(crate) fn set_account_feed_live(&mut self, live: bool, cx: &mut Context<Self>) {
        self.account_feed_live = live;
        if live {
            self.agents.poll = None;
            self.github_poll = None;
        } else {
            self.schedule_agent_poll(cx);
        }
    }
    pub(crate) fn account_event(&mut self, kind: &str, resource: &str, cx: &mut Context<Self>) {
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
            _ => {}
        }
    }
    pub(super) fn receive_agents(&mut self, kind: &str, data: Value, cx: &mut Context<Self>) {
        self.agents.error = None;
        match kind {
            "agents" => {
                match serde_json::from_value::<Vec<AgentConnection>>(data["connections"].clone()) {
                    Ok(connections) => {
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
    fn open_agent(
        &mut self,
        agent: AgentId,
        expected_owner: Option<Owner>,
        cx: &mut Context<Self>,
    ) {
        if self.owner != expected_owner {
            return;
        }
        self.agents.recovery_target = None;
        self.agents.recovery_open = false;
        if self.unfinished_agent_login() {
            let connection = self
                .agents
                .login_binding
                .as_ref()
                .filter(|binding| binding.agent == agent.as_str())
                .and_then(|binding| binding.connection.clone());
            self.select_agent_target(agent, connection, expected_owner, cx);
            return;
        }
        let current = self
            .agents
            .connections
            .iter()
            .rev()
            .find(|c| c.agent == agent)
            .cloned();
        self.select_agent_target(agent, current.map(|c| c.connection_id), expected_owner, cx);
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
    pub(super) fn render_agents(&self, _window: &Window, cx: &mut Context<Self>) -> Div {
        let busy = self
            .pending
            .values()
            .any(|p| p.kind.starts_with("agent") && p.kind != "agents");
        let mut body = column();
        for (i, (agent, label)) in [
            (AgentId::Claude, "Claude"),
            (AgentId::Codex, "Codex"),
            (AgentId::Grok, "Grok"),
        ]
        .into_iter()
        .enumerate()
        {
            let connection = self
                .agents
                .connections
                .iter()
                .rev()
                .find(|c| c.agent == agent);
            let description = connection
                .map(|c| match c.state {
                    ConnectionState::Connected => c
                        .account_label
                        .clone()
                        .unwrap_or_else(|| "Connected".into()),
                    ConnectionState::Pending => "Verifying connection…".into(),
                    ConnectionState::ReconnectRequired => "Reconnect to continue".into(),
                    ConnectionState::Revoked => "Disconnected".into(),
                    ConnectionState::Error => "Connection failed. Reconnect to try again.".into(),
                })
                .unwrap_or_else(|| "Connect your account or API key".into());
            let expected_owner = self.owner.clone();
            body = body.child(cs::setting_row(
                i == 0,
                label,
                description,
                Some(
                    cs::button(
                        SharedString::from(format!("cloud-agent-{}", agent.as_str())),
                        if connection.is_some() {
                            "Manage"
                        } else {
                            "Connect"
                        },
                        false,
                        busy,
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_agent(agent, expected_owner.clone(), cx)
                    }))
                    .into_any_element(),
                ),
            ));
        }
        if let Some((agent, method)) = self.agents.edit {
            let label = match agent {
                AgentId::Claude => "Claude",
                AgentId::Codex => "Codex",
                AgentId::Grok => "Grok",
            };
            let alternative = if agent == AgentId::Claude {
                ConnectionMethod::SetupToken
            } else {
                ConnectionMethod::DeviceCode
            };
            let mut form = column()
                .gap(px(10.))
                .p(px(12.))
                .border_t_1()
                .border_color(s::card_border())
                .child(s::description_text(format!("Connect {label}")));
            let mut choices = row().gap(px(8.));
            for (choice, title) in [
                (
                    alternative,
                    if agent == AgentId::Claude {
                        "Setup token"
                    } else {
                        "Device code"
                    },
                ),
                (ConnectionMethod::ApiKey, "API key"),
            ] {
                choices = choices.child(
                    cs::button(
                        SharedString::from(format!("agent-method-{}", choice.as_str())),
                        title,
                        method == choice,
                        busy,
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.unfinished_agent_login() {this.agents.error=Some("Finish or cancel the pending device sign-in before changing its connection method.".into());cx.notify();return;}
                        let Some(owner)=this.owner.clone() else{return};
                        this.agents.selection=Some(Binding {owner,agent:agent.as_str().into(),connection:None,idempotency_key:uuid::Uuid::new_v4().to_string(),operation:None});
                        this.agents.login_binding=None;
                        this.agents.operation=None;
                        this.agents.edit = Some((agent, choice));
                        this.agents
                            .secret
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                        cx.notify();
                    })),
                );
            }
            form = form.child(choices);
            if method == ConnectionMethod::DeviceCode {
                let selected = self
                    .agents
                    .selection
                    .as_ref()
                    .and_then(|binding| binding.connection.as_deref());
                form=form.child(s::description_text(if selected.is_some() {"Reconnect this existing connection using the same provider account. Existing tasks keep their selected connection; a different account must be added separately."}else{"Add a separate account connection. Existing tasks keep their previously selected connection."}));
                for connection in self.agents.connections.iter().filter(|c| {
                    self.agents.supports_reconnect
                        && c.agent == agent
                        && c.method == ConnectionMethod::DeviceCode
                }) {
                    let id = connection.connection_id.clone();
                    let expected_owner = self.owner.clone();
                    let title = format!(
                        "Reconnect {}",
                        connection
                            .account_label
                            .as_deref()
                            .unwrap_or(&connection.connection_id)
                    );
                    form = form.child(
                        cs::button(
                            SharedString::from(format!("agent-reconnect-{id}")),
                            title,
                            selected == Some(id.as_str()),
                            busy,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_agent_target(
                                agent,
                                Some(id.clone()),
                                expected_owner.clone(),
                                cx,
                            )
                        })),
                    );
                }
                if selected.is_some() && !self.agents.supports_reconnect {
                    form=form.child(s::description_text("The server needs an update before this existing connection can be safely reauthorized. Add another account creates a separate connection."));
                }
                let expected_owner = self.owner.clone();
                form = form.child(
                    cs::button(
                        "add-agent-account",
                        "Add another account",
                        selected.is_none(),
                        busy,
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_agent_target(agent, None, expected_owner.clone(), cx)
                    })),
                );
            }
            if method == ConnectionMethod::SetupToken {
                form = form.child(s::description_text(
                    "Run claude setup-token in your terminal, then paste the generated token here.",
                ));
            }
            if method != ConnectionMethod::DeviceCode {
                let replacing = self
                    .agents
                    .selection
                    .as_ref()
                    .is_some_and(|b| b.connection.is_some());
                form=form.child(s::description_text(if replacing {"Verify a replacement for this task's exact selected connection. The previous credential remains active until verification succeeds. Existing running sessions may need Restart runtime afterward."}else{"Add a separate connection. Existing tasks keep their selected connection."}));
                if replacing {
                    let expected_owner = self.owner.clone();
                    form = form.child(
                        cs::button(
                            "add-separate-agent-credential",
                            "Add separate connection",
                            false,
                            busy,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_agent_target(agent, None, expected_owner.clone(), cx);
                            if !this.unfinished_agent_login() {
                                this.agents.edit = Some((agent, method));
                            }
                        })),
                    );
                    if !self.agents.supports_reconfigure {
                        form = form.child(s::description_text(
                            "Update this server to replace the selected credential safely.",
                        ));
                    }
                }
                form = form.child(self.agents.secret.clone());
            }
            if let Some(operation) = self.agents.operation.as_ref().filter(|o| o.agent == agent) {
                if matches!(
                    operation.state,
                    LoginState::Queued | LoginState::Authorizing
                ) {
                    let seconds = ((operation.expires_at - chrono::Utc::now().timestamp_millis())
                        .max(0)
                        / 1000) as u64;
                    form = form.child(s::description_text(format!(
                        "Sign-in session expires in {}:{:02}",
                        seconds / 60,
                        seconds % 60
                    )));
                    form = form.child(s::description_text(
                        if operation.verification_url.is_some() {
                            "Finish sign-in in your browser. You can close this panel and return."
                        } else {
                            if method == ConnectionMethod::DeviceCode {
                                "Preparing device sign-in…"
                            } else {
                                "Verifying the replacement credential…"
                            }
                        },
                    ));
                    if let Some(code) = &operation.verification_code {
                        let code = code.clone();
                        form = form.child(
                            cs::button(
                                "copy-agent-device-code",
                                SharedString::from(format!("Copy code · {code}")),
                                false,
                                false,
                                false,
                            )
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                            }),
                        );
                    }
                    if let Some(url) = operation
                        .verification_url
                        .as_ref()
                        .filter(|u| u.starts_with("https://"))
                    {
                        let url = url.clone();
                        form = form.child(
                            cs::button(
                                "open-agent-verification",
                                "Open sign-in page",
                                true,
                                false,
                                false,
                            )
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                        );
                    }
                    let id = operation.operation_id.clone();
                    form = form.child(
                        cs::button(
                            "cancel-agent-device-login",
                            "Cancel sign-in",
                            false,
                            busy,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.call(
                                "agent-cancel",
                                methods::CLOUD_AGENT_LOGIN_CANCEL,
                                json!({"operationId":id}),
                                cx,
                            )
                        })),
                    );
                } else if operation.state == LoginState::Expired {
                    form = form.child(s::description_text(
                        "This code expired. Start sign-in again for a new code.",
                    ));
                } else if operation.state == LoginState::Error {
                    form = form.child(s::description_text(match operation.error_code.as_deref() {
                        Some("auth_account_mismatch")=>"This sign-in used a different provider account. Reconnect using the original account, or choose Add another account to create a separate connection. Existing tasks were not switched.",
                        Some("auth_identity_review_required")=>"The existing connection has no verified provider identity. An operator must review it before it can be reused. Add another account creates a separate connection and does not switch existing tasks.",
                        _=>"Sign-in failed. Retry the selected connection or explicitly add another account.",
                    }));
                } else if operation.state == LoginState::Connected {
                    form = form.child(s::description_text("Account connected."));
                }
            }
            let pending = self.agents.operation.as_ref().is_some_and(|o| {
                o.agent == agent && matches!(o.state, LoginState::Queued | LoginState::Authorizing)
            });
            if !pending
                && !(method == ConnectionMethod::DeviceCode
                    && self
                        .agents
                        .selection
                        .as_ref()
                        .is_some_and(|binding| binding.connection.is_some())
                    && !self.agents.supports_reconnect)
                && !(method != ConnectionMethod::DeviceCode
                    && self
                        .agents
                        .selection
                        .as_ref()
                        .is_some_and(|b| b.connection.is_some())
                    && !self.agents.supports_reconfigure)
                && !self.unfinished_agent_login()
            {
                form = form.child(
                    cs::button(
                        "submit-agent-connection",
                        if method == ConnectionMethod::DeviceCode {
                            "Start sign-in"
                        } else {
                            if self
                                .agents
                                .selection
                                .as_ref()
                                .is_some_and(|b| b.connection.is_some())
                            {
                                "Verify replacement credential"
                            } else {
                                "Verify and add connection"
                            }
                        },
                        true,
                        busy,
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.submit_agent(cx))),
                );
            }
            if self.agents.login_binding.is_some() {
                form = form.child(
                    cs::button(
                        "refresh-agent-verification",
                        "Refresh verification status",
                        false,
                        busy,
                        false,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.call("agents", methods::CLOUD_AGENT_CONNECTIONS, json!({}), cx)
                    })),
                );
            }
            if let Some(connection) = self.agents.connections.iter().rev().find(|c| {
                c.agent == agent
                    && self.agents.managed_connection.as_deref() == Some(c.connection_id.as_str())
            }) {
                let expected_owner = self.owner.clone();
                let id = connection.connection_id.clone();
                form = form.child(
                    cs::button(
                        "disconnect-agent-connection",
                        "Disconnect",
                        false,
                        busy,
                        false,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.owner != expected_owner {
                            return;
                        }
                        this.call(
                            "agent-disconnect",
                            methods::CLOUD_AGENT_DISCONNECT,
                            json!({"connectionId":id}),
                            cx,
                        )
                    })),
                );
            }
            form = form.child(
                cs::button("close-agent-connection", "Done", false, false, false).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.agents.edit = None;
                        this.agents.recovery_open = false;
                        this.agents.recovery_target = None;
                        this.agents
                            .secret
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                        cx.notify();
                    }),
                ),
            );
            body = body.child(form);
        }
        if let Some(error) = &self.agents.error {
            body = body.child(
                div()
                    .p(px(12.))
                    .text_size(px(12.))
                    .text_color(s::destructive_text())
                    .child(error.clone()),
            );
        }
        cs::group(
            "Coding agents",
            Some("Connect once, then choose the account for each cloud task."),
            Some(
                cs::button("refresh-cloud-agents", "Refresh", false, busy, false)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.agents.error = None;
                        this.call("agents", methods::CLOUD_AGENT_CONNECTIONS, json!({}), cx);
                    }))
                    .into_any_element(),
            ),
            body,
        )
    }
}
