//! Native Computer actions use the account API; execution availability is not authority.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Restart,
    Recreate,
    Delete,
    RetryRestart,
    RetryRecreate,
    Continue,
    Discard,
}
impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Restart => "Restart runtime",
            Self::Recreate => "Recreate computer",
            Self::Delete => "Delete computer",
            Self::RetryRestart => "Retry runtime restart",
            Self::RetryRecreate => "Retry computer recreation",
            Self::Continue => "Continue with new commands",
            Self::Discard => "Discard unstarted messages",
        }
    }
    fn detail(self) -> &'static str {
        match self {
            Self::Restart | Self::RetryRestart => {
                "Restart the Zuse runtime, agents and terminals on this computer. Repository files stay on its disk. Interrupted commands will not be replayed automatically."
            }
            Self::Recreate | Self::RetryRecreate => {
                "Verify a recovery backup before replacing this computer. Files and supported environment state are restored; running processes and terminals restart. The task and its messages keep their identities."
            }
            Self::Delete => {
                "Permanently delete this task's computer, retained disk, stored cloud history and recovery data. Save any work you need first. This cannot be undone."
            }
            Self::Continue => {
                "Permit new commands after reviewing interrupted work. Commands with uncertain outcomes are not replayed."
            }
            Self::Discard => {
                "Cancel messages that never started and permit new commands. Commands with uncertain outcomes are not replayed."
            }
        }
    }
    fn method(self) -> &'static str {
        match self {
            Self::Restart | Self::RetryRestart | Self::Continue | Self::Discard => {
                methods::CLOUD_RUNTIME_REPAIR
            }
            Self::Recreate | Self::RetryRecreate => methods::CLOUD_COMPUTER_RECREATION,
            Self::Delete => methods::CLOUD_DELETE,
        }
    }
    fn summary(self) -> &'static str {
        if matches!(self, Self::Recreate | Self::RetryRecreate) {
            "computerRecreation"
        } else {
            "runtimeRepair"
        }
    }
}
#[derive(Clone)]
struct Pending {
    action: Action,
    key: String,
    params: Value,
}
#[derive(Default)]
pub(super) struct State {
    pub open: bool,
    pub(super) menu: popover::Popup<()>,
    pub metadata: Value,
    logs: Value,
    confirm: Option<Action>,
    pending: Option<Pending>,
    refresh: Option<Task<()>>,
    refresh_again: bool,
    error: Option<String>,
}
impl State {
    #[cfg(test)]
    pub(super) fn setup_output_for_test(&self) -> &Value {
        &self.logs
    }
    pub(super) fn observe_metadata(&mut self, row: Value) -> bool {
        if row["revision"]
            .as_i64()
            .zip(self.metadata["revision"].as_i64())
            .is_some_and(|(next, current)| next < current)
        {
            return false;
        }
        let changed = ["state", "statusCode", "preparation", "startupPhase"]
            .iter()
            .any(|key| self.metadata[*key] != row[*key]);
        self.metadata = row;
        self.open && changed
    }
    pub(super) fn execution_controls_enabled(&self) -> bool {
        self.pending.is_none()
            && !active(&self.metadata)
            && !matches!(
                self.metadata["state"].as_str(),
                Some("deleting" | "deleted" | "archiving" | "archived")
            )
    }
    pub(super) fn composer_enabled(&self) -> bool {
        !matches!(
            self.metadata["state"].as_str(),
            Some("deleting" | "deleted")
        ) && !self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.action == Action::Delete)
    }
}
fn preparation_steps(row: &Value) -> Vec<zuse_viewport::CloudPreparationStep> {
    zuse_viewport::cloud_preparation_progress(row, false)
        .steps
        .into_iter()
        .filter(|step| {
            !(row["state"] == "ready"
                && step.state == zuse_viewport::CloudPreparationStepState::Pending
                && row["preparation"][step.key]["state"].is_null())
        })
        .collect()
}
fn editable(row: &Value) -> bool {
    row["taskCommandCanEdit"] == true
        || row["runtimeRepairCanEdit"] == true
        || row["computerRecreationCanEdit"] == true
}
fn active(row: &Value) -> bool {
    ["runtimeRepair", "computerRecreation"].iter().any(|key| {
        row[*key]["operationId"].is_string()
            && row[*key]["state"]
                .as_str()
                .is_some_and(|state| state != "ready")
    })
}
fn allowed(row: &Value, action: Action) -> bool {
    if !editable(row)
        || matches!(
            row["state"].as_str(),
            Some("deleting" | "deleted" | "archiving" | "archived")
        )
    {
        return false;
    }
    match action {
        Action::Restart => {
            !active(row)
                && row["runtimeRepairCanEdit"] == true
                && row["runtimeRepairAvailable"] == true
        }
        Action::Recreate => {
            !active(row)
                && row["computerRecreationCanEdit"] == true
                && row["computerRecreationAvailable"] == true
        }
        Action::Delete => true,
        Action::RetryRestart => {
            let operation = &row[action.summary()];
            row["runtimeRepairCanEdit"] == true
                && operation["operationId"].is_string()
                && operation["errorCode"].is_string()
                && !matches!(
                    operation["state"].as_str(),
                    Some("ready" | "attention-required")
                )
        }
        Action::RetryRecreate => {
            let operation = &row["computerRecreation"];
            row["computerRecreationCanEdit"] == true
                && operation["operationId"].is_string()
                && operation["errorCode"].is_string()
                && operation["state"] == "attention-required"
        }
        Action::Continue | Action::Discard => {
            row["runtimeRepairCanEdit"] == true
                && row["runtimeRepair"]["state"] == "attention-required"
                && row["runtimeRepair"]["operationId"].is_string()
        }
    }
}
pub(super) fn issue(row: &Value) -> Option<(&'static str, &'static str)> {
    issue_code(row["statusCode"].as_str().unwrap_or_default()).or_else(|| {
        zuse_viewport::cloud_preparation_progress(row,false).steps.into_iter()
            .filter(|step|step.key!="computer" && step.state==zuse_viewport::CloudPreparationStepState::Failed)
            .find_map(|step|issue_code(row["preparation"][step.key]["errorCode"].as_str().unwrap_or_default()).or_else(||match step.key {
                "repository"=>issue_code("repository_preparation_failed"),
                "environment"=>issue_code("environment_setup_failed"),
                "credentials"=>issue_code("credential_preparation_failed"),
                "agent"=>Some(("Coding agent startup needs attention","Review the startup output and selected connection in Cloud settings. Your saved message will not be resent.")),
                _=>Some(("Computer start needs attention","Review the computer status and compute access in Cloud settings. Your saved message will not be resent.")),
            }))
    }).or_else(|| (row["state"]=="failed").then_some(("Computer start needs attention","Review the computer status and compute access in Cloud settings. Your saved message will not be resent.")))
}
fn issue_code(code: &str) -> Option<(&'static str, &'static str)> {
    match code {
        "repository_preparation_failed"
        | "github_repository_access_denied"
        | "github_reconnect_required" => Some((
            "Repository setup needs attention",
            "Check GitHub access and the selected branch in Cloud settings. Reconnecting alone cannot repair repository permissions. Your message remains saved.",
        )),
        "credential_preparation_failed"
        | "agent_connection_required"
        | "agent_connection_reconnect_required"
        | "agent_connection_refresh_uncertain" => Some((
            "Coding agent connection needs attention",
            "Reconnect the selected coding agent in Cloud settings, then restart its runtime when available. Your message is not sent again.",
        )),
        "environment_setup_failed" => Some((
            "Environment setup failed",
            "Review the setup output below and correct the project's environment commands before restarting. Your task and message remain saved.",
        )),
        "cloud_entitlement_required" | "cloud_subscription_required" => Some((
            "Compute access required",
            "Review compute access in Cloud settings before resuming this computer.",
        )),
        "managed_base_unavailable_for_agent"
        | "staging_managed_base_candidate_expired"
        | "cloud_initial_runtime_bootstrap_fenced" => Some((
            "Computer configuration needs attention",
            "The server's managed environment or staging allowance needs an operator update. Repeating your message cannot fix this.",
        )),
        _ => None,
    }
}
fn request(mut scope: Value, action: Action, key: &str, row: &Value, check: bool) -> Option<Value> {
    if check {
        scope["action"] = json!("status");
        scope["operationId"] = row[action.summary()]["operationId"].clone();
        return scope["operationId"].is_string().then_some(scope);
    }
    match action {
        Action::Delete => scope["commandId"] = json!(key),
        Action::RetryRestart | Action::RetryRecreate => {
            scope["action"] = json!("retry");
            scope["operationId"] = row[action.summary()]["operationId"].clone();
            if !scope["operationId"].is_string() {
                return None;
            }
        }
        Action::Continue | Action::Discard => {
            scope["action"] = json!("resolve");
            scope["operationId"] = row["runtimeRepair"]["operationId"].clone();
            if !scope["operationId"].is_string() {
                return None;
            }
            scope["decision"] = json!(if action == Action::Continue {
                "continue"
            } else {
                "discard"
            });
        }
        _ => {
            scope["action"] = json!("start");
            scope["idempotencyKey"] = json!(key);
        }
    }
    Some(scope)
}
impl CloudConversation {
    pub(super) fn close_computer(&mut self, cx: &mut Context<Self>) {
        self.computer.open = false;
        if self.computer.menu.begin_close() {
            popover::reap_popup(cx, |this: &mut Self| &mut this.computer.menu);
        }
        cx.notify();
    }
    pub(super) fn toggle_computer(&mut self, cx: &mut Context<Self>) {
        if self.computer.open {
            self.close_computer(cx);
        } else {
            self.computer.open = true;
            self.computer.menu.open(());
            self.refresh_computer(cx);
            cx.notify();
        }
    }
    pub(super) fn refresh_computer(&mut self, cx: &mut Context<Self>) {
        if !self.authorized {
            return;
        }
        if self.computer.refresh.is_some() {
            self.computer.refresh_again = true;
            return;
        }
        let target = self.target.clone();
        let mut params = self.params();
        // Deleted history has no setup logs; its final metadata remains useful.
        params["includePreparationLogs"] = json!(!matches!(
            self.computer.metadata["state"].as_str(),
            Some("deleting" | "deleted")
        ));
        self.computer.refresh = Some(cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .call_scoped(methods::CLOUD_WORKSPACE, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.computer.refresh = None;
                if !this.authorized {
                    return;
                }
                match result {
                    Ok(value)
                        if value["accountId"] == target.subject
                            && value["organizationId"].as_str()
                                == target.organization.as_deref()
                            && value["workspace"]["workspaceId"] == target.workspace =>
                    {
                        this.computer.observe_metadata(value["workspace"].clone());
                        this.computer.logs = value["preparationLogs"]["log"].clone();
                    }
                    Ok(_) => this.revoke(cx),
                    Err(error) => this.computer.error = Some(error.to_string()),
                }
                if std::mem::take(&mut this.computer.refresh_again)
                    && this.computer.open
                    && this.authorized
                {
                    this.refresh_computer(cx);
                }
                cx.notify();
            });
        }));
    }
    fn computer_action(&mut self, action: Action, check: bool, cx: &mut Context<Self>) {
        if !self.authorized || self.operation.is_some() {
            return;
        }
        if !check
            && self
                .computer
                .pending
                .as_ref()
                .is_some_and(|pending| pending.action != action)
        {
            return;
        }
        let retry = self
            .computer
            .pending
            .as_ref()
            .is_some_and(|pending| pending.action == action);
        if !check && !retry && !allowed(&self.computer.metadata, action) {
            return;
        }
        let key = if let Some(pending) = self
            .computer
            .pending
            .as_ref()
            .filter(|p| p.action == action)
        {
            pending.key.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        let params = if !check && retry {
            self.computer.pending.as_ref().unwrap().params.clone()
        } else {
            let Some(params) = request(self.params(), action, &key, &self.computer.metadata, check)
            else {
                return;
            };
            params
        };
        if !check {
            self.computer.pending = Some(Pending {
                action,
                key,
                params: params.clone(),
            });
        }
        self.computer.confirm = None;
        self.computer.error = None;
        let target = self.target.clone();
        self.operation = Some(cx.spawn(async move |this, cx| {
            let result = target.controller.client().call_scoped(action.method(), params).await;
            let _ = this.update(cx, |this, cx| {
                this.operation = None;
                if !this.authorized { return; }
                match result {
                    Ok(value) if value["accountId"] == target.subject && value["organizationId"].as_str() == target.organization.as_deref() && (if action == Action::Delete { value["workspace"]["workspaceId"] == target.workspace } else { value["workspaceId"] == target.workspace }) => {
                        if !check { this.computer.pending = None; }
                        if action == Action::Delete {
                            this.computer.metadata = value["workspace"].clone();
                            this.status = json!({"state":"deleting","message":"Deleting computer…"});
                            this.environment = None;
                            this.execution = None;
                            this.terminal = None;
                            this.state.update(cx, |state, cx| state.detach_cloud_conversation_execution(cx));
                        } else {
                            this.computer.metadata[action.summary()] = value[action.summary()].clone();
                            if matches!(action, Action::Restart | Action::Recreate | Action::RetryRestart | Action::RetryRecreate) && !check {
                                this.execution = None;
                                this.terminal = None;
                                this.status = json!({"state":"connecting","message":"Reconnecting after the computer operation…"});
                                this.state.update(cx, |state, cx| state.detach_cloud_conversation_execution(cx));
                                this.environment = None;
                                this.watch_environment(false, cx);
                            }
                        }
                        this.refresh_computer(cx);
                        this.refresh_history(cx);
                    }
                    Ok(_) => this.revoke(cx),
                    Err(error) => this.computer.error = Some(format!("{error}. Check this computer before retrying; the original request identity is retained.")),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub(super) fn render_computer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let row = self.computer.metadata.clone();
        let busy = self.operation.is_some();
        let mut panel = div()
            .id("native-computer-panel")
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(16.))
            .py(px(12.));
        let ready = self.ready(cx);
        let lifecycle_enabled =
            self.operation.is_none() && self.computer.execution_controls_enabled();
        let sleeping = self.domain_sleeping() || self.status["state"] == "paused";
        panel = panel.child(
            div().flex().items_center().gap(px(4.)).child(
                popover::btn_ghost(
                    &theme,
                    if sleeping {
                        "Resume"
                    } else if ready {
                        "Sleep computer"
                    } else {
                        "Reconnect"
                    },
                    "native-cloud-lifecycle",
                )
                .id("native-cloud-lifecycle")
                .opacity(if lifecycle_enabled { 1. } else { 0.45 })
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.operation.is_some() || !this.computer.execution_controls_enabled() {
                        return;
                    }
                    if this.domain_sleeping() || this.status["state"] == "paused" {
                        this.watch_environment(true, cx);
                    } else if this.ready(cx) {
                        this.sleep(cx);
                    } else {
                        this.watch_environment(false, cx);
                    }
                })),
            ),
        );
        let description = [
            row["providerId"].as_str(),
            row["branch"].as_str(),
            row["state"].as_str(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        panel = panel.child(div().text_size(px(12.)).child(description));
        for (key, label) in [
            ("computerSizeId", "Size"),
            ("environmentRevision", "Environment version"),
        ] {
            if let Some(value) = row.get(key).filter(|v| !v.is_null()) {
                let text = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| value.to_string());
                panel = panel.child(div().text_size(px(12.)).child(format!("{label}: {text}")));
            }
        }
        if let Some((title, detail)) = issue(&row) {
            panel = panel
                .child(div().text_size(px(12.)).child(title))
                .child(popover::dialog_body(&theme, detail));
            panel = panel.child(
                popover::btn_ghost(&theme, "Cloud settings", "computer-setup-settings")
                    .id("computer-setup-settings")
                    .on_click(cx.listener(|_, _, window, cx| {
                        window.dispatch_action(Box::new(OpenCloudSettings), cx)
                    })),
            );
        }
        for step in preparation_steps(&row) {
            let running = step.state == zuse_viewport::CloudPreparationStepState::Running;
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .when(running, |line| {
                        line.child(crate::loaders::mini_mono_spinner(
                            format!("native-preparation-{}", step.key),
                            2.,
                            theme.text_muted,
                            cx.entity_id(),
                            cx,
                        ))
                    })
                    .child(format!("{}: {}", step.title, step.state.label())),
            );
        }
        let mut controls = div().flex().flex_wrap().gap(px(8.));
        if editable(&row) && !busy && self.ready(cx) && self.computer.execution_controls_enabled() {
            controls = controls.child(
                popover::btn_ghost(&theme, "Stop agent", "computer-stop-agent")
                    .id("computer-stop-agent")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.operation.is_none()
                            && this.ready(cx)
                            && this.computer.execution_controls_enabled()
                        {
                            this.composer
                                .update(cx, |composer, cx| composer.interrupt_selected(cx));
                        }
                    })),
            );
        }
        for action in [
            Action::Restart,
            Action::Recreate,
            Action::Delete,
            Action::RetryRestart,
            Action::RetryRecreate,
            Action::Continue,
            Action::Discard,
        ] {
            if !busy && self.computer.pending.is_none() && allowed(&row, action) {
                controls = controls.child(
                    popover::btn_ghost(&theme, action.label(), format!("computer-{action:?}"))
                        .id(SharedString::from(format!("computer-{action:?}")))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.computer.confirm = Some(action);
                            cx.notify();
                        })),
                );
            }
        }
        for action in [Action::Restart, Action::Recreate] {
            let operation = &row[action.summary()];
            if operation["operationId"].is_string() {
                let phase = operation["state"].as_str().unwrap_or("Checking");
                panel = panel.child(
                    div()
                        .text_size(px(12.))
                        .child(format!("{}: {phase}", action.label())),
                );
                if !busy {
                    controls =
                        controls.child(
                            popover::btn_ghost(
                                &theme,
                                &format!("Check {}", action.label().to_lowercase()),
                                format!("check-{action:?}"),
                            )
                            .id(SharedString::from(format!("check-{action:?}")))
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.computer_action(action, true, cx),
                            )),
                        );
                }
            }
        }
        if let Some(pending) = self.computer.pending.clone().filter(|_| !busy) {
            controls = controls.child(
                popover::btn_ghost(&theme, "Retry original request", "computer-retry-original")
                    .id("computer-retry-original")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.computer_action(pending.action, false, cx)
                    })),
            );
        }
        controls = controls.child(
            popover::btn_ghost(&theme, "Refresh computer", "computer-refresh")
                .id("computer-refresh")
                .on_click(cx.listener(|this, _, _, cx| this.refresh_computer(cx))),
        );
        panel = panel.child(controls);
        if busy {
            panel = panel.child(div().text_size(px(12.)).child("Request in progress…"));
        }
        if let Some(error) = &self.computer.error {
            panel = panel.child(div().text_size(px(12.)).child(error.clone()));
        }
        if let Some(action) = self.computer.confirm {
            panel = panel.child(
                popover::dialog_card(&theme)
                    .child(popover::dialog_title(
                        &theme,
                        &format!("{}?", action.label()),
                    ))
                    .child(popover::dialog_body(&theme, action.detail()))
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .mt(px(12.))
                            .child(
                                popover::btn_ghost(&theme, "Cancel", "computer-cancel")
                                    .id("computer-cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.computer.confirm = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                popover::btn_danger(&theme, action.label())
                                    .id("computer-confirm")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.computer_action(action, false, cx)
                                    })),
                            ),
                    ),
            );
        }
        if self.computer.logs.is_object() {
            let output = format!(
                "{}\n{}",
                self.computer.logs["stdout"].as_str().unwrap_or_default(),
                self.computer.logs["stderr"].as_str().unwrap_or_default()
            )
            .chars()
            .take(16_384)
            .collect::<String>();
            if !output.trim().is_empty() {
                panel = panel.child(div().text_size(px(12.)).child("Setup output"));
                panel = panel.child(
                    div()
                        .id("computer-setup-output")
                        .max_h(px(140.))
                        .overflow_y_scroll()
                        .text_size(px(11.))
                        .child(output),
                );
            }
        }
        panel.into_any_element()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn runtime_restart_detaches_execution_without_erasing_cloud_history(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, cx| {
            state.selected_chat = Some("cloud-session".into());
            let entry = zuse_model::SessionMessageEntry {
                id: "saved-message".into(),
                role: zuse_model::MessageRole::User,
                parts: vec![zuse_model::MessagePart::Text {
                    id: "text".into(),
                    text: "Keep this saved message".into(),
                }],
                created_at: 1,
                device_id: "cloud".into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            };
            state.transcript = vec![entry.clone()];
            state.push_echo("cloud-session", entry);
            state.detach_cloud_conversation_execution(cx);
            assert_eq!(state.selected_chat.as_deref(), Some("cloud-session"));
            assert_eq!(state.transcript[0].id, "saved-message");
            assert_eq!(state.pending_echoes()[0].id, "saved-message");
            assert!(state.engine().is_none());
            assert!(matches!(state.connection, ConnectionStatus::Connecting));
        });
    }
    #[test]
    fn delete_and_uncertain_delete_disable_the_composer() {
        let mut state = State::default();
        state.metadata = json!({"state":"ready"});
        assert!(state.composer_enabled());
        state.pending = Some(Pending {
            action: Action::Delete,
            key: "original".into(),
            params: json!({"commandId":"original"}),
        });
        assert!(!state.composer_enabled());
        state.pending = None;
        state.metadata["state"] = json!("deleted");
        assert!(!state.composer_enabled());
    }
    #[test]
    fn uncertain_or_deleting_operations_disable_conflicting_execution_controls() {
        let mut state = State::default();
        state.metadata = json!({"state":"ready"});
        assert!(state.execution_controls_enabled());
        for action in [Action::Delete, Action::Restart] {
            state.pending = Some(Pending {
                action,
                key: "original-request".into(),
                params: json!({"workspaceId":"original"}),
            });
            assert!(!state.execution_controls_enabled());
        }
        state.pending = None;
        for phase in ["deleting", "deleted", "archiving", "archived"] {
            state.metadata["state"] = json!(phase);
            assert!(!state.execution_controls_enabled());
        }
        state.metadata =
            json!({"state":"ready","runtimeRepair":{"operationId":"original","state":"starting"}});
        assert!(!state.execution_controls_enabled());
        state.metadata["runtimeRepair"]["state"] = json!("ready");
        assert!(state.execution_controls_enabled());
    }
    #[test]
    fn computer_actions_require_server_permission_and_real_capabilities() {
        let mut row = json!({"state":"setup","taskCommandCanEdit":true,"runtimeRepairCanEdit":true,"runtimeRepairAvailable":true});
        assert!(allowed(&row, Action::Restart));
        assert!(!allowed(&row, Action::Recreate));
        row["runtimeRepair"] = json!({"operationId":"repair","state":"starting"});
        assert!(!allowed(&row, Action::Restart));
        assert!(!allowed(&row, Action::Recreate));
        row["state"] = json!("deleted");
        assert!(!allowed(&row, Action::Delete));
        assert!(!allowed(&json!({"state":"ready"}), Action::Restart));
    }
    #[test]
    fn original_request_scope_and_identity_survive_retry_and_status_is_read_only() {
        let scope = json!({"workspaceId":"task","expectedAccountId":"owner","expectedOrganizationId":"org"});
        let row = json!({"runtimeRepair":{"operationId":"original"}});
        let first = request(scope.clone(), Action::Restart, "same-key", &row, false).unwrap();
        assert_eq!(
            first,
            request(scope.clone(), Action::Restart, "same-key", &row, false).unwrap()
        );
        assert_eq!(first["workspaceId"], "task");
        assert_eq!(first["expectedAccountId"], "owner");
        assert_eq!(first["expectedOrganizationId"], "org");
        let check = request(scope, Action::Restart, "ignored", &row, true).unwrap();
        assert_eq!(check["action"], "status");
        assert_eq!(check["operationId"], "original");
        assert!(check.get("idempotencyKey").is_none());
        let retry = request(
            json!({"workspaceId":"task"}),
            Action::RetryRestart,
            "unused",
            &row,
            false,
        )
        .unwrap();
        assert_eq!(retry["operationId"], "original");
        assert_eq!(retry["action"], "retry");
        assert!(retry.get("idempotencyKey").is_none());
    }
    #[test]
    fn recreation_retries_only_its_attention_phase_while_repair_requires_resolution() {
        let mut row = json!({"state":"ready","computerRecreationCanEdit":true,
            "runtimeRepairCanEdit":true,"computerRecreation":{"operationId":"recreate-original",
                "state":"attention-required","errorCode":"checkpoint_failed"}});
        assert!(allowed(&row, Action::RetryRecreate));
        assert!(!allowed(&row, Action::Recreate));
        let retry = request(
            json!({"workspaceId":"task"}),
            Action::RetryRecreate,
            "unused",
            &row,
            false,
        )
        .unwrap();
        assert_eq!(retry["action"], "retry");
        assert_eq!(retry["operationId"], "recreate-original");
        assert!(retry.get("idempotencyKey").is_none());
        row["computerRecreation"]["state"] = json!("restoring");
        assert!(!allowed(&row, Action::RetryRecreate));
        row["runtimeRepair"] = json!({"operationId":"repair-original",
            "state":"attention-required","errorCode":"outcome_unknown"});
        assert!(!allowed(&row, Action::RetryRestart));
        assert!(allowed(&row, Action::Continue));
    }
    #[test]
    fn repository_and_environment_failures_have_specific_saved_message_guidance() {
        for (stage, fragment) in [
            ("repository", "GitHub"),
            ("environment", "setup output"),
            ("credentials", "selected coding agent"),
        ] {
            let mut row = json!({"state":"ready","statusCode":"ready","preparation":{}});
            row["preparation"][stage] =
                json!({"state":"failed","errorCode":"unrecognized_failure"});
            assert!(
                issue(&row).unwrap().1.contains(fragment),
                "reported stage failure needs actionable guidance even when generic status hides it"
            );
        }
        let legacy = json!({"state":"ready","preparation":{"repository":{"state":"completed"},"environment":{"state":"completed"}}});
        let steps = preparation_steps(&legacy);
        assert_eq!(
            steps.iter().map(|step| step.key).collect::<Vec<_>>(),
            vec!["computer", "repository", "environment"],
            "legacy ready computer does not invent waiting credential/agent stages"
        );
        let pending = preparation_steps(&json!({"state":"provisioning"}));
        assert!(pending.iter().any(|step| step.key == "credentials"
            && step.state == zuse_viewport::CloudPreparationStepState::Pending));
        let explicit =
            preparation_steps(&json!({"state":"ready","preparation":{"agent":{"state":"failed"}}}));
        assert!(
            explicit.iter().any(|step| step.key == "agent"
                && step.key != "computer"
                && step.state == zuse_viewport::CloudPreparationStepState::Failed),
            "reported failure remains actionable on ready computer"
        );
        let credential = issue(&json!({"statusCode":"credential_preparation_failed"})).unwrap();
        assert_eq!(credential.0, "Coding agent connection needs attention");
        assert!(credential.1.contains("Reconnect the selected coding agent"));
        assert!(credential.1.contains("not sent again"));
        assert!(
            issue(&json!({"statusCode":"repository_preparation_failed"}))
                .unwrap()
                .1
                .contains("GitHub")
        );
        assert!(
            issue(&json!({"statusCode":"environment_setup_failed"}))
                .unwrap()
                .1
                .contains("setup output")
        );
    }
}
