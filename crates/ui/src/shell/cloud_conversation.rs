//! A cloud conversation occupies the current native conversation surface.
//! Account history/admission and execution readiness have separate lifetimes.
use super::*;
use crate::state::CloudTarget;
use serde_json::{Value, json};
mod activity;
mod computer;
pub(super) use computer::Tone;
mod history;

pub(super) enum CloudConversationEvent {
    Terminal,
    Files,
    OpenFile(String),
}
pub(super) struct CloudConversation {
    pub(super) target: CloudTarget,
    pub(super) state: Entity<AppState>,
    pub(super) composer: Entity<Composer>,
    transcript: Entity<Transcript>,
    pub(super) terminal: Option<Entity<TerminalPanel>>,
    boot: EngineBootConfig,
    config: Value,
    status: Value,
    admissions: Value,
    pub(super) first_message: Option<String>,
    pub(super) title: String,
    error: Option<String>,
    authorized: bool,
    access_revoked: bool,
    history_read: Option<Task<()>>,
    history: history::State,
    read_again: bool,
    environment: Option<Task<()>>,
    account: Option<Task<()>>,
    events: Option<Task<()>>,
    execution: Option<Task<()>>,
    operation: Option<Task<()>>,
    sleep_command: Option<String>,
    computer: computer::State,
    _subscriptions: Vec<Subscription>,
}
impl gpui::EventEmitter<CloudConversationEvent> for CloudConversation {}

/// The transcripts of recently closed cloud conversations, so reopening one
/// shows its messages immediately instead of an empty chat while history
/// loads. Keyed by account and conversation; display only, never authority.
#[derive(Default)]
struct TranscriptCache {
    entries: std::collections::HashMap<String, Vec<zuse_model::SessionMessageEntry>>,
    order: std::collections::VecDeque<String>,
}
impl gpui::Global for TranscriptCache {}
impl TranscriptCache {
    const CAPACITY: usize = 24;
    fn key(target: &CloudTarget) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            target.subject,
            target.organization.as_deref().unwrap_or_default(),
            target.workspace,
            target.initial_session
        )
    }
    fn get(&self, key: &str) -> Option<Vec<zuse_model::SessionMessageEntry>> {
        self.entries.get(key).cloned()
    }
    fn put(&mut self, key: String, entries: Vec<zuse_model::SessionMessageEntry>) {
        self.order.retain(|existing| existing != &key);
        self.order.push_back(key.clone());
        self.entries.insert(key, entries);
        while self.order.len() > Self::CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}
impl CloudConversation {
    pub(super) fn new(
        target: CloudTarget,
        row: Value,
        launch: Value,
        boot: EngineBootConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = target.initial_session.clone();
        let cache_key = TranscriptCache::key(&target);
        let cached = cx.default_global::<TranscriptCache>().get(&cache_key);
        let state = cx.new(|_| {
            let mut state = AppState::new();
            // Account-owned rendering is available before a guest connects.
            state.connection = ConnectionStatus::Ready;
            state.selected_chat = Some(session);
            state.auto_selected = true;
            // The last transcript this app showed for the chat paints at
            // once; the history read replaces it, like any saved page.
            if let Some(entries) = cached {
                state.apply_transcript(entries);
            }
            state
        });
        let transcript = cx.new(|cx| Transcript::new(state.clone(), cx));
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        let mut config = row;
        if let (Some(config), Some(launch)) = (config.as_object_mut(), launch.as_object()) {
            for key in ["agent", "model", "reasoning", "modelOptions", "runtimeMode"] {
                if let Some(value) = launch.get(key) {
                    config.insert(key.into(), value.clone());
                }
            }
        }
        composer.update(cx, |composer, cx| {
            composer.set_cloud_task(target.clone(), config.clone(), cx);
            composer.focus_pending = true;
        });
        let subscriptions = vec![
            cx.on_release({
                let state = state.clone();
                move |_: &mut Self, cx: &mut App| {
                    let entries = state.read(cx).transcript.clone();
                    if !entries.is_empty() {
                        cx.default_global::<TranscriptCache>()
                            .put(cache_key, entries);
                    }
                }
            }),
            cx.subscribe(&composer, |this: &mut Self, _, event, cx| match event {
                ComposerEvent::CloudLaunchChanged => this.refresh_history(cx),
                ComposerEvent::CloudForegroundAdmitted => this.foreground_admitted(cx),
                ComposerEvent::CloudSleepRequested => this.sleep(cx),
                ComposerEvent::Sent {
                    chat_id,
                    message_id,
                } => this.transcript.update(cx, |transcript, cx| {
                    transcript.on_own_send(chat_id.clone(), message_id.clone(), cx)
                }),
                ComposerEvent::Queued {
                    chat_id,
                    message_id,
                } => this.transcript.update(cx, |transcript, cx| {
                    transcript.on_own_queued_send(chat_id.clone(), message_id.clone(), cx)
                }),
                ComposerEvent::WorkspaceCommand(crate::composer::WorkspaceCommand::Terminal) => {
                    cx.emit(CloudConversationEvent::Terminal)
                }
                ComposerEvent::WorkspaceCommand(crate::composer::WorkspaceCommand::Files) => {
                    cx.emit(CloudConversationEvent::Files)
                }
                ComposerEvent::OpenFile(path) => {
                    cx.emit(CloudConversationEvent::OpenFile(path.to_string()))
                }
                _ => {}
            }),
            cx.observe(&composer, |_: &mut Self, _, cx| cx.notify()),
            cx.observe(&state, |this: &mut Self, state, cx| {
                this.history.observe_live(state.read(cx));
                let transcript = state.read(cx).transcript.clone();
                this.composer.update(cx, |composer, cx| {
                    composer.apply_cloud_response_activity(&transcript, cx)
                });
                cx.notify();
            }),
        ];
        let first_message = launch["firstMessage"].as_str().map(str::to_owned);
        let title = config["title"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| first_message.clone())
            .unwrap_or_else(|| "Cloud chat".into());
        let initial_metadata = config.clone();
        let mut this = Self {
            target,
            state,
            composer,
            transcript,
            terminal: None,
            boot,
            config,
            status: json!({"state":"connecting","message":"Opening cloud conversation…"}),
            admissions: Value::Null,
            first_message,
            title,
            error: None,
            authorized: false,
            access_revoked: false,
            history_read: None,
            history: history::State::default(),
            read_again: false,
            environment: None,
            account: None,
            events: None,
            execution: None,
            operation: None,
            sleep_command: None,
            computer: computer::State::default(),
            _subscriptions: subscriptions,
        };
        this.computer.metadata = initial_metadata;
        this.watch_account(cx);
        // Paint the saved copy now; the fresh read follows the account check.
        this.read_provisional_history(cx);
        this
    }
    #[cfg(test)]
    fn activity(&self, cx: &App) -> Option<activity::Activity> {
        self.computer_activity(cx)
            .or_else(|| self.agent_activity(cx))
    }
    /// The cloud computer's own stage (starting, cloning, environment).
    fn computer_activity(&self, cx: &App) -> Option<activity::Activity> {
        if self.access_revoked {
            return None;
        }
        let state = self.state.read(cx);
        let launch = self.admissions.as_array().and_then(|rows| {
            rows.iter()
                .find(|row| row["commandId"] == format!("launch:{}", self.target.workspace))
        });
        // Before the first history receipt, this newly admitted session's first
        // canonical user is its launch message. Never credit an older assistant.
        let initial_id = launch
            .and_then(|row| row["messageId"].as_str())
            .or_else(|| {
                state
                    .transcript
                    .iter()
                    .find(|entry| entry.role == zuse_model::MessageRole::User)
                    .map(|entry| entry.id.as_str())
            });
        let response_started = initial_id
            .is_some_and(|id| zuse_viewport::cloud_response_started(&state.transcript, id));
        let initial = !response_started
            && (self.first_message.is_some()
                || launch.is_some_and(|row| {
                    matches!(
                        row["executionState"].as_str(),
                        Some("queued" | "accepted" | "admitted")
                    )
                }));
        activity::preparation(&self.computer.metadata, initial)
    }
    /// The message's progress once it is with the agent.
    fn agent_activity(&self, cx: &App) -> Option<activity::Activity> {
        if self.access_revoked {
            return None;
        }
        self.composer
            .read(cx)
            .cloud_activity()
            .map(|(label, busy)| activity::Activity {
                label: label.into(),
                next: None,
                busy,
            })
            .or_else(|| activity::command(&self.admissions, &self.state.read(cx).transcript))
    }
    /// Id of the stand-in for the launch prompt until history delivers it.
    fn launch_echo_id(&self) -> String {
        format!("launch-echo:{}", self.target.workspace)
    }
    /// Put progress where an ordinary chat shows it: the prompt as the first
    /// message right away, the computer's stage in the composer's status
    /// line, and the agent's progress in the working line under the messages.
    /// Every setter ignores unchanged values, so this is safe on each frame.
    fn sync_progress(&mut self, cx: &mut Context<Self>) {
        let pending = activity::pending_initial_prompt(
            self.first_message.as_deref(),
            &self.admissions,
            &self.state.read(cx).transcript,
        );
        let echo = self.launch_echo_id();
        let session = self.state.read(cx).selected_chat.clone();
        if let Some(session) = session {
            let shown = self
                .state
                .read(cx)
                .pending_echoes()
                .iter()
                .any(|entry| entry.id == echo);
            match (pending, shown) {
                (Some(text), false) => self.state.update(cx, |state, cx| {
                    let device = state.local_device_id.clone().unwrap_or_default();
                    state.push_echo(
                        &session,
                        zuse_model::SessionMessageEntry {
                            id: echo.clone(),
                            role: zuse_model::MessageRole::User,
                            parts: vec![zuse_model::MessagePart::Text {
                                id: format!("text:{echo}"),
                                text,
                            }],
                            created_at: chrono::Utc::now().timestamp_millis(),
                            device_id: device,
                            status: None,
                            continuation_of: None,
                            duration_ms: None,
                        },
                    );
                    cx.notify();
                }),
                (None, true) => self.state.update(cx, |state, cx| {
                    state.remove_echo(&session, &echo);
                    cx.notify();
                }),
                _ => {}
            }
        }
        let computer = self.computer_activity(cx);
        let agent = self.agent_activity(cx);
        // Issues (a failed stage, a message that needs checking) stay in the
        // status line; routine progress is the working line's.
        let status = computer
            .as_ref()
            .map(|stage| (stage.label.clone(), stage.busy))
            .or_else(|| {
                agent
                    .as_ref()
                    .filter(|activity| !activity.busy)
                    .map(|activity| (activity.label.clone(), false))
            });
        let line = if computer.as_ref().is_some_and(|stage| stage.busy) {
            Some(SharedString::from("Waiting for computer…"))
        } else {
            agent
                .filter(|activity| activity.busy)
                .map(|activity| SharedString::from(activity.label))
        };
        self.composer
            .update(cx, |composer, cx| composer.set_cloud_status(status, cx));
        self.transcript
            .update(cx, |transcript, cx| transcript.set_external_activity(line, cx));
    }

    pub(super) fn branch_label(&self) -> Option<String> {
        if self.access_revoked {
            return None;
        }
        self.computer.metadata["branch"]
            .as_str()
            .or_else(|| self.config["branch"].as_str())
            .filter(|branch| !branch.trim().is_empty())
            .map(str::to_owned)
    }

    /// The same cloud icon placement as an ordinary conversation. Computer
    /// details never consume a second header or push the transcript down.
    pub(super) fn render_header_control(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let open = self.computer.open;
        let mut button = header_icon_button(
            "native-cloud-computer",
            icons::DITHER_CLOUD,
            "Cloud computer",
            &theme,
            cx.listener(|this, _, _, cx| {
                if this.computer.menu.take_press_was_open() {
                    this.close_computer(cx);
                } else {
                    this.toggle_computer(cx);
                }
            }),
        )
        .role(gpui::Role::Button)
        .aria_label("Cloud computer")
        .aria_expanded(open)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, _| {
                window.prevent_default();
                this.computer.menu.note_trigger_press();
            }),
        );
        if self.computer.menu.get().is_some() {
            let panel = self.render_computer(cx);
            let card = popover::popover_card(&theme)
                .id("native-cloud-computer-card")
                .w(px(360.))
                .max_h(px(560.))
                .overflow_y_scroll()
                .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_computer(cx)))
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_computer(cx);
                        cx.stop_propagation();
                    }
                }))
                .child(panel);
            button = button.relative().child(popover::anchored_menu_below_end(
                "native-cloud-computer-menu",
                card.into_any_element(),
                self.computer.menu.closing_since(),
            ));
        }
        button.into_any_element()
    }
    pub(super) fn project_id(&self) -> Option<&str> {
        self.config["projectId"].as_str()
    }
    fn params(&self) -> Value {
        json!({"workspaceId":self.target.workspace,"sessionId":self.target.initial_session,
            "expectedAccountId":self.target.subject,"expectedOrganizationId":self.target.organization})
    }
    pub(super) fn ready(&self, cx: &App) -> bool {
        self.authorized
            && !self.domain_sleeping()
            && self.status["state"] == "ready"
            && self.computer.execution_controls_enabled()
            && self.state.read(cx).engine().is_some()
            && matches!(self.state.read(cx).connection, ConnectionStatus::Ready)
    }
    fn watch_account(&mut self, cx: &mut Context<Self>) {
        let target = self.target.clone();
        self.account = Some(cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                .await;
            let Ok(mut stream) = result else {
                let _ = this.update(cx, |this, cx| this.revoke(cx));
                return;
            };
            while let Some(value) = stream.recv().await {
                if value["user"]["id"].as_str() != Some(target.subject.as_str())
                    || value["orgId"].as_str() != target.organization.as_deref()
                {
                    let _ = this.update(cx, |this, cx| this.revoke(cx));
                    return;
                }
                let _ = this.update(cx, |this, cx| {
                    if !this.authorized {
                        this.authorized = true;
                        this.access_revoked = false;
                        this.read_provisional_history(cx);
                        this.refresh_history(cx);
                        this.watch_environment(false, cx);
                        this.watch_events(cx);
                    }
                });
            }
            let _ = this.update(cx, |this, cx| this.revoke(cx));
        }));
    }
    fn revoke(&mut self, cx: &mut Context<Self>) {
        self.authorized = false;
        self.access_revoked = true;
        self.admissions = Value::Null;
        self.environment = None;
        self.events = None;
        self.history_read = None;
        self.history = history::State::default();
        self.execution = None;
        self.operation = None;
        self.computer = computer::State::default();
        self.state
            .update(cx, |state, cx| state.prepare_runtime_replacement(cx));
        self.composer
            .update(cx, |composer, cx| composer.clear_cloud_task(cx));
        self.title = "Cloud chat".into();
        self.first_message = None;
        self.error = Some(
            "Cloud access changed. Select this conversation from its original account.".into(),
        );
        cx.notify();
    }
    fn revoke_for_history_error(&mut self, error: &zuse_rpc::RpcError, cx: &mut Context<Self>) {
        self.revoke(cx);
        if matches!(error, zuse_rpc::RpcError::Capability(code) if code == "workspace_not_found") {
            self.error = Some("This conversation is no longer available. Choose another conversation or start a new chat.".into());
        }
    }
    fn watch_events(&mut self, cx: &mut Context<Self>) {
        let target = self.target.clone();
        self.events=Some(cx.spawn(async move |this,cx| {
            let result=target.controller.client().subscribe_scoped(methods::CLOUD_EVENTS,
                json!({"expectedAccountId":target.subject,"expectedOrganizationId":target.organization})).await;
            let Ok(mut stream)=result else{return};
            while let Some(value)=stream.recv().await {
                if value["accountId"]!=target.subject || value["organizationId"].as_str()!=target.organization.as_deref(){continue;}
                let data=&value["data"];
                let reset = data["type"] == "reset"
                    && data.get("reason").is_none_or(|reason| reason.is_null()
                        || matches!(reason.as_str(), Some("cursor_expired" | "consumer_lagged")));
                if data["revokedWorkspaceIds"].as_array().is_some_and(|ids|ids.iter().any(|id|id==&target.workspace))
                    || matches!(data["type"].as_str(), Some("error" | "closed"))
                    || (data["cacheRevoked"] == true && !reset) {
                    let _=this.update(cx,|this,cx|this.revoke(cx));return;
                }
                let dirty=matches!(data["type"].as_str(),Some("ready"|"reset"|"revalidate")) || data["events"].as_array().is_some_and(|events|
                    events.iter().any(|event|event["workspaceId"]==target.workspace||event["resourceId"]==target.workspace));
                if dirty {let _=this.update(cx,|this,cx|this.refresh_history(cx));}
            }
        }));
    }
    fn refresh_history(&mut self, cx: &mut Context<Self>) {
        if !self.authorized {
            return;
        }
        if self.history_read.is_some() {
            self.read_again = true;
            return;
        }
        let target = self.target.clone();
        let mut params = self.params();
        params["incremental"] = json!(true);
        self.history_read = Some(cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .call_scoped(methods::CLOUD_HISTORY, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.history_read = None;
                if !this.authorized || this.access_revoked {
                    return;
                }
                match result {
                    Ok(value)
                        if value["accountId"] == target.subject
                            && value["organizationId"].as_str()
                                == target.organization.as_deref() =>
                    {
                        this.apply_history(value, cx)
                    }
                    Ok(_) => this.revoke(cx),
                    Err(error) if history::access_revoked(&error) => {
                        this.revoke_for_history_error(&error, cx)
                    }
                    Err(error) => {
                        this.error = Some(error.to_string());
                        cx.notify();
                    }
                }
                if std::mem::take(&mut this.read_again) {
                    this.refresh_history(cx);
                }
            });
        }));
    }
    fn apply_history(&mut self, value: Value, cx: &mut Context<Self>) {
        // A validated fresh receipt wins even if the local cache finishes later.
        self.history.fresh_received = true;
        self.history.provisional_read = None;
        self.history.provisional_request = None;
        if std::mem::take(&mut self.history.provisional_painted) {
            // Cached cursors are provisional too: they cannot reject a fresh
            // authoritative receipt as stale after a local snapshot replay.
            self.history.head = None;
            self.history.older_read = None;
        }
        let decoded = serde_json::from_value::<zuse_model::cloud::Head>(value["head"].clone());
        if decoded.as_ref().is_ok_and(|head| self.history.stale(head)) {
            return;
        }
        self.admissions = value["commandAdmissions"].clone();
        self.composer.update(cx, |composer, cx| {
            composer.apply_cloud_admission_activity(&self.admissions, cx)
        });
        self.apply_computer_metadata(&value["workspace"], cx);
        let Ok(head) = decoded else {
            self.error = Some(
                "This conversation's saved history is unavailable. Refresh to try again.".into(),
            );
            cx.notify();
            return;
        };
        if head.workspace_id != self.target.workspace
            || head.session_id != self.target.initial_session
        {
            self.revoke(cx);
            return;
        }
        let Some((head, projection_changed)) = self.history.latest(head) else {
            return;
        };
        if projection_changed {
            self.execution = None;
            self.state
                .update(cx, |state, cx| state.prepare_runtime_replacement(cx));
        }
        if let Some(title) = head
            .chat
            .title
            .as_ref()
            .filter(|title| !title.trim().is_empty())
        {
            self.title = title.clone();
        }
        self.first_message = None;
        self.error = None;
        if let Some(config) = &head.chat.config {
            self.config["agent"] = serde_json::to_value(config.harness).unwrap_or_default();
            self.config["model"] = json!(config.model);
            self.config["reasoning"] = serde_json::to_value(config.reasoning).unwrap_or_default();
            self.config["sandbox"] = serde_json::to_value(config.sandbox).unwrap_or_default();
            self.config["modelOptions"] = json!(config.model_options);
            self.config["runtimeMode"] = json!(match config.sandbox {
                zuse_protocol::SandboxLevel::DangerFullAccess => "full-access",
                zuse_protocol::SandboxLevel::Auto => "auto",
                _ => "approval-required",
            });
        }
        self.config["queueRunSelectionVersion"] = value["queueRunSelectionVersion"].clone();
        // Never inject saved pages into the live watch's delta baseline.
        self.project_saved_history(&head, cx);
        self.composer.update(cx, |composer, cx| {
            composer.set_cloud_task(self.target.clone(), self.config.clone(), cx)
        });
        if self.authorized && !self.domain_sleeping() && self.status["state"] == "ready" {
            self.connect_execution(cx);
        }
        cx.notify();
    }
    fn domain_sleeping(&self) -> bool {
        matches!(
            self.computer.metadata["state"].as_str(),
            Some("paused" | "pausing" | "archived" | "archiving")
        ) || matches!(
            self.computer.metadata["desiredState"].as_str(),
            Some("paused" | "archived")
        )
    }
    fn detach_execution(&mut self, cx: &mut Context<Self>) {
        self.history.observe_live(self.state.read(cx));
        self.history.materialize_live(self.state.read(cx));
        self.execution = None;
        self.state.update(cx, |state, cx| {
            state.detach_cloud_conversation_execution(cx)
        });
    }
    fn apply_computer_metadata(&mut self, row: &Value, cx: &mut Context<Self>) {
        if row["workspaceId"] != self.target.workspace {
            return;
        }
        let refresh_logs = self.computer.observe_metadata(row.clone());
        let sleeping = self.domain_sleeping();
        self.composer
            .update(cx, |composer, _| composer.set_cloud_task_sleeping(sleeping));
        if sleeping && (self.execution.is_some() || self.state.read(cx).engine().is_some()) {
            self.detach_execution(cx);
        }
        if refresh_logs {
            self.refresh_computer(cx);
        }
    }
    fn foreground_admitted(&mut self, cx: &mut Context<Self>) {
        if !self.authorized || self.access_revoked {
            return;
        }
        // A durable Run already owns the wake. This only replaces a retired
        // attachment; neither history nor queue/config changes can resume it.
        if !self.ready(cx) {
            self.environment = None;
            self.detach_execution(cx);
            self.watch_environment(false, cx);
        }
        self.refresh_history(cx);
    }
    fn watch_environment(&mut self, interactive: bool, cx: &mut Context<Self>) {
        if !self.authorized
            || interactive
                && (self.operation.is_some() || !self.computer.execution_controls_enabled())
        {
            return;
        }
        let target = self.target.clone();
        let mut params = self.params();
        params["interactiveResume"] = json!(interactive);
        self.environment = Some(cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .subscribe_scoped(methods::CLOUD_ENVIRONMENT, params)
                .await;
            let Ok(mut stream) = result else {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(
                        "Computer connection interrupted. Reconnect to check its current state."
                            .into(),
                    );
                    cx.notify();
                });
                return;
            };
            while let Some(status) = stream.recv().await {
                let _ = this.update(cx, |this, cx| {
                    let ready = status["state"] == "ready";
                    let sleeping = this.domain_sleeping() || status["state"] == "paused";
                    // Retired transport says blocked, not paused. It cannot
                    // erase the authoritative retained computer lifecycle.
                    if sleeping || ready {
                        this.composer
                            .update(cx, |composer, _| composer.set_cloud_task_sleeping(sleeping));
                    }
                    this.status = status;
                    if ready && !this.domain_sleeping() {
                        this.connect_execution(cx);
                    }
                    cx.notify();
                });
            }
        }));
    }
    fn connect_execution(&mut self, cx: &mut Context<Self>) {
        if !self.authorized || self.execution.is_some() {
            return;
        }
        if let Some(engine) = self.state.read(cx).engine() {
            if !matches!(self.state.read(cx).connection, ConnectionStatus::Ready) {
                engine.retry_connection();
            }
            return;
        }
        let mut boot = self.boot.clone();
        boot.target = Some(crate::state::RemoteTarget::Cloud(self.target.clone()));
        let session = self.target.initial_session.clone();
        let connect = Tokio::spawn(cx, crate::state::EngineHandle::bootstrap(boot));
        self.execution=Some(cx.spawn(async move |this,cx| {
            let result=connect.await;
            let _=this.update(cx,|this,cx| {
                this.execution=None;
                if !this.authorized{return;}
                match result {
                    Ok(Ok(handle))=> {
                        this.history.start_live(this.state.read(cx).transcript_revision);
                        this.state.update(cx,|state,cx|state.attach_cloud_conversation(handle,session,cx));
                    },
                    _=>this.error=Some("The conversation is saved, but the computer tools could not connect. Reconnect to try again.".into()),
                }
                cx.notify();
            });
        }));
    }
    fn sleep(&mut self, cx: &mut Context<Self>) {
        if !self.authorized
            || self.operation.is_some()
            || !self.computer.execution_controls_enabled()
        {
            return;
        }
        self.environment = None;
        self.detach_execution(cx);
        self.status = json!({"state":"connecting","message":"Putting computer to sleep…"});
        let target = self.target.clone();
        let mut params = self.params();
        params["commandId"] = json!(
            self.sleep_command
                .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
        );
        self.operation = Some(cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .call_scoped(methods::CLOUD_PAUSE, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.operation = None;
                match result {
                    Ok(value) => {
                        this.sleep_command = None;
                        this.apply_computer_metadata(&value["workspace"], cx);
                    }
                    Err(error) => {
                        this.error = Some(error.to_string());
                        this.watch_environment(false, cx);
                    }
                }
                this.refresh_history(cx);
                cx.notify();
            });
        }));
    }
}
impl Render for CloudConversation {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        self.sync_progress(cx);
        let composer_visible = self.authorized && self.computer.composer_enabled();
        let composer_width =
            settings::current(cx).transcript_width + 2. * crate::composer::COMPOSER_OUTER_PAD;
        let notice = self.error.clone();
        let show_tray = notice.is_some();
        div()
            .id("native-cloud-conversation")
            .size_full()
            .flex()
            .flex_col()
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .children(self.render_older_history(cx))
            .child(div().flex_1().min_h_0().child(self.transcript.clone()))
            .when(show_tray || composer_visible, |view| {
                view.child(
                    div()
                        .w_full()
                        .max_w(px(composer_width))
                        .mx_auto()
                        .pb(px(12.))
                        .when(show_tray, |stack| {
                            stack.child(
                                div()
                                    .id("native-cloud-queue-tray")
                                    .debug_selector(|| "native-cloud-queue-tray".into())
                                    .mx(px(24.))
                                    .rounded_t(px(14.))
                                    .border_1()
                                    .border_b_0()
                                    .border_color(theme.border)
                                    .bg(theme.bg)
                                    .overflow_hidden()
                                    .when_some(notice, |tray, text| {
                                        tray.child(
                                            div()
                                                .id("native-cloud-conversation-notice")
                                                .px(px(12.))
                                                .py(px(8.))
                                                .text_size(px(12.))
                                                .text_color(theme.text_muted)
                                                .child(text),
                                        )
                                    }),
                            )
                        })
                        .when(composer_visible, |stack| stack.child(self.composer.clone())),
                )
            })
    }
}

impl Shell {
    /// A new cloud chat between Send and the conversation opening. It already
    /// reads as the conversation it becomes: the prompt is the first message,
    /// with the working line under it, laid out where the transcript puts
    /// them, so nothing moves when the conversation takes over.
    pub(super) fn render_cloud_launch_outlet(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let Some(prompt) = self.composer.read(cx).cloud_launch_prompt() else {
            return div().into_any_element();
        };
        let width = crate::settings::transcript_width(cx);
        let clearance = (self.bottom_stack.get() - self.terminal_geometry.get().height).max(0.0)
            + Theme::TRANSCRIPT_FADE_BAND
            + 8.0;
        div()
            .id("cloud-launch-outlet")
            .debug_selector(|| "cloud-launch-outlet".into())
            .size_full()
            .flex()
            .flex_col()
            .justify_end()
            .pb(px(clearance))
            .child(
                div().w_full().flex().justify_center().px(px(48.0)).child(
                    div()
                        .w_full()
                        .max_w(px(width))
                        .min_w_0()
                        .child(
                            div().w_full().flex().justify_end().child(
                                div()
                                    .id("cloud-launch-prompt")
                                    .debug_selector(|| "cloud-launch-prompt".into())
                                    .min_w_0()
                                    .max_w(px(width * 0.8))
                                    .bg(theme.user_bubble_fill())
                                    .rounded(px(12.0))
                                    .rounded_tr(px(4.0))
                                    .px(px(14.0))
                                    .py(px(8.0))
                                    .text_size(crate::typography::text_md())
                                    .line_height(crate::typography::ui_rems(
                                        crate::transcript::USER_LINE_HEIGHT,
                                    ))
                                    .text_color(theme.text)
                                    .opacity(0.65)
                                    .child(prompt),
                            ),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(Theme::SPACE_SM))
                                .pt(px(Theme::SPACE_LG))
                                .child(crate::loaders::mini_glyph_spinner(
                                    "cloud-launch-working",
                                    2.5,
                                    theme.glyph,
                                    cx.entity_id(),
                                    cx,
                                ))
                                .child(
                                    div()
                                        .text_size(crate::typography::text_xs())
                                        .text_color(theme.text_muted)
                                        .child("Waiting for computer…"),
                                ),
                        ),
                ),
            )
            .into_any_element()
    }

    /// Cloud conversations keep their selected session in task state, not the
    /// navigation controller. Both kinds need the same chat chrome.
    pub(super) fn has_conversation(&self, cx: &App) -> bool {
        self.cloud_conversation.is_some() || self.state.read(cx).selected_chat_row().is_some()
    }

    /// Focus and commands follow the displayed conversation, while navigation
    /// keeps its original controller composer and drafts.
    pub(super) fn conversation_composer(&self, cx: &App) -> Entity<Composer> {
        self.cloud_conversation
            .as_ref()
            .map(|view| view.read(cx).composer.clone())
            .unwrap_or_else(|| self.composer.clone())
    }

    /// Task tools never inherit the local controller's execution target.
    pub(super) fn conversation_state(&self, cx: &App) -> Entity<AppState> {
        self.cloud_conversation
            .as_ref()
            .map(|view| view.read(cx).state.clone())
            .unwrap_or_else(|| self.state.clone())
    }
    pub(super) fn subscribe_cloud_conversation(
        &mut self,
        view: &Entity<CloudConversation>,
        cx: &mut Context<Self>,
    ) {
        self.cloud_conversation_events =
            Some(cx.subscribe(view, |this, _, event, cx| match event {
                CloudConversationEvent::Terminal => this.add_terminal_surface(cx),
                CloudConversationEvent::Files => {
                    this.pending_workspace_command = Some(crate::composer::WorkspaceCommand::Files);
                    cx.notify();
                }
                CloudConversationEvent::OpenFile(path) => {
                    this.pending_chip_file = Some(path.clone());
                    cx.notify();
                }
            }));
    }
    pub(super) fn open_cloud_conversation(
        &mut self,
        target: CloudTarget,
        row: Value,
        launch: Value,
        cx: &mut Context<Self>,
    ) {
        self.sync_cloud_tab_scope(cx);
        let state = self.state.read(cx);
        if state.cloud_user_id() != Some(target.subject.as_str())
            || state.cloud_organization_id() != target.organization.as_deref()
        {
            return;
        }
        if !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        if self.cloud_conversation.as_ref().is_some_and(|view| {
            view.read(cx).target.subject == target.subject
                && view.read(cx).target.organization == target.organization
                && view.read(cx).target.workspace == target.workspace
                && view.read(cx).target.initial_session == target.initial_session
        }) {
            self.state
                .update(cx, |state, cx| state.select_chat(None, cx));
            self.set_route(Route::Chat, cx);
            return;
        }
        let retained = self.cloud_tabs.retained_index(&target, cx);
        if let Some(index) = retained {
            self.select_cloud_tab(index, cx);
            return;
        }
        if self.cloud_tabs.is_empty() {
            self.track_current_cloud_canvas(cx);
        }
        let session = target.initial_session.clone();
        // The computer record names its project only by id; carry the
        // repository's name in from the account's project list.
        let mut row = row;
        let project_id = row["projectId"]
            .as_str()
            .or_else(|| launch["projectId"].as_str())
            .map(str::to_owned);
        if let Some(project) = project_id.and_then(|id| {
            self.state
                .read(cx)
                .cloud_sidebar_projects()
                .into_iter()
                .find(|project| project["projectId"].as_str() == Some(id.as_str()))
        }) && let Some(object) = row.as_object_mut()
        {
            for key in ["displayName", "repositoryIdentity"] {
                if let Some(value) = project.get(key).filter(|value| value.is_string()) {
                    let field = if key == "displayName" {
                        "repositoryDisplayName"
                    } else {
                        key
                    };
                    object.entry(field).or_insert_with(|| value.clone());
                }
            }
        }
        let view = cx.new(|cx| CloudConversation::new(target, row, launch, self.boot.clone(), cx));
        self.subscribe_cloud_conversation(&view, cx);
        self.cloud_tabs.set_current(view.clone(), cx);
        self.cloud_conversation = Some(view);
        self.active_chat = session;
        self.state
            .update(cx, |state, cx| state.select_chat(None, cx));
        self.set_route(Route::Chat, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    #[test]
    fn transcript_cache_keeps_the_most_recent_conversations() {
        let mut cache = TranscriptCache::default();
        let entry = |id: &str| zuse_model::SessionMessageEntry {
            id: id.into(),
            role: zuse_model::MessageRole::User,
            parts: vec![],
            created_at: 0,
            device_id: String::new(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        };
        for n in 0..=TranscriptCache::CAPACITY {
            cache.put(format!("chat-{n}"), vec![entry(&format!("m{n}"))]);
        }
        assert!(cache.get("chat-0").is_none(), "the oldest conversation is evicted");
        // Revisiting refreshes a conversation's place.
        cache.put("chat-1".into(), vec![entry("again")]);
        cache.put("extra".into(), vec![entry("x")]);
        assert_eq!(cache.get("chat-1").unwrap()[0].id, "again");
        assert!(cache.get("chat-2").is_none());
    }
    struct PendingController;
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for PendingController {
        async fn handle(
            &self,
            _method: &str,
            _params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                futures::stream::pending(),
            )))
        }
    }
    fn initialize(cx: &mut App) {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
    }
    struct ChatSurface(Entity<CloudConversation>);
    impl Render for ChatSurface {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let header = self.0.update(cx, |view, cx| view.render_header_control(cx));
            div()
                .relative()
                .w(px(1000.))
                .h(px(720.))
                .bg(Theme::of(cx).bg)
                .child(self.0.clone())
                .child(div().absolute().top(px(4.)).right(px(12.)).child(header))
        }
    }
    #[gpui::test]
    fn cloud_chat_uses_one_composer_tray_and_an_icon_menu(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init(cx);
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let directory = tempfile::tempdir().unwrap();
        let view = cx.new(|cx| CloudConversation::new(CloudTarget {
            controller, subject: "owner".into(), organization: None, workspace: "task".into(), initial_session: "session".into(),
        }, json!({"workspaceId":"task","state":"provisioning","agent":"codex","model":"gpt-6.1-sol"}), json!({"firstMessage":"Build a small website"}), boot(directory.path()), cx));
        view.update(cx, |view, _| {
            view.account = None;
            view.authorized = true;
        });
        let window = cx.add_window(|_, _| ChatSurface(view.clone()));
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            assert!(
                visual.debug_bounds("native-cloud-queue-tray").is_none(),
                "the prompt is never parked in a queue card"
            );
            assert!(
                visual.debug_bounds("native-cloud-terminal").is_none(),
                "no permanent cloud toolbar"
            );
            let button = visual
                .debug_bounds("native-cloud-computer")
                .expect("normal header icon");
            visual.simulate_click(button.center(), gpui::Modifiers::default());
        }
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            let echoes = view.state.read(cx).pending_echoes();
            assert_eq!(echoes.len(), 1, "the prompt is the conversation's first message");
            assert!(matches!(
                &echoes[0].parts[..],
                [zuse_model::MessagePart::Text { text, .. }] if text == "Build a small website"
            ));
            assert_eq!(
                view.composer.read(cx).cloud_status(),
                Some(&("Starting computer".to_owned(), true)),
                "the computer's stage is the composer's one status line"
            );
        });
        assert!(view.read_with(cx, |view, _| view.computer.open));
        view.update(cx, |view, cx| {
            view.close_computer(cx);
        });
        assert!(!view.read_with(cx, |view, _| view.computer.open));
    }

    fn boot(path: &std::path::Path) -> EngineBootConfig {
        EngineBootConfig {
            target: None,
            data_dir: path.into(),
            ipc_port: 0,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: None,
            default_harness: zuse_protocol::HarnessId::Mock,
        }
    }
    fn account(state: &mut AppState) -> Value {
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        let project = json!({"projectId":"project","accountId":"owner","repositoryIdentity":"github.com/owner/repo","displayName":"repo"});
        assert!(state.apply_cloud_sidebar_catalog(
            &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[project]}),
            &json!({"accountId":"owner","organizationId":null,"workspaces":[]})));
        project
    }
    struct PreparationLogController {
        requests: std::sync::atomic::AtomicUsize,
        release: tokio::sync::Semaphore,
        metadata: std::sync::Mutex<Value>,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for PreparationLogController {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            if method != methods::CLOUD_WORKSPACE {
                return Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::pending(),
                )));
            }
            assert_eq!(params["workspaceId"], "task");
            assert_eq!(params["includePreparationLogs"], true);
            self.requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.release.acquire().await.unwrap().forget();
            Ok(zuse_rpc::RpcReply::Value(
                json!({"accountId":"owner","organizationId":null,"workspace":self.metadata.lock().unwrap().clone(),"preparationLogs":{"log":{"stdout":"Latest initialization output","stderr":""}}}),
            ))
        }
    }
    #[gpui::test]
    fn open_computer_logs_follow_stage_events_with_one_coalesced_trailing_refresh(
        cx: &mut TestAppContext,
    ) {
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(initialize);
        let service = Arc::new(PreparationLogController {
            requests: AtomicUsize::new(0),
            release: tokio::sync::Semaphore::new(0),
            metadata: Mutex::new(json!({"workspaceId":"task","state":"provisioning","revision":1})),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let directory = tempfile::tempdir().unwrap();
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task"}),
                Value::Null,
                boot(directory.path()),
                cx,
            )
        });
        view.update(cx,|view,cx|{
            view.account=None;view.authorized=true;
            view.apply_computer_metadata(&json!({"workspaceId":"task","state":"provisioning","revision":1,"preparation":{"repository":{"state":"running"}}}),cx);
        });
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        assert_eq!(
            service.requests.load(Ordering::SeqCst),
            0,
            "closed Details never fetches setup output on stage events"
        );
        view.update(cx, |view, cx| view.toggle_computer(cx));
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        assert_eq!(service.requests.load(Ordering::SeqCst), 1);
        let latest = json!({"workspaceId":"task","state":"provisioning","revision":3,"preparation":{"repository":{"state":"completed"},"environment":{"state":"running"},"credentials":{"state":"running"}}});
        *service.metadata.lock().unwrap() = latest.clone();
        view.update(cx,|view,cx|{
            view.apply_computer_metadata(&json!({"workspaceId":"task","state":"provisioning","revision":2,"preparation":{"repository":{"state":"completed"}}}),cx);
            view.apply_computer_metadata(&latest,cx);
        });
        assert_eq!(
            service.requests.load(Ordering::SeqCst),
            1,
            "changes while a request is active only mark it dirty"
        );
        service.release.add_permits(1);
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        assert_eq!(
            service.requests.load(Ordering::SeqCst),
            2,
            "multiple live stage changes produce one trailing refresh"
        );
        service.release.add_permits(1);
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        view.update(cx, |view, cx| {
            assert_eq!(
                view.computer.setup_output_for_test()["stdout"],
                "Latest initialization output"
            );
            view.toggle_computer(cx);
            view.apply_computer_metadata(
                &json!({"workspaceId":"task","state":"ready","revision":4}),
                cx,
            );
        });
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        assert_eq!(
            service.requests.load(Ordering::SeqCst),
            2,
            "closed Details do not poll or retain a refresh loop"
        );
        view.update(cx, |view, cx| view.revoke(cx));
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
    }
    struct RetiredEnvironmentController {
        awake: std::sync::atomic::AtomicBool,
        environment_calls: std::sync::Mutex<Vec<Value>>,
        requests: std::sync::Mutex<Vec<Value>>,
        admit: tokio::sync::Semaphore,
    }
    fn retained_history(awake: bool) -> Value {
        json!({"accountId":"owner","organizationId":null,"workspace":{"workspaceId":"task","providerId":"boxd","state":if awake{"ready"}else{"paused"},"desiredState":if awake{"ready"}else{"paused"}},"head":{"schemaVersion":1,"payloadFormat":"fixture","workspaceId":"task","sessionId":"session","runtimeGeneration":1,"cursor":{"epoch":"fixture","version":1},"chat":{"id":"session","deviceId":"cloud:task","archived":false,"createdAt":"2026-10-31T00:00:00Z","title":"Retained work"},"conversations":[],"transcript":{"reset":[{"id":"saved-user","role":"user","parts":[{"id":"saved-text","kind":"text","text":"Earlier saved work"}],"createdAt":1,"deviceId":"cloud:task"}]},"queue":[],"queuePaused":false,"olderMessageSequence":null}})
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for RetiredEnvironmentController {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            use futures::StreamExt as _;
            let pending = |initial: Value| {
                zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::once(async move { initial }).chain(futures::stream::pending()),
                ))
            };
            match method {
                methods::CLOUD_ACCOUNT_STATUS => {
                    Ok(pending(json!({"user":{"id":"owner"},"orgId":null})))
                }
                methods::CLOUD_EVENTS => Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::pending(),
                ))),
                methods::CLOUD_HISTORY => Ok(zuse_rpc::RpcReply::Value(retained_history(
                    self.awake.load(std::sync::atomic::Ordering::SeqCst),
                ))),
                methods::CLOUD_ENVIRONMENT => {
                    self.environment_calls.lock().unwrap().push(params);
                    Ok(pending(
                        if self.awake.load(std::sync::atomic::Ordering::SeqCst) {
                            json!({"state":"ready","epoch":2})
                        } else {
                            json!({"state":"blocked","epoch":1,"message":zuse_engine::cloud::environment::PAUSING_MESSAGE})
                        },
                    ))
                }
                methods::CLOUD_TASK_COMMAND => {
                    self.requests.lock().unwrap().push(params.clone());
                    self.admit.acquire().await.unwrap().forget();
                    self.awake.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(zuse_rpc::RpcReply::Value(
                        json!({"accountId":"owner","organizationId":null,"data":{"commandId":params["request"]["commandId"],"durablyAdmitted":true}}),
                    ))
                }
                methods::CLOUD_RPC => {
                    let request = params["requestId"].clone();
                    let value = match params["method"].as_str().unwrap_or_default() {
                        methods::ENGINE_INFO => {
                            json!({"id":request,"ok":{"deviceId":"cloud:task","workspaceScope":"synced","capabilities":[]}})
                        }
                        methods::WATCH_CHATS => {
                            json!({"id":request,"item":[retained_history(true)["head"]["chat"].clone()]})
                        }
                        methods::WATCH_DOC_MESSAGES => {
                            json!({"id":request,"item":retained_history(true)["head"]["transcript"]})
                        }
                        _ => json!({"id":request,"ok":[]}),
                    };
                    // RpcReply::Stream supplies the controller's outer item.
                    // Its payload is already the complete guest server frame.
                    Ok(pending(value))
                }
                _ => Err(zuse_rpc::RpcError::UnknownMethod(method.into())),
            }
        }
    }
    #[gpui::test]
    fn paused_history_and_retired_transport_send_reopens_passively_without_losing_native_canvas(
        cx: &mut TestAppContext,
    ) {
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        });
        let service = Arc::new(RetiredEnvironmentController {
            awake: AtomicBool::new(false),
            environment_calls: Mutex::new(vec![]),
            requests: Mutex::new(vec![]),
            admit: tokio::sync::Semaphore::new(0),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let old_engine = controller.clone();
        let directory = tempfile::tempdir().unwrap();
        let view=cx.new(|cx|CloudConversation::new(CloudTarget{controller,subject:"owner".into(),organization:None,workspace:"task".into(),initial_session:"session".into()},json!({"workspaceId":"task","agent":"codex","model":"gpt-6.1-sol","runtimeMode":"full-access"}),Value::Null,boot(directory.path()),cx));
        view.update(cx, |view, cx| {
            view.account = None;
            view.authorized = true;
            view.state
                .update(cx, |state, _| state.set_test_engine(old_engine));
            view.apply_history(retained_history(false), cx);
            view.watch_environment(false, cx);
        });
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        let (composer_id, state_id) = view.read_with(cx, |view, _| {
            (view.composer.entity_id(), view.state.entity_id())
        });
        view.update(cx, |view, cx| {
            view.composer.update(cx, |composer, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("Continue retained work", cx));
                composer.send_for_test("Continue retained work".into(), false, cx);
            })
        });
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        {
            let requests = service.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(
                requests[0]["request"]["wake"], "explicit-resume",
                "real foreground Send must wake despite blocked transport"
            );
        }
        view.update(cx, |view, cx| {
            view.composer.update(cx, |composer, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("Next retained draft", cx))
            })
        });
        service.admit.add_permits(1);
        for _ in 0..40 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(view.composer.entity_id(), composer_id);
            assert_eq!(view.state.entity_id(), state_id);
            assert_eq!(
                view.composer.read(cx).input.read(cx).text(),
                "Next retained draft"
            );
            assert!(
                view.state
                    .read(cx)
                    .transcript
                    .iter()
                    .any(|entry| entry.id == "saved-user")
            );
            assert!(
                view.state.read(cx).engine().is_some(),
                "fresh passive connection must attach after retired handle"
            );
            assert_eq!(
                view.state
                    .read(cx)
                    .engine()
                    .unwrap()
                    .engine_info()
                    .device_id,
                "cloud:task",
                "old memory-test engine must be replaced"
            );
        });
        let calls = service.environment_calls.lock().unwrap();
        assert!(
            calls.len() >= 2,
            "durable foreground admission must reopen the retired environment"
        );
        assert!(
            calls.iter().all(|p| p["interactiveResume"] == false),
            "attachment must not issue a second Resume"
        );
        assert_eq!(
            service.requests.lock().unwrap().len(),
            1,
            "one durable Run only"
        );
        drop(calls);
        view.update(cx, |view, cx| view.revoke(cx));
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
    }
    #[gpui::test]
    fn admitted_cloud_activity_is_visible_before_response_and_clears_on_terminal_history(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"allocating"}),
                json!({"firstMessage":"Build this"}),
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx,|view,cx| {
            let composer=view.composer.entity_id();let state=view.state.entity_id();
            assert_eq!(view.activity(cx).unwrap().label,"Starting computer");
            view.computer.metadata=json!({"state":"ready","preparation":{"repository":{"state":"completed"},"environment":{"state":"completed"}}});
            assert!(view.activity(cx).unwrap().busy,"initial launch before history waits visibly");
            let entry=|id:&str,role,text:&str|zuse_model::SessionMessageEntry{id:id.into(),role,parts:vec![zuse_model::MessagePart::Text{id:format!("text:{id}"),text:text.into()}],created_at:0,device_id:String::new(),status:None,continuation_of:None,duration_ms:None};
            view.state.update(cx,|state,_|state.transcript=vec![entry("first",zuse_model::MessageRole::User,"Build this"),entry("answer",zuse_model::MessageRole::Assistant,"")]);
            assert!(view.activity(cx).unwrap().busy,"empty first placeholder is not a response");
            view.state.update(cx,|state,_|state.transcript[1]=entry("answer",zuse_model::MessageRole::Assistant,"I am working"));
            assert!(view.first_message.is_some(),"test covers pre-history launch fallback");
            assert!(view.activity(cx).is_none(),"visible exact launch response clears initial waiting fallback");
            view.admissions=json!([{"commandId":"launch:task","messageId":"first","text":"Build this","executionState":"running"}]);
            assert_eq!(view.activity(cx).unwrap().label,"Agent working…","actual running tools remain visible after response");
            view.state.update(cx,|state,_|state.transcript.clear());
            view.first_message=None;
            for execution in ["queued","starting","running"] {
                view.admissions=json!([{"commandId":"launch:task","messageId":"first","text":"Build this","executionState":execution}]);
                assert!(view.activity(cx).unwrap().busy,"admitted-before-response must not look idle");
            }
            view.admissions=json!([{"commandId":"queued-control","executionState":"running","messageId":null}]);
            assert!(view.activity(cx).is_none(),"queue/control delivery is not inference");
            for terminal in ["completed","failed","cancelled"] {
                view.admissions=json!([{"commandId":"launch:task","messageId":"first","text":"Build this","executionState":terminal}]);
                assert!(view.activity(cx).is_none(),"terminal receipt clears loader");
            }
            assert_eq!(view.composer.entity_id(),composer);assert_eq!(view.state.entity_id(),state,"activity never replaces desktop/controller state");
            view.authorized=true;
            view.admissions=json!([{"commandId":"launch:task","messageId":"first","text":"Build this","executionState":"running"}]);
            assert!(view.activity(cx).unwrap().busy);
            view.revoke(cx);
            assert!(view.admissions.is_null(),"revocation clears admission presentation from old account");
            assert!(view.activity(cx).is_none(),"revocation is terminal, not a new computer starting");
            assert!(view.error.as_deref().unwrap().contains("Cloud access changed"));

        });
    }
    struct OlderHistoryController {
        calls: std::sync::Mutex<Vec<Value>>,
        page: Value,
        release: tokio::sync::Semaphore,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for OlderHistoryController {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            if method == methods::CLOUD_HISTORY {
                self.calls.lock().unwrap().push(params);
                self.release.acquire().await.unwrap().forget();
                return Ok(zuse_rpc::RpcReply::Value(self.page.clone()));
            }
            assert_eq!(
                method,
                methods::CLOUD_ACCOUNT_STATUS,
                "paging must never allocate/wake/execute"
            );
            Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                futures::stream::pending(),
            )))
        }
    }
    fn saved_page(ids: &[&str], before: Option<i64>) -> Value {
        let mut value = retained_history(false);
        value["head"]["transcript"]["reset"]=json!(ids.iter().map(|id|json!({"id":id,"role":"assistant","parts":[{"id":format!("text:{id}"),"kind":"text","text":id}],"createdAt":1,"deviceId":"cloud:task"})).collect::<Vec<_>>());
        value["head"]["olderMessageSequence"] = json!(before);
        value
    }
    struct NativeOlderButton(Entity<CloudConversation>);
    impl Render for NativeOlderButton {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(600.))
                .h(px(200.))
                .children(self.0.update(cx, |view, cx| view.render_older_history(cx)))
        }
    }
    #[gpui::test]
    fn native_older_button_reads_one_scoped_page_without_wake_and_keeps_current_view(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(initialize);
        let service = std::sync::Arc::new(OlderHistoryController {
            calls: Default::default(),
            page: saved_page(&["one", "two"], None),
            release: tokio::sync::Semaphore::new(0),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let dir = tempfile::tempdir().unwrap();
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"paused","desiredState":"paused"}),
                Value::Null,
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.account = None;
            view.authorized = true;
            view.apply_history(saved_page(&["tail"], Some(2)), cx);
        });
        let transcript = view.read_with(cx, |view, _| view.transcript.entity_id());
        let window = cx.add_window(|_, _| NativeOlderButton(view.clone()));
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            let bounds = visual
                .debug_bounds("native-cloud-history-older")
                .expect("real native older button is painted");
            visual.simulate_click(bounds.center(), gpui::Modifiers::default());
        }
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        let calls = service.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["workspaceId"], "task");
        assert_eq!(calls[0]["sessionId"], "session");
        assert_eq!(calls[0]["beforeSequence"], 2);
        assert_eq!(calls[0]["cursor"]["epoch"], "fixture");
        assert_eq!(calls[0]["expectedAccountId"], "owner");
        assert!(calls[0]["interactiveResume"].is_null());
        assert!(calls[0]["wake"].is_null());
        view.update(cx, |view, cx| view.load_older(cx));
        assert_eq!(
            service.calls.lock().unwrap().len(),
            1,
            "inflight page is not fetched twice"
        );
        service.release.add_permits(1);
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        view.update(cx, |view, cx| {
            assert_eq!(view.transcript.entity_id(), transcript);
            assert_eq!(
                view.state
                    .read(cx)
                    .transcript
                    .iter()
                    .map(|row| row.id.as_str())
                    .collect::<Vec<_>>(),
                vec!["one", "two", "tail"]
            );
            assert!(
                view.history
                    .head
                    .as_ref()
                    .unwrap()
                    .older_message_sequence
                    .is_none()
            );
            assert!(view.render_older_history(cx).is_none());
            let mut latest = saved_page(&["tail", "new"], Some(2));
            latest["head"]["cursor"]["version"] = json!(2);
            view.apply_history(latest, cx);
            assert_eq!(
                view.state
                    .read(cx)
                    .transcript
                    .iter()
                    .map(|row| row.id.as_str())
                    .collect::<Vec<_>>(),
                vec!["one", "two", "tail", "new"]
            );
            view.load_older(cx);
        });
        assert_eq!(
            service.calls.lock().unwrap().len(),
            1,
            "loaded range is retained and never refetched"
        );
        cx.update_window(window.into(), |_, window, cx| window.remove_window())
            .unwrap();
    }
    #[gpui::test]
    fn native_history_revoke_cancels_pending_page_and_full_live_replay_survives_detach(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(initialize);
        let service = std::sync::Arc::new(OlderHistoryController {
            calls: Default::default(),
            page: saved_page(&["one", "two"], None),
            release: tokio::sync::Semaphore::new(0),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let engine = controller.clone();
        let dir = tempfile::tempdir().unwrap();
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"paused","desiredState":"paused"}),
                Value::Null,
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.account = None;
            view.authorized = true;
            view.apply_history(saved_page(&["tail"], Some(2)), cx);
            view.load_older(cx);
        });
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        assert_eq!(service.calls.lock().unwrap().len(), 1);
        view.update(cx, |view, cx| view.revoke(cx));
        service.release.add_permits(1);
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        view.read_with(cx, |view, cx| {
            assert!(view.history.head.is_none());
            assert!(view.state.read(cx).transcript.is_empty());
            assert!(!view.authorized);
        });
        view.update(cx, |view, cx| {
            view.access_revoked = false;
            view.authorized = true;
            view.apply_history(saved_page(&["tail"], Some(2)), cx);
            view.history
                .start_live(view.state.read(cx).transcript_revision);
            view.state.update(cx, |state, cx| {
                state.set_test_engine(engine);
                let full = serde_json::from_value::<zuse_model::cloud::Head>(
                    saved_page(&["full-old", "live-new"], None)["head"].clone(),
                )
                .unwrap();
                state
                    .receive_transcript_update(full.transcript, cx)
                    .unwrap();
            });
            view.history.observe_live(view.state.read(cx));
            let mut active = saved_page(&["tail"], Some(2));
            active["workspace"]["state"] = json!("ready");
            active["workspace"]["desiredState"] = json!("ready");
            view.apply_history(active, cx);
            assert!(
                view.state.read(cx).engine().is_some(),
                "fixture keeps live execution attached"
            );
            assert_eq!(
                view.state.read(cx).transcript[0].id,
                "full-old",
                "API never overwrites attached watch baseline"
            );
            view.detach_execution(cx);
            view.apply_history(saved_page(&["tail"], Some(2)), cx);
            assert_eq!(
                view.state.read(cx).transcript[0].id,
                "full-old",
                "full live history survives passive detach/latest refresh"
            );
            assert!(view.render_older_history(cx).is_none());
            let mut newer = saved_page(&["live-new", "new-api-tail"], Some(1));
            newer["head"]["cursor"]["version"] = json!(2);
            newer["head"]["transcript"]["reset"][0]["parts"][0]["text"] = json!("Authoritative revised reply");
            view.apply_history(newer, cx);
            assert!(view.state.read(cx).engine().is_none());
            assert_eq!(view.state.read(cx).transcript.iter().map(|row|row.id.as_str()).collect::<Vec<_>>(),vec!["full-old","live-new","new-api-tail"],"newer authoritative API tail updates retained full history after detach");
            assert!(matches!(&view.state.read(cx).transcript[1].parts[0], zuse_model::MessagePart::Text{text,..} if text=="Authoritative revised reply"));
            let mut reset = saved_page(&["new-generation"], Some(2));
            reset["head"]["runtimeGeneration"] = json!(2);
            reset["head"]["cursor"]["epoch"] = json!("new-epoch");
            view.apply_history(reset, cx);
            assert_eq!(
                view.state.read(cx).transcript[0].id,
                "new-generation",
                "generation reset cannot retain previous projection rows"
            );
        });
    }
    #[gpui::test]
    fn native_active_history_keeps_loaded_prefix_when_latest_account_event_revalidates(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"paused","desiredState":"paused"}),
                Value::Null,
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx,|view,cx| {
            let mut initial = retained_history(false);
            let row=|id:&str|json!({"id":id,"role":"assistant","parts":[{"id":format!("text:{id}"),"kind":"text","text":id}],"createdAt":1,"deviceId":"cloud:task"});
            initial["head"]["transcript"]["reset"]=json!([row("older"),row("tail")]);
            initial["head"]["olderMessageSequence"]=json!(50);
            view.apply_history(initial.clone(),cx);
            let mut latest=initial;
            latest["head"]["cursor"]["version"]=json!(2);
            latest["head"]["transcript"]["reset"]=json!([row("tail"),row("new")]);
            latest["head"]["olderMessageSequence"]=json!(51);
            view.apply_history(latest,cx);
            assert_eq!(view.state.read(cx).transcript.iter().map(|entry|entry.id.as_str()).collect::<Vec<_>>(),vec!["older","tail","new"],"latest account history page must not discard the already loaded native prefix");
        });
    }
    #[gpui::test]
    fn cloud_tab_title_survives_placeholder_clear_and_uses_saved_history_title(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","title":"Stored title"}),
                json!({"firstMessage":"Prompt title"}),
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx,|view,cx| {
            assert_eq!(view.title,"Stored title");
            let mut value=json!({"head":{"schemaVersion":1,"payloadFormat":"fixture","workspaceId":"task","sessionId":"session","runtimeGeneration":1,"cursor":{"epoch":"fixture","version":1},"chat":{"id":"session","deviceId":"fixture","archived":false,"createdAt":"2026-10-31T00:00:00Z","title":"Canonical title"},"conversations":[],"transcript":{"reset":[]},"queue":[],"queuePaused":false,"olderMessageSequence":null}});
            let _:zuse_model::cloud::Head=serde_json::from_value(value["head"].clone()).expect("real typed history fixture");view.apply_history(value.clone(),cx);assert!(view.first_message.is_none());assert_eq!(view.title,"Canonical title");value["head"]["chat"]["title"]=Value::Null;view.apply_history(value,cx);assert_eq!(view.title,"Canonical title","an empty later history title cannot erase the stable label");
        });
    }
    #[gpui::test]
    fn cloud_only_new_chat_reuses_native_shell_composer_and_window(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                let valid=account(&mut state);
                let unavailable=json!({"projectId":"unavailable","accountId":"owner","canCreate":false,"repositoryIdentity":"github.com/owner/unavailable"});
                state.cloud_project=Some(unavailable.clone());
                assert!(state.apply_cloud_sidebar_catalog(
                    &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[unavailable,valid]}),
                    &json!({"accountId":"owner","organizationId":null,"workspaces":[]})));
                state
            });
            Shell::new(state, boot(dir.path()), cx)
        });
        let windows = cx.update(|cx| cx.windows().len());
        window
            .update(cx, |shell, _, cx| {
                let controller = shell.state.entity_id();
                let composer = shell.composer.entity_id();
                let transcript = shell.transcript.entity_id();
                shell.open_new_session(None, cx);
                assert_eq!(shell.state.entity_id(), controller);
                assert_eq!(shell.composer.entity_id(), composer);
                assert_eq!(shell.transcript.entity_id(), transcript);
                assert!(matches!(shell.route, Route::Chat));
                assert!(shell.state.read(cx).selected_chat.is_none());
                assert_eq!(
                    shell.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],
                    "project"
                );
                assert_eq!(shell.state.read(cx).cloud_sidebar_projects().len(), 2);
                assert!(shell.cloud_conversation.is_none());
            })
            .unwrap();
        assert_eq!(
            cx.update(|cx| cx.windows().len()),
            windows,
            "New Chat must not create another window"
        );
    }
    #[gpui::test]
    fn admitted_cloud_task_selects_current_native_view_before_execution_ready(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                account(&mut state);
                state
            });
            state.update(cx, |state, _| state.set_test_engine(controller.clone()));
            Shell::new(state, boot(dir.path()), cx)
        });
        let windows = cx.update(|cx| cx.windows().len());
        let (controller_state,sidebar,composer)=window.update(cx,|shell,_,cx| {
            let controller_state=shell.state.entity_id();let sidebar=shell.sidebar_pane.entity_id();let composer=shell.composer.entity_id();
            // Exercise the production composer event subscription, not a root replacement.
            shell.composer.update(cx,|_,cx|cx.emit(ComposerEvent::CloudAdmitted {
                workspace:json!({"workspaceId":"task","initialSessionId":"session","commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2}),
                account:"owner".into(),organization:None,
                launch:json!({"firstMessage":"One admitted message","agent":"codex","model":"fixture"}),
            }));
            (controller_state,sidebar,composer)
        }).unwrap();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.state.entity_id(), controller_state);
                assert_eq!(shell.sidebar_pane.entity_id(), sidebar);
                assert_eq!(shell.composer.entity_id(), composer);
                let conversation = shell
                    .cloud_conversation
                    .as_ref()
                    .expect("admission selects current conversation");
                let task = conversation.read(cx);
                assert_eq!(task.first_message.as_deref(), Some("One admitted message"));
                assert_eq!(
                    task.state.read(cx).selected_chat.as_deref(),
                    Some("session")
                );
                assert!(task.state.read(cx).engine().is_none());
                assert!(!task.ready(cx));
                assert_ne!(shell.conversation_state(cx).entity_id(), controller_state);
                shell.add_terminal_surface(cx);
                assert!(
                    shell.right_terminal.is_none(),
                    "preparing cloud tools must never fall back to local execution"
                );
                assert!(
                    shell
                        .cloud_conversation
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .terminal
                        .is_none()
                );
            })
            .unwrap();
        assert_eq!(
            cx.update(|cx| cx.windows().len()),
            windows,
            "Send must not create another window"
        );
    }
    /// Paint the same chrome gate, header, tab row and right-pane host as
    /// Shell::render without starting a live account or guest connection.
    struct CloudChromeHost {
        shell: Entity<Shell>,
        _directory: tempfile::TempDir,
    }
    impl Render for CloudChromeHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.shell.update(cx, |shell, cx| {
                let header = shell
                    .main_chrome_visible(cx)
                    .then(|| shell.render_title_bar(cx));
                let tabs = shell
                    .conversation_tab_row_visible(cx)
                    .then(|| shell.render_conversation_tab_row(cx));
                div()
                    .relative()
                    .size_full()
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .child(div().flex_1())
                            .child(shell.render_right_pane(window, cx)),
                    )
                    .children(
                        header.map(|header| {
                            div().absolute().top_0().left_0().right_0().child(header)
                        }),
                    )
                    .children(tabs)
                    .child(shell.workspace_sidebar_header(cx))
            })
        }
    }
    #[gpui::test]
    fn cloud_only_chat_paints_header_logo_and_opens_right_sidebar(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            initialize(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    account(&mut state);
                    state
                });
                let mut shell = Shell::new(state, boot(directory.path()), cx);
                shell.reduced_motion = true;
                shell.viewport_width = 1400.;
                shell.viewport_height = 960.;
                shell.settings.sidebar_collapsed = false;
                assert!(
                    !shell.main_chrome_visible(cx),
                    "blank local canvas keeps its existing chrome policy"
                );
                shell.open_cloud_conversation(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task".into(),
                        initial_session: "session".into(),
                    },
                    json!({"workspaceId":"task","initialSessionId":"session"}),
                    Value::Null,
                    cx,
                );
                let view = shell.cloud_conversation.as_ref().unwrap().clone();
                view.update(cx, |view, cx| {
                    let mut history = retained_history(false);
                    history["head"]["chat"]["sourceContext"] = json!({
                        "checkoutId":"task","repoRoot":"/workspace/repo","cwd":"/workspace/repo",
                        "branch":"task/cloud-chrome","observedAt":"2026-11-01T00:00:00Z"
                    });
                    view.apply_history(history, cx);
                    view.account = None;
                });
                assert!(shell.state.read(cx).selected_chat.is_none());
                assert!(shell.main_chrome_visible(cx));
                assert!(shell.conversation_tab_row_visible(cx));
                shell
            });
            CloudChromeHost {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("workspace-app-logo").is_some());
        assert!(cx.debug_bounds("top-bar-branch").is_some());
        assert!(cx.debug_bounds("native-cloud-computer").is_some());
        assert!(cx.debug_bounds("conversation-tab-row").is_some());
        assert!(cx.debug_bounds("cloud-new-tab").is_some());
        let toggle = cx
            .debug_bounds("toggle-changes")
            .expect("cloud chat must expose its right sidebar toggle");
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(shell.read_with(cx, |shell, cx| shell.right_pane_open(cx)));
        assert!(cx.debug_bounds("surface-card-files").is_some());
        assert!(cx.debug_bounds("surface-card-terminal").is_some());
        let toggle = cx.debug_bounds("toggle-changes").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        assert!(!shell.read_with(cx, |shell, cx| shell.right_pane_open(cx)));
        let view = shell.read_with(cx, |shell, _| {
            shell.cloud_conversation.as_ref().unwrap().clone()
        });
        view.update(cx, |view, cx| {
            view.config["branch"] = json!("task/private-saved-branch");
            assert!(view.branch_label().is_some());
            view.revoke(cx);
            assert!(
                view.branch_label().is_none(),
                "revocation must clear the fallback header label too"
            );
        });
    }

    struct ConversationHost {
        shell: Entity<Shell>,
        _directory: tempfile::TempDir,
    }
    impl Render for ConversationHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.shell.update(cx, |shell, cx| {
                div()
                    .size_full()
                    .flex()
                    .child(shell.render_main(window, 900., 900., cx))
            })
        }
    }
    #[gpui::test]
    fn closing_cloud_tools_returns_focus_and_typing_to_displayed_native_composer(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            initialize(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    account(&mut state);
                    state
                });
                let mut shell = Shell::new(state, boot(directory.path()), cx);
                shell.open_cloud_conversation(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task".into(),
                        initial_session: "session".into(),
                    },
                    json!({"workspaceId":"task","initialSessionId":"session"}),
                    json!({"firstMessage":"Accepted task","agent":"codex","model":"fixture"}),
                    cx,
                );
                // A focus-only state fixture, not authentication or delivery evidence.
                // Every transport remains in-process and pending; no command can execute.
                shell
                    .cloud_conversation
                    .as_ref()
                    .unwrap()
                    .update(cx, |conversation, cx| {
                        conversation.authorized = true;
                        let pickers = conversation.composer.read(cx).pickers().clone();
                        pickers.update(cx, |pickers, cx| {
                            pickers.seed_cloud_task_catalog_for_test(cx)
                        });
                    });
                shell
            });
            ConversationHost {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                window.focus(&shell.navigation_focus.right, cx);
                shell.restore_right_focus_after_close(window, cx);
                assert!(
                    shell
                        .conversation_composer(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                );
                assert!(!shell.composer.focus_handle(cx).is_focused(window));
            })
        });
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_input("Continue this cloud task");
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell
                    .conversation_composer(cx)
                    .read(cx)
                    .input
                    .read(cx)
                    .text(),
                "Continue this cloud task"
            );
            assert_eq!(shell.composer.read(cx).input.read(cx).text(), "");
        });
    }
    struct ProvisionalHistoryController {
        fresh_failure: Option<&'static str>,
        events: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Value>>>,
        all_requests: std::sync::Mutex<Vec<(String, Value)>>,
        cache: tokio::sync::Semaphore,
        fresh: tokio::sync::Semaphore,
        requests: std::sync::Mutex<Vec<Value>>,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for ProvisionalHistoryController {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            use futures::StreamExt as _;
            self.all_requests
                .lock()
                .unwrap()
                .push((method.into(), params.clone()));
            match method {
                methods::CLOUD_ACCOUNT_STATUS => Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::once(async { json!({"user":{"id":"owner"},"orgId":null}) })
                        .chain(futures::stream::pending()),
                ))),
                methods::CLOUD_HISTORY => {
                    self.requests.lock().unwrap().push(params.clone());
                    if params["beforeSequence"].as_i64().is_some() {
                        return Err(match self.fresh_failure {
                            Some("older-permission-denied") => zuse_rpc::RpcError::Failed(
                                "You do not have access to this workspace.".into(),
                            ),
                            Some("older-transport-error") => zuse_rpc::RpcError::Transport(
                                "synthetic older-page network unavailable".into(),
                            ),
                            _ => zuse_rpc::RpcError::Failed("unexpected older request".into()),
                        });
                    }
                    let cached = params["cacheOnly"] == true;
                    if cached {
                        self.cache.acquire().await.unwrap().forget();
                    } else {
                        self.fresh.acquire().await.unwrap().forget();
                    }
                    if !cached {
                        match self.fresh_failure {
                            Some("permission-denied") => {
                                return Err(zuse_rpc::RpcError::Failed(
                                    "You do not have access to this workspace.".into(),
                                ));
                            }
                            Some("session-denied") => return Err(zuse_rpc::RpcError::Failed("Cloud could not verify your session. Reconnect in Settings → General.".into())),
                            Some("workspace-missing") => return Err(zuse_rpc::RpcError::Capability("workspace_not_found".into())),
                            Some("capability-denied") => {
                                return Err(zuse_rpc::RpcError::Capability(
                                    "workspace_access_denied".into(),
                                ));
                            }
                            Some("transport-error") => {
                                return Err(zuse_rpc::RpcError::Transport(
                                    "synthetic network unavailable".into(),
                                ));
                            }
                            _ => {}
                        }
                    }
                    let mut value = retained_history(false);
                    if matches!(
                        self.fresh_failure,
                        Some("older-permission-denied" | "older-transport-error")
                    ) {
                        value["head"]["olderMessageSequence"] = json!(1);
                    }
                    value["head"]["cursor"]["version"] = json!(if cached { 99 } else { 2 });
                    value["head"]["transcript"]["reset"][0]["id"] =
                        json!(if cached { "cached-user" } else { "fresh-user" });
                    if cached {
                        value["provisional"] = json!(true);
                        value["workspace"] = json!({"state":"ready"});
                        value["commandAdmissions"] = json!([{"id":"cached-admission"}]);
                        value["head"]["chat"]["config"] = json!({"harness":"grok","model":"grok-4.7","sandbox":"danger-full-access","modelOptions":{}});
                    }
                    Ok(zuse_rpc::RpcReply::Value(value))
                }
                methods::CLOUD_EVENTS => {
                    let receiver = self.events.lock().unwrap().take().unwrap();
                    Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                        futures::stream::unfold(receiver, |mut receiver| async {
                            receiver.recv().await.map(|value| (value, receiver))
                        }),
                    )))
                }
                methods::CLOUD_ENVIRONMENT => Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::pending(),
                ))),
                _ => Err(zuse_rpc::RpcError::UnknownMethod(method.into())),
            }
        }
    }
    fn provisional_cache_fixture(cx: &mut TestAppContext, scenario: &str) {
        use std::sync::{Arc, Mutex};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        });
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let service = Arc::new(ProvisionalHistoryController {
            events: Mutex::new(Some(events_rx)),
            all_requests: Mutex::new(vec![]),
            fresh_failure: match scenario {
                "permission-denied" => Some("permission-denied"),
                "session-denied" => Some("session-denied"),
                "capability-denied" => Some("capability-denied"),
                "workspace-missing" => Some("workspace-missing"),
                "transport-error" => Some("transport-error"),
                "older-permission-denied" => Some("older-permission-denied"),
                "older-transport-error" => Some("older-transport-error"),
                _ => None,
            },
            cache: tokio::sync::Semaphore::new(0),
            fresh: tokio::sync::Semaphore::new(0),
            requests: Mutex::new(vec![]),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let directory = tempfile::tempdir().unwrap();
        let view=cx.new(|cx|CloudConversation::new(CloudTarget{controller,subject:"owner".into(),organization:None,workspace:"task".into(),initial_session:"session".into()},json!({"workspaceId":"task","agent":"codex","model":"gpt-6.1-sol","state":"paused"}),Value::Null,boot(directory.path()),cx));
        view.update(cx, |view, cx| {
            view.composer.update(cx, |composer, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("Retained draft", cx))
            })
        });
        let pump = |cx: &mut TestAppContext| {
            for _ in 0..30 {
                cx.run_until_parked();
                runtime.block_on(async {
                    tokio::task::yield_now().await;
                });
            }
        };
        pump(cx);
        let requests = service.requests.lock().unwrap().clone();
        assert_eq!(
            requests.len(),
            2,
            "account authorization must race cache with exactly one fresh read"
        );
        assert_eq!(
            requests.iter().filter(|p| p["cacheOnly"] == true).count(),
            1
        );
        assert!(requests.iter().all(|p| p["workspaceId"] == "task"));
        if scenario.starts_with("events-") {
            service.cache.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                assert!(
                    view.state
                        .read(cx)
                        .transcript
                        .iter()
                        .any(|row| row.id == "cached-user")
                )
            });
            let identity = view.read_with(cx, |view, _| {
                (
                    view.state.entity_id(),
                    view.composer.entity_id(),
                    view.transcript.entity_id(),
                )
            });
            if scenario == "events-reset" {
                service.fresh.add_permits(1);
                pump(cx);
                for reason in [
                    None,
                    Some(Value::Null),
                    Some(json!("cursor_expired")),
                    Some(json!("consumer_lagged")),
                ] {
                    let mut data = json!({"type":"reset","cacheRevoked":true});
                    if let Some(reason) = reason {
                        data["reason"] = reason;
                    }
                    events_tx
                        .send(json!({"accountId":"owner","organizationId":null,"data":data}))
                        .unwrap();
                    service.fresh.add_permits(1);
                    pump(cx);
                }
                events_tx.send(json!({"accountId":"another-owner","organizationId":null,"data":{"type":"closed","cacheRevoked":true}})).unwrap();
                pump(cx);
                assert_eq!(
                    service.requests.lock().unwrap().len(),
                    6,
                    "only one fresh revalidation per known reset, no stale-scope refresh"
                );
                view.read_with(cx, |view, cx| {
                    assert!(view.authorized && !view.access_revoked);
                    assert_eq!(
                        (
                            view.state.entity_id(),
                            view.composer.entity_id(),
                            view.transcript.entity_id()
                        ),
                        identity
                    );
                    assert!(
                        view.state
                            .read(cx)
                            .transcript
                            .iter()
                            .any(|row| row.id == "fresh-user")
                    );
                    assert_eq!(
                        view.composer.read(cx).input.read(cx).text(),
                        "Retained draft"
                    );
                });
                let calls = service.all_requests.lock().unwrap();
                assert!(calls.iter().all(|(method, _)| matches!(
                    method.as_str(),
                    methods::CLOUD_ACCOUNT_STATUS
                        | methods::CLOUD_EVENTS
                        | methods::CLOUD_HISTORY
                        | methods::CLOUD_ENVIRONMENT
                )));
                let environments = calls
                    .iter()
                    .filter(|(method, _)| method == methods::CLOUD_ENVIRONMENT)
                    .collect::<Vec<_>>();
                assert_eq!(
                    environments.len(),
                    1,
                    "reset must not reopen or wake the execution connection"
                );
                assert_eq!(environments[0].1["interactiveResume"], false);
            } else {
                let data = match scenario {
                    "events-target-revoked" => {
                        json!({"type":"reset","reason":"cursor_expired","cacheRevoked":true,"revokedWorkspaceIds":["task"]})
                    }
                    "events-error" => {
                        json!({"type":"error","code":"cloud_event_access_denied","cacheRevoked":true})
                    }
                    "events-closed" => json!({"type":"closed","cacheRevoked":true}),
                    _ => panic!("unknown event fixture"),
                };
                events_tx
                    .send(json!({"accountId":"owner","organizationId":null,"data":data}))
                    .unwrap();
                pump(cx);
                view.read_with(cx, |view, cx| {
                    assert!(view.access_revoked);
                    assert!(view.state.read(cx).transcript.is_empty());
                    assert!(view.history.head.is_none());
                });
            }
            return;
        }
        if matches!(
            scenario,
            "older-permission-denied" | "older-transport-error"
        ) {
            service.cache.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                assert!(
                    view.state
                        .read(cx)
                        .transcript
                        .iter()
                        .any(|row| row.id == "cached-user")
                )
            });
            service.fresh.add_permits(1);
            pump(cx);
            view.update(cx, |view, cx| view.load_older(cx));
            pump(cx);
            let requests = service.requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            assert_eq!(
                requests.iter().filter(|p| p["beforeSequence"] == 1).count(),
                1
            );
            view.read_with(cx, |view, cx| {
                if scenario == "older-permission-denied" {
                    assert!(
                        view.access_revoked,
                        "actual older-page 403 must erase saved rows"
                    );
                    assert!(view.state.read(cx).transcript.is_empty());
                    assert!(view.history.head.is_none());
                } else {
                    assert!(!view.access_revoked);
                    assert!(
                        view.state
                            .read(cx)
                            .transcript
                            .iter()
                            .any(|row| row.id == "fresh-user")
                    );
                    assert!(view.history.head.is_some());
                    assert!(view.error.as_deref().is_some_and(|error| {
                        error.contains("Earlier history could not be loaded")
                    }));
                }
            });
            return;
        }
        if matches!(
            scenario,
            "permission-denied"
                | "session-denied"
                | "capability-denied"
                | "workspace-missing"
                | "transport-error"
        ) {
            service.cache.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                assert!(
                    view.state
                        .read(cx)
                        .transcript
                        .iter()
                        .any(|row| row.id == "cached-user")
                )
            });
            service.fresh.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                if scenario == "transport-error" {
                    assert!(!view.access_revoked);
                    assert!(
                        view.state
                            .read(cx)
                            .transcript
                            .iter()
                            .any(|row| row.id == "cached-user")
                    );
                    assert!(
                        view.error
                            .as_deref()
                            .is_some_and(|error| error.contains("network unavailable"))
                    );
                } else {
                    assert!(
                        view.access_revoked,
                        "fresh permission denial must revoke already painted saved rows"
                    );
                    assert!(view.state.read(cx).transcript.is_empty());
                    assert!(view.history.head.is_none());
                    assert!(view.history.provisional_read.is_none());
                    if scenario == "workspace-missing" {
                        assert_eq!(view.error.as_deref(), Some("This conversation is no longer available. Choose another conversation or start a new chat."));
                    }
                    assert!(service.all_requests.lock().unwrap().iter().all(|(method, _)| matches!(method.as_str(), methods::CLOUD_ACCOUNT_STATUS | methods::CLOUD_HISTORY | methods::CLOUD_EVENTS | methods::CLOUD_ENVIRONMENT)), "revocation must not admit lifecycle or inference commands");
                }
            });
            return;
        }
        if scenario == "held-fresh" {
            service.cache.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                assert!(
                    view.state
                        .read(cx)
                        .transcript
                        .iter()
                        .any(|row| row.id == "cached-user"),
                    "cached head should paint while fresh API is held"
                );
                assert!(view.state.read(cx).transcript_baseline("session").is_none());
                assert!(view.admissions.is_null());
                assert_eq!(view.config["model"], "gpt-6.1-sol");
                assert_eq!(view.computer.metadata["state"], "paused");
                assert!(view.state.read(cx).engine().is_none());
                assert_eq!(
                    view.composer.read(cx).input.read(cx).text(),
                    "Retained draft"
                );
            });
            service.fresh.add_permits(1);
            pump(cx);
        } else if scenario == "late-cache" {
            service.fresh.add_permits(1);
            pump(cx);
            service.cache.add_permits(1);
            pump(cx);
        } else {
            view.update(cx, |view, cx| view.revoke(cx));
            service.cache.add_permits(1);
            service.fresh.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                assert!(view.access_revoked);
                assert!(view.state.read(cx).transcript.is_empty());
                assert!(view.history.head.is_none());
            });
            return;
        }
        view.read_with(cx, |view, cx| {
            let rows = &view.state.read(cx).transcript;
            assert!(rows.iter().any(|row| row.id == "fresh-user"));
            assert!(
                !rows.iter().any(|row| row.id == "cached-user"),
                "late cache must never replace fresh receipt"
            );
            assert_eq!(view.config["model"], "gpt-6.1-sol");
            assert_eq!(
                view.composer.read(cx).input.read(cx).text(),
                "Retained draft"
            );
        });
    }
    #[gpui::test]
    fn active_native_cached_history_paints_while_fresh_api_is_held(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "held-fresh");
    }
    #[gpui::test]
    fn active_native_late_cache_cannot_replace_fresh_history_or_selection(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "late-cache");
    }
    #[gpui::test]
    fn active_native_revocation_cancels_pending_provisional_history(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "revoke");
    }
    #[gpui::test]
    fn active_native_fresh_permission_denial_clears_already_painted_cache(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "permission-denied");
    }
    #[gpui::test]
    fn active_native_fresh_capability_denial_clears_already_painted_cache(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "capability-denied");
    }
    #[gpui::test]
    fn active_native_transport_failure_keeps_readable_provisional_cache(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "transport-error");
    }
    #[gpui::test]
    fn active_native_older_permission_denial_clears_saved_history(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "older-permission-denied");
    }
    #[gpui::test]
    fn active_native_older_transport_failure_keeps_saved_history(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "older-transport-error");
    }
    #[gpui::test]
    fn active_native_event_resets_revalidate_in_place_without_wake(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "events-reset");
    }
    #[gpui::test]
    fn active_native_explicit_target_revocation_overrides_cursor_reset(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "events-target-revoked");
    }
    #[gpui::test]
    fn active_native_error_event_clears_cached_history(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "events-error");
    }
    #[gpui::test]
    fn active_native_closed_event_clears_cached_history(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "events-closed");
    }
    #[gpui::test]
    fn active_native_fresh_session_denial_clears_already_painted_cache(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "session-denied");
    }
    #[gpui::test]
    fn active_native_missing_workspace_clears_provisional_history_without_sign_in(
        cx: &mut TestAppContext,
    ) {
        provisional_cache_fixture(cx, "workspace-missing");
    }
}
