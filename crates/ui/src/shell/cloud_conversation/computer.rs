//! Native Computer actions use the account API; execution availability is not authority.
use super::environment_card::{self, StatusAction};
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
/// How a status reads: its dot's color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::shell) enum Tone {
    Running,
    Busy,
    Idle,
    Failed,
}
impl Tone {
    pub(in crate::shell) fn color(self, theme: &Theme) -> gpui::Hsla {
        match self {
            Self::Running => theme.success,
            Self::Busy => theme.warning,
            Self::Idle => theme.text_muted.opacity(0.5),
            Self::Failed => theme.danger,
        }
    }
}
/// A cloud computer at a glance.
#[derive(Clone, Debug)]
pub(in crate::shell) struct ComputerSummary {
    pub label: &'static str,
    pub tone: Tone,
    pub sleeping: bool,
    pub repository: Option<String>,
    pub branch: Option<String>,
    pub created: Option<String>,
    pub size: Option<String>,
    /// How it last woke: from memory, or restarted on a provider that does
    /// not keep memory across sleep.
    pub last_wake: Option<String>,
}

/// "Woke from memory · 1.2 s" or "Restarted on wake · 6.4 s". Absent until the
/// computer has woken once under a runtime that reports it.
pub(in crate::shell) fn last_wake_label(row: &serde_json::Value) -> Option<String> {
    let wake = &row["lastWake"];
    let kind = match wake["fromMemory"].as_bool()? {
        true => "Woke from memory",
        false => "Restarted on wake",
    };
    Some(match wake["ms"].as_i64() {
        Some(ms) if ms >= 0 => format!("{kind} · {:.1} s", ms as f64 / 1000.0),
        _ => kind.to_owned(),
    })
}
/// The computer menu's pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Summary,
    /// Every action on the computer.
    More,
    /// Apps to open the synced folder in.
    OpenIn,
    /// Ways to open the computer over SSH.
    Ssh,
}
/// What to do once SSH access is ready.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SshAction {
    Monitor,
    Terminal,
    Copy,
    /// Open the repository in an installed editor or terminal.
    Open(crate::shell::ssh_open::SshTarget),
}
/// One row of the "Open via SSH" list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SshEntry {
    /// An installed app that opens the computer over SSH.
    App(crate::shell::ssh_open::SshTarget),
    /// The system terminal, before the apps are known.
    Terminal,
    Copy,
}
impl SshEntry {
    fn id(&self) -> &'static str {
        match self {
            Self::App(target) => target.app.id(),
            Self::Terminal => "terminal",
            Self::Copy => "copy",
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::App(target) => target.app.label(),
            Self::Terminal => "Terminal",
            Self::Copy => "Copy SSH command",
        }
    }
    fn action(&self) -> SshAction {
        match self {
            Self::App(target) => SshAction::Open(target.clone()),
            Self::Terminal => SshAction::Terminal,
            Self::Copy => SshAction::Copy,
        }
    }
}
/// The SSH list: every detected app in menu order (the system terminal
/// until detection finishes), then "Copy SSH command".
fn ssh_entries(targets: &[crate::shell::ssh_open::SshTarget]) -> Vec<SshEntry> {
    let mut entries: Vec<SshEntry> = targets.iter().cloned().map(SshEntry::App).collect();
    if entries.is_empty() {
        entries.push(SshEntry::Terminal);
    }
    entries.push(SshEntry::Copy);
    entries
}
/// Run a blocking app launch off the UI thread; a failure names the app.
fn launch_reporting(
    label: &'static str,
    job: impl FnOnce() -> Result<(), String> + Send + 'static,
    cx: &mut Context<CloudConversation>,
) {
    let launch = cx.background_executor().spawn(async move { job() });
    cx.spawn(async move |this, cx| {
        if let Err(error) = launch.await {
            tracing::warn!(target = label, %error, "open over SSH failed");
            let _ = this.update(cx, |_, cx| {
                crate::toast::show(
                    cx,
                    crate::toast::ToastKind::Error,
                    format!("Couldn't open {label} over SSH: {error}"),
                );
            });
        }
    })
    .detach();
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
    /// Local sync of this computer's repository: its last status, the poll
    /// that keeps it current while the menu is open, and a pending change.
    sync: Value,
    sync_poll: Option<Task<()>>,
    sync_change: Option<Task<()>>,
    sync_folder: Option<Task<()>>,
    sync_error: Option<String>,
    /// Which page of the menu shows.
    page: Page,
    /// SSH access: the host once prepared, the preparation, its failure.
    ssh: Option<crate::cloud_ssh::Prepared>,
    ssh_task: Option<Task<()>>,
    ssh_error: Option<String>,
    monitor_connecting: bool,
    monitor: Option<super::monitor::Connection>,
    monitor_browser: Option<Entity<crate::browser::BrowserSurface>>,
    /// The apps this computer can open a folder in (the shell detects them).
    open_targets: Vec<crate::shell::open_in::OpenTarget>,
    /// The apps this computer can open the cloud computer in over SSH.
    ssh_targets: Vec<crate::shell::ssh_open::SshTarget>,
    /// The machine's last CPU, memory and disk snapshot, and the poll that
    /// keeps it current while the summary shows it.
    pub(super) resources: Value,
    resources_poll: Option<Task<()>>,
    resources_wanted: Option<std::time::Instant>,
}
impl State {
    pub(super) fn monitor_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<CloudConversation>,
    ) -> Option<Entity<crate::browser::BrowserSurface>> {
        let connection = self.monitor.as_ref()?;
        if self.monitor_browser.is_none() {
            // An isolated browser keeps the SSH-delivered credential out of shared history.
            let browser = cx.new(|cx| {
                crate::browser::BrowserSurface::new(Default::default(), false, window, cx)
            });
            browser.update(cx, |view, cx| {
                view.hide_toolbar();
                view.set_presentation(crate::browser::model::Presentation::Live, cx);
                view.navigate(&connection.url, window, cx);
            });
            self.monitor_browser = Some(browser);
        }
        self.monitor_browser.clone()
    }

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
pub(super) fn preparation_steps(row: &Value) -> Vec<zuse_viewport::CloudPreparationStep> {
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
        "cloud_retained_machine_missing" => Some((
            "This computer no longer exists",
            "This computer no longer exists in Cloud, so it can't wake. Recreate it from this menu, or start a new chat. This conversation is saved.",
        )),
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
/// A bordered, monospace field with a main action and a chevron for more,
/// like the previous Zuse summary card's SSH and synced-folder controls.
fn split_field(theme: &Theme) -> gpui::Div {
    div()
        .mx(px(8.))
        .my(px(6.))
        .h(px(28.))
        .flex()
        .items_center()
        .rounded(px(6.))
        .border_1()
        .border_color(theme.text.opacity(0.15))
        .bg(theme.muted_fill().opacity(0.4))
        .overflow_hidden()
        .font_family("Geist Mono")
}
fn split_main(theme: &Theme, id: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .h_full()
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(8.))
        .cursor_pointer()
        .hover(|el| el.bg(theme.element_hover))
}
fn split_chevron(theme: &Theme, id: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .w(px(28.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .border_l_1()
        .border_color(theme.text.opacity(0.15))
        .cursor_pointer()
        .hover(|el| el.bg(theme.element_hover))
        .role(gpui::Role::Button)
        .child(
            icon(icons::ALT_ARROW_DOWN)
                .size(px(13.))
                .text_color(theme.text_muted),
        )
}
impl CloudConversation {
    pub(super) fn close_computer(&mut self, cx: &mut Context<Self>) {
        self.computer.open = false;
        self.computer.page = Page::Summary;
        self.computer.confirm = None;
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
            // Apps installed since the last look show up in the SSH and
            // open-in lists; the last snapshot stays while this runs.
            cx.emit(CloudConversationEvent::RefreshOpenTargets);
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
                this.publish_computer_online(cx);
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
    /// The composer's Update computer started a recreation: follow it like
    /// one started from this menu.
    pub(super) fn computer_recreation_started(
        &mut self,
        recreation: Value,
        cx: &mut Context<Self>,
    ) {
        if !self.authorized {
            return;
        }
        self.computer.metadata["computerRecreation"] = recreation;
        self.execution = None;
        self.terminal = None;
        self.status = json!({"state":"connecting","message":"Updating computer…"});
        self.state.update(cx, |state, cx| {
            state.detach_cloud_conversation_execution(cx)
        });
        self.environment = None;
        self.watch_environment(false, cx);
        self.refresh_computer(cx);
        self.refresh_history(cx);
        cx.notify();
    }
    fn sync_body(&self) -> Value {
        json!({"expectedAccountId":self.target.subject,"expectedOrganizationId":self.target.organization,"workspaceId":self.target.workspace})
    }
    /// Keep the local sync status current while the menu is open.
    fn watch_sync(&mut self, cx: &mut Context<Self>) {
        if self.computer.sync_poll.is_some() || !self.authorized {
            return;
        }
        let target = self.target.clone();
        let body = self.sync_body();
        self.computer.sync_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let open = this
                    .update(cx, |this, _| this.computer.open && this.authorized)
                    .unwrap_or(false);
                if !open {
                    let _ = this.update(cx, |this, _| this.computer.sync_poll = None);
                    return;
                }
                let idle = this
                    .update(cx, |this, _| this.computer.sync_change.is_none())
                    .unwrap_or(false);
                if idle {
                    let result = target
                        .controller
                        .client()
                        .call_scoped(methods::CLOUD_SYNC_STATUS, body.clone())
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.computer.sync_change.is_some() {
                            return;
                        }
                        match result {
                            Ok(value) if value["accountId"] == target.subject => {
                                this.computer.sync = value["status"].clone();
                                this.computer.sync_error = None;
                            }
                            Ok(_) => {}
                            Err(error) => this.computer.sync_error = Some(error.to_string()),
                        }
                        cx.notify();
                    });
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }));
    }
    fn change_sync(
        &mut self,
        method: &'static str,
        enabled: bool,
        path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.computer.sync_change.is_some() || !self.authorized {
            return;
        }
        let target = self.target.clone();
        let mut body = self.sync_body();
        body["enabled"] = json!(enabled);
        body["localPath"] = json!(path);
        self.computer.sync_error = None;
        self.computer.sync_change = Some(cx.spawn(async move |this, cx| {
            let result = target.controller.client().call_scoped(method, body).await;
            let _ = this.update(cx, |this, cx| {
                this.computer.sync_change = None;
                match result {
                    Ok(value) if value["accountId"] == target.subject => {
                        this.computer.sync = value["status"].clone()
                    }
                    Ok(_) => {}
                    Err(error) => this.computer.sync_error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn choose_sync_folder(&mut self, cx: &mut Context<Self>) {
        if self.computer.sync_change.is_some() || self.computer.sync_folder.is_some() {
            return;
        }
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose Sync Folder".into()),
        });
        self.computer.sync_folder = Some(cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |this, cx| {
                this.computer.sync_folder = None;
                if let Ok(Ok(Some(paths))) = result
                    && let Some(path) = paths.first()
                {
                    this.change_sync(
                        methods::CLOUD_SYNC_CONFIGURE,
                        true,
                        Some(path.to_string_lossy().into_owned()),
                        cx,
                    );
                }
                cx.notify();
            });
        }));
    }
    /// The synced local folder, when sync is on and has one.
    pub(in crate::shell) fn synced_folder(&self) -> Option<String> {
        (self.computer.sync["enabled"] == true)
            .then(|| self.computer.sync["localPath"].as_str().map(str::to_owned))
            .flatten()
    }
    /// What the summary and the menu say about this computer.
    pub(in crate::shell) fn computer_summary(&self) -> ComputerSummary {
        let row = &self.computer.metadata;
        let state = if self.domain_sleeping() || self.status["state"] == "paused" {
            "paused"
        } else {
            row["state"].as_str().unwrap_or("starting")
        };
        let (label, tone) = match state {
            "ready" => ("Running", Tone::Running),
            "paused" => ("Sleeping", Tone::Idle),
            "pausing" => ("Going to sleep…", Tone::Idle),
            "failed" | "error" | "blocked" => ("Needs attention", Tone::Failed),
            "deleting" | "deleted" => ("Deleted", Tone::Idle),
            _ => ("Starting…", Tone::Busy),
        };
        let config = &self.config;
        let repository = row["repositoryDisplayName"]
            .as_str()
            .or_else(|| config["repositoryDisplayName"].as_str())
            .or_else(|| row["repositoryIdentity"].as_str())
            .or_else(|| config["repositoryIdentity"].as_str())
            .map(str::to_owned)
            .or_else(|| {
                row["repositoryUrl"].as_str().map(|url| {
                    url.trim_end_matches(".git")
                        .trim_start_matches("https://")
                        .trim_start_matches("github.com/")
                        .to_owned()
                })
            });
        ComputerSummary {
            label,
            tone,
            sleeping: state == "paused",
            repository,
            branch: self.branch_label(),
            size: row["computerSizeId"]
                .as_str()
                .or_else(|| self.config["sizeId"].as_str())
                .filter(|size| !size.is_empty())
                .map(|size| {
                    if let Some(label) = zuse_protocol::cloud_billing::plans::size_label(size) {
                        return label.to_owned();
                    }
                    let mut chars = size.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                        .unwrap_or_default()
                }),
            last_wake: last_wake_label(row),
            created: row["createdAt"]
                .as_i64()
                .and_then(chrono::DateTime::from_timestamp_millis)
                .map(|time| {
                    time.with_timezone(&chrono::Local)
                        .format("%b %-d, %-I:%M %p")
                        .to_string()
                }),
        }
    }
    /// Keep the machine's CPU, memory and disk current while something shows
    /// them. Each call extends the want; the poll stops a few seconds after
    /// the last one. Reads go to the computer's own engine, so they need it
    /// connected.
    pub(in crate::shell) fn want_resources(&mut self, cx: &mut Context<Self>) {
        self.computer.resources_wanted = Some(std::time::Instant::now());
        if self.computer.resources_poll.is_some() {
            return;
        }
        self.computer.resources_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let engine = this
                    .update(cx, |this, cx| {
                        let wanted = this
                            .computer
                            .resources_wanted
                            .is_some_and(|at| at.elapsed() < Duration::from_secs(4));
                        if !wanted {
                            return Err(());
                        }
                        Ok(this.state.read(cx).engine().cloned())
                    })
                    .ok()
                    .and_then(Result::ok);
                let Some(engine) = engine else {
                    let _ = this.update(cx, |this, _| this.computer.resources_poll = None);
                    return;
                };
                if let Some(engine) = engine {
                    let job = gpui_tokio::Tokio::spawn(cx, async move {
                        engine
                            .client()
                            .call(methods::MACHINE_RESOURCES, json!({}))
                            .await
                    });
                    if let Ok(Ok(value)) = job.await {
                        let _ = this.update(cx, |this, cx| {
                            this.computer.resources = value;
                            cx.notify();
                        });
                    }
                }
                cx.background_executor().timer(Duration::from_secs(3)).await;
            }
        }));
    }
    /// The apps the synced folder, and the computer over SSH, can open in,
    /// as the shell detected them.
    pub(in crate::shell) fn set_open_targets(
        &mut self,
        targets: Vec<crate::shell::open_in::OpenTarget>,
        ssh_targets: Vec<crate::shell::ssh_open::SshTarget>,
    ) {
        self.computer.open_targets = targets;
        self.computer.ssh_targets = ssh_targets;
    }
    /// The last machine snapshot, when one arrived.
    pub(in crate::shell) fn resources(&self) -> &Value {
        &self.computer.resources
    }
    /// Pause a running computer, or wake a sleeping one.
    pub(in crate::shell) fn toggle_sleep(&mut self, cx: &mut Context<Self>) {
        if self.operation.is_some() || !self.computer.execution_controls_enabled() {
            return;
        }
        if self.domain_sleeping() || self.status["state"] == "paused" {
            self.watch_environment(true, cx);
        } else if self.ready(cx) {
            self.sleep(cx);
        } else {
            self.watch_environment(false, cx);
        }
    }
    /// Whether the computer is up, so SSH can reach it.
    pub(in crate::shell) fn desktop_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<crate::browser::BrowserSurface>> {
        if !self.authorized || self.access_revoked || !self.computer_running() {
            self.disconnect_desktop(cx);
            return None;
        }
        if self.ready(cx)
            && self.computer.monitor.is_none()
            && self.computer.ssh_task.is_none()
            && self.computer.ssh_error.is_none()
        {
            self.open_ssh(SshAction::Monitor, cx);
        }
        self.computer.monitor_view(window, cx)
    }
    pub(in crate::shell) fn connect_desktop(&mut self, cx: &mut Context<Self>) {
        if self.authorized && !self.access_revoked {
            if self.domain_sleeping() || self.status["state"] == "paused" {
                self.watch_environment(true, cx);
            } else {
                self.open_ssh(SshAction::Monitor, cx);
            }
        }
    }
    pub(in crate::shell) fn desktop_status(&self) -> (&str, Option<&'static str>) {
        if !self.authorized || self.access_revoked {
            return ("Waiting for cloud access", None);
        }
        if !self.computer_running() {
            let wake = (self.domain_sleeping() || self.status["state"] == "paused")
                && self.operation.is_none()
                && self.computer.execution_controls_enabled();
            return (
                "The cloud computer is offline",
                wake.then_some("Wake computer"),
            );
        }
        if self.computer.ssh_task.is_some() {
            return ("Connecting to desktop…", None);
        }
        (
            self.computer
                .ssh_error
                .as_deref()
                .unwrap_or("Open this computer’s desktop"),
            Some("Connect"),
        )
    }
    pub(in crate::shell) fn disconnect_desktop(&mut self, cx: &mut Context<Self>) {
        if self.computer.monitor_connecting {
            self.computer.ssh_task.take();
            self.computer.monitor_connecting = false;
        }
        self.computer.monitor.take();
        if let Some(browser) = self.computer.monitor_browser.take() {
            browser.update(cx, |v, cx| {
                v.set_presentation(crate::browser::model::Presentation::Hidden, cx);
                v.close(cx);
            });
        }
    }
    fn computer_running(&self) -> bool {
        matches!(self.computer_summary().tone, Tone::Running)
    }
    /// The status line's Retry on a computer that needs attention: the
    /// repair the server offers for it, confirmed in the computer menu.
    pub(super) fn retry_failed_computer(&mut self, cx: &mut Context<Self>) {
        self.open_computer(cx);
        self.computer.confirm = [
            Action::RetryRestart,
            Action::RetryRecreate,
            Action::Restart,
            Action::Recreate,
        ]
        .into_iter()
        .find(|action| allowed(&self.computer.metadata, *action));
        cx.notify();
    }
    /// The status line's Delete: confirmed in the computer menu, like Delete
    /// computer there.
    pub(super) fn delete_failed_computer(&mut self, cx: &mut Context<Self>) {
        self.open_computer(cx);
        self.computer.confirm = Some(Action::Delete);
        cx.notify();
    }
    pub(in crate::shell) fn open_computer(&mut self, cx: &mut Context<Self>) {
        if !self.computer.open {
            self.toggle_computer(cx);
        }
    }
    /// Prepare SSH access (this device's key, the computer's sshd, the host
    /// entry), then run `action`.
    fn open_ssh(&mut self, action: SshAction, cx: &mut Context<Self>) {
        if self.computer.ssh_task.is_some() {
            return;
        }
        if !self.computer_running() {
            self.computer.ssh_error = Some("Wake the computer to open it over SSH.".into());
            cx.notify();
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.connect_execution(cx);
            self.computer.ssh_error =
                Some("Connecting to the computer. Try again in a moment.".into());
            cx.notify();
            return;
        };
        let workspace = self.target.workspace.clone();
        let port = self.boot.ipc_port;
        self.computer.ssh_error = None;
        self.computer.monitor_connecting = action == SshAction::Monitor;
        self.computer.ssh_task = Some(cx.spawn(async move |this, cx| {
            let result: Result<crate::cloud_ssh::Prepared, String> = async {
                let key = cx
                    .background_executor()
                    .spawn(async { crate::cloud_ssh::ensure_key() })
                    .await?;
                let prepared = gpui_tokio::Tokio::spawn(cx, async move {
                    engine
                        .client()
                        .call(methods::SSH_PREPARE, json!({"publicKey": key}))
                        .await
                })
                .await
                .map_err(|_| "SSH setup was interrupted.".to_string())?
                .map_err(|e| e.to_string())?;
                let user = prepared["user"].as_str().unwrap_or("zuse").to_owned();
                let path = prepared["path"]
                    .as_str()
                    .unwrap_or("/home/zuse/workspace")
                    .to_owned();
                let host_key = prepared["hostKey"].as_str().map(str::to_owned);
                if action == SshAction::Monitor && host_key.is_none() {
                    return Err("Monitor requires a verified SSH host key.".into());
                }
                let host_workspace = workspace.clone();
                let host_user = user.clone();
                cx.background_executor()
                    .spawn(async move {
                        if let Some(host_key) = host_key {
                            crate::cloud_ssh::pin_host_key(&host_workspace, &host_key)?;
                        }
                        crate::cloud_ssh::install_host(&host_workspace, &host_user, port)
                    })
                    .await?;
                Ok(crate::cloud_ssh::Prepared {
                    host: crate::cloud_ssh::host_alias(&workspace),
                    user,
                    path,
                })
            }
            .await;
            let monitor = if action == SshAction::Monitor {
                if let Ok(prepared) = &result {
                    let host = prepared.host.clone();
                    Some(
                        gpui_tokio::Tokio::spawn(cx, async move {
                            super::monitor::Connection::open(&host).await
                        })
                        .await
                        .unwrap_or_else(|_| Err("Monitor connection was interrupted.".into())),
                    )
                } else {
                    None
                }
            } else {
                None
            };
            let _ = this.update(cx, |this, cx| {
                this.computer.ssh_task = None;
                this.computer.monitor_connecting = false;
                match result {
                    Ok(prepared) => {
                        this.computer.ssh = Some(prepared.clone());
                        match action {
                            SshAction::Monitor => match monitor.expect("Monitor SSH handshake") {
                                Ok(connection) => {
                                    this.computer.monitor = Some(connection);
                                    this.close_computer(cx);
                                }
                                Err(error) => this.computer.ssh_error = Some(error),
                            },
                            SshAction::Copy => {
                                cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                    "ssh {}",
                                    prepared.host
                                )));
                                this.close_computer(cx);
                            }
                            SshAction::Terminal => {
                                let command = crate::cloud_ssh::terminal_command(&prepared);
                                launch_reporting(
                                    "Terminal",
                                    move || crate::cloud_ssh::open_terminal(&command),
                                    cx,
                                );
                                this.close_computer(cx);
                            }
                            SshAction::Open(target) => {
                                launch_reporting(
                                    target.app.label(),
                                    move || crate::shell::ssh_open::open(&target, &prepared),
                                    cx,
                                );
                                this.close_computer(cx);
                            }
                        }
                    }
                    Err(error) => this.computer.ssh_error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    /// SSH: the back row, then [`Self::render_ssh_entries`] and a hint.
    fn render_ssh_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let busy = self.computer.ssh_task.is_some();
        let host = crate::cloud_ssh::host_alias(&self.target.workspace);
        let mut page = div()
            .id("native-computer-ssh")
            .flex()
            .flex_col()
            .text_size(crate::typography::text_xs())
            .child(
                popover::menu_row(&theme, false, "computer-ssh-back")
                    .id("computer-ssh-back")
                    .child(icon(icons::ALT_ARROW_LEFT).size(px(14.)).text_color(theme.text_muted))
                    .child(div().flex_1().text_color(theme.text_muted).child("Computer"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.computer.page = Page::Summary;
                        cx.notify();
                    })),
            )
            .child(popover::menu_separator())
            .children(self.render_ssh_entries(&theme, cx))
            .child(popover::menu_separator())
            .child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_color(theme.text_muted)
                    .child(if busy {
                        "Setting up SSH access…".to_owned()
                    } else if self.computer.ssh.is_some() {
                        format!("Ready. Editors with Remote SSH can connect to {host}.")
                    } else {
                        format!("Adds the host {host} to your SSH config, for terminals and editors with Remote SSH.")
                    }),
            );
        if let Some(error) = self.computer.ssh_error.clone() {
            page = page.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(theme.danger)
                    .child(error),
            );
        }
        page.into_any_element()
    }
    /// The "Open via SSH" list on its own (no back row or hint), so it can
    /// show in any menu: each installed app, then "Copy SSH command".
    fn render_ssh_entries(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // Like the SSH row itself: dimmed and inert unless the computer runs.
        let enabled = self.computer.ssh_task.is_none() && self.computer_running();
        let mut rows = Vec::new();
        for entry in ssh_entries(&self.computer.ssh_targets) {
            let id = format!("computer-ssh-{}", entry.id());
            let glyph: AnyElement = match &entry {
                SshEntry::App(target) => match &target.icon {
                    Some(png) => gpui::img(png.clone()).size(px(16.)).into_any_element(),
                    None => icon(if target.app.is_terminal() {
                        icons::TERMINAL
                    } else {
                        crate::shell::open_in::fallback_glyph(target.app.id())
                    })
                    .size(px(14.))
                    .text_color(theme.text_muted)
                    .into_any_element(),
                },
                SshEntry::Terminal => icon(icons::TERMINAL)
                    .size(px(14.))
                    .text_color(theme.text_muted)
                    .into_any_element(),
                SshEntry::Copy => icon(icons::COPY)
                    .size(px(14.))
                    .text_color(theme.text_muted)
                    .into_any_element(),
            };
            if entry == SshEntry::Copy {
                rows.push(popover::menu_separator().into_any_element());
            }
            let label = entry.label();
            let action = entry.action();
            rows.push(
                popover::menu_row(theme, false, id.clone())
                    .id(SharedString::from(id))
                    .role(gpui::Role::MenuItem)
                    .aria_label(label)
                    .opacity(if enabled { 1. } else { 0.5 })
                    .child(
                        div()
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(glyph),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if enabled {
                            this.open_ssh(action.clone(), cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        rows
    }
    /// The synced folder's apps: each opens it there, then copy its path.
    fn render_open_in_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let path = self.synced_folder();
        let sync_busy = self.computer.sync_change.is_some() || self.computer.sync_folder.is_some();
        let mut page = div()
            .id("native-computer-open-in")
            .flex()
            .flex_col()
            .text_size(crate::typography::text_xs())
            .child(
                popover::menu_row(&theme, false, "computer-open-in-back")
                    .id("computer-open-in-back")
                    .child(
                        icon(icons::ALT_ARROW_LEFT)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(theme.text_muted)
                            .child("Computer"),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.computer.page = Page::Summary;
                        cx.notify();
                    })),
            )
            .child(popover::menu_separator());
        let Some(path) = path else {
            return page
                .child(
                    div()
                        .px(px(8.))
                        .py(px(6.))
                        .text_color(theme.text_muted)
                        .child("Turn on sync to open this computer's files here."),
                )
                .into_any_element();
        };
        for target in self.computer.open_targets.clone() {
            let glyph: AnyElement = match &target.icon {
                Some(png) => gpui::img(png.clone()).size(px(16.)).into_any_element(),
                None => icon(crate::shell::open_in::fallback_glyph(target.id))
                    .size(px(14.))
                    .text_color(theme.text_muted)
                    .into_any_element(),
            };
            let open_path = path.clone();
            page = page.child(
                popover::menu_row(&theme, false, format!("cloud-open-in-{}", target.id))
                    .id(SharedString::from(format!("cloud-open-in-{}", target.id)))
                    .child(
                        div()
                            .size(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(glyph),
                    )
                    .child(target.label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.emit(CloudConversationEvent::OpenPathIn(
                            target.clone(),
                            open_path.clone(),
                        ));
                        this.close_computer(cx);
                    })),
            );
        }
        let copy = path.clone();
        page.child(popover::menu_separator())
            .child(
                popover::menu_row(&theme, false, "cloud-open-in-copy")
                    .id("cloud-open-in-copy")
                    .child(icon(icons::COPY).size(px(14.)).text_color(theme.text_muted))
                    .child("Copy path")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        this.close_computer(cx);
                    })),
            )
            .child(
                popover::menu_row(&theme, false, "cloud-sync-folder")
                    .id("cloud-sync-folder")
                    .opacity(if sync_busy { 0.5 } else { 1. })
                    .child(
                        icon(icons::FOLDER)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child("Change folder…")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !sync_busy {
                            this.choose_sync_folder(cx);
                        }
                    })),
            )
            .into_any_element()
    }
    pub(super) fn render_computer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        /// The width of a reading's load bar.
        const COMPUTER_LOAD_BAR_WIDTH: f32 = 36.;
        self.watch_sync(cx);
        let theme = Theme::of(cx).for_popup();
        let row = self.computer.metadata.clone();
        let busy = self.operation.is_some();
        let summary = self.computer_summary();
        let running = matches!(summary.tone, Tone::Running);
        if running && self.computer.page == Page::Summary {
            self.want_resources(cx);
        }
        let card = environment_card::card(environment_card::Inputs {
            summary: &summary,
            row: &row,
            config: &self.config,
            resources: &self.computer.resources,
            now: chrono::Utc::now(),
            restart_allowed: !busy
                && self.computer.pending.is_none()
                && allowed(&row, Action::Restart),
        });
        // One summary row: icon, label, then the value pushed to the end.
        let detail = |id: &'static str, label: &'static str, glyph: &'static str| {
            div()
                .id(id)
                .px(px(8.))
                .h(px(28.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    icon(glyph)
                        .size(px(14.))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(div().flex_none().child(label))
        };
        let value = || {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_end()
                .gap(px(6.))
                .text_color(theme.text_muted)
        };
        let text = |value: String| div().min_w_0().truncate().child(value);
        let dot = |tone: Tone| {
            div()
                .flex_none()
                .size(px(6.))
                .rounded_full()
                .bg(tone.color(&theme))
        };

        // Title, and every other action behind the overflow button.
        let mut menu = div()
            .id("native-computer-panel")
            .flex()
            .flex_col()
            .text_size(crate::typography::text_xs())
            .child(
                div()
                    .pl(px(8.))
                    .pr(px(2.))
                    .pt(px(2.))
                    .pb(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::text_sm())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .id("computer-more")
                            .flex_none()
                            .size(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|el| el.bg(theme.element_hover))
                            .role(gpui::Role::Button)
                            .aria_label("More computer actions")
                            .tip("More")
                            .child(
                                icon(icons::MORE_HORIZONTAL)
                                    .size(px(14.))
                                    .text_color(theme.text_muted),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.computer.page = Page::More;
                                cx.notify();
                            })),
                    ),
            );

        // Who and what: repository, created, setup.
        if let Some(repository) = card.repository.clone() {
            let mut name = value().child(text(repository.name));
            if let Some(url) = repository.url {
                name = name.child(
                    div()
                        .id("computer-repository-link")
                        .flex_none()
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|el| el.bg(theme.element_hover))
                        .tip("Open repository")
                        .child(
                            icon(icons::ARROW_UP_RIGHT)
                                .size(px(12.))
                                .text_color(theme.text_muted),
                        )
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );
            }
            menu =
                menu.child(detail("computer-repository", "Repository", icons::FOLDER).child(name));
        }
        if let Some(created) = card.created.clone() {
            let mut created_row = detail("computer-created", "Created", icons::CLOCK_CIRCLE)
                .child(value().child(text(created.relative)));
            if let Some(exact) = created.exact {
                created_row = created_row.tip(exact);
            }
            menu = menu.child(created_row);
        }
        if let Some((label, tone)) = card.setup {
            menu = menu.child(
                detail("computer-setup", "Setup", icons::PR_REPAIR).child(
                    value()
                        .when(tone == Tone::Failed, |el| el.text_color(theme.danger))
                        .child(text(label.to_owned())),
                ),
            );
        }

        // The machine: where it runs, its state, and what it is using.
        let mut status = value();
        if let Some(action) = card.action {
            let enabled = action == StatusAction::Wake
                || (!busy && self.computer.execution_controls_enabled());
            status = status.child(
                div()
                    .id("computer-status-action")
                    .flex_none()
                    .h(px(20.))
                    .px(px(4.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .opacity(if enabled { 1. } else { 0.45 })
                    .hover(|el| el.bg(theme.element_hover).text_color(theme.text))
                    .role(gpui::Role::Button)
                    .child(
                        icon(match action {
                            StatusAction::Restart => icons::RESTART,
                            StatusAction::Wake => icons::ACTION_PLAY,
                        })
                        .size(px(12.))
                        .text_color(theme.text_muted),
                    )
                    .child(action.label())
                    .on_click(cx.listener(move |this, _, _, cx| match action {
                        StatusAction::Restart => {
                            this.computer.confirm = Some(Action::Restart);
                            cx.notify();
                        }
                        StatusAction::Wake => this.toggle_sleep(cx),
                    })),
            );
        }
        status = status.child(dot(card.tone)).child(card.status);
        let mut status_row =
            detail("computer-status", "Status", icons::CLOUD_ACTIVITY).child(status);
        if let Some(wake) = card.last_wake.clone() {
            status_row = status_row.tip(wake);
        }
        menu = menu
            .child(popover::menu_separator())
            .child(
                detail("computer-environment", "Environment", icons::CLOUD)
                    .child(value().child(text(card.environment.clone()))),
            )
            .child(status_row);
        if let Some(startup) = card.startup.clone() {
            menu = menu.child(
                detail("computer-startup", "Startup", icons::CLOCK_CIRCLE).child(
                    value()
                        .child(crate::loaders::mini_mono_spinner(
                            "computer-startup-spinner",
                            2.,
                            theme.text_muted,
                            cx.entity_id(),
                            cx,
                        ))
                        .child(text(startup)),
                ),
            );
        }
        // Live readings carry a small dithered load bar before the value.
        let load = super::load::machine_load(&self.computer.resources);
        for (id, label, glyph, reading, fraction) in [
            (
                "computer-cpu",
                "CPU",
                icons::CLOUD_CPU,
                card.cpu.clone(),
                load.cpu,
            ),
            (
                "computer-memory",
                "Memory",
                icons::CLOUD_MEMORY,
                card.memory.clone(),
                load.memory,
            ),
            (
                "computer-disk",
                "Disk",
                icons::HARD_DRIVE,
                card.disk.clone(),
                load.disk,
            ),
            (
                "computer-size",
                "Size",
                icons::MONITOR,
                card.size.clone(),
                None,
            ),
        ] {
            if let Some(reading) = reading {
                let bar = fraction.map(|fraction| {
                    crate::dither_bar::load_bar(fraction, &theme).w(px(COMPUTER_LOAD_BAR_WIDTH))
                });
                menu = menu.child(
                    detail(id, label, glyph).child(value().children(bar).child(text(reading))),
                );
            }
        }

        if let Some((title, detail)) = issue(&row) {
            menu = menu.child(popover::menu_separator()).child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().text_color(theme.danger).child(title))
                    .child(div().text_color(theme.text_muted).child(detail)),
            );
            menu = menu.child(
                popover::menu_row(&theme, false, "computer-setup-settings")
                    .id("computer-setup-settings")
                    .child(
                        icon(icons::SETTINGS)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child("Open Cloud settings")
                    .on_click(cx.listener(|_, _, window, cx| {
                        window.dispatch_action(Box::new(OpenCloudSettings), cx)
                    })),
            );
        }

        // Every action lives on the "More" page.
        let mut more_rows: Vec<AnyElement> = Vec::new();
        let lifecycle_enabled = !busy && self.computer.execution_controls_enabled();
        more_rows.push(
            popover::menu_row(&theme, false, "native-cloud-lifecycle")
                .id("native-cloud-lifecycle")
                .opacity(if lifecycle_enabled { 1. } else { 0.45 })
                .child(
                    icon(if summary.sleeping {
                        icons::ACTION_PLAY
                    } else {
                        icons::MOON
                    })
                    .size(px(14.))
                    .text_color(theme.text_muted),
                )
                .child(if summary.sleeping {
                    "Wake computer"
                } else if self.ready(cx) {
                    "Put computer to sleep"
                } else {
                    "Reconnect"
                })
                .on_click(cx.listener(|this, _, _, cx| this.toggle_sleep(cx)))
                .into_any_element(),
        );
        if editable(&row) && !busy && self.ready(cx) && self.computer.execution_controls_enabled() {
            more_rows.push(
                popover::menu_row(&theme, false, "computer-stop-agent")
                    .id("computer-stop-agent")
                    .child(icon(icons::STOP).size(px(14.)).text_color(theme.text_muted))
                    .child("Stop agent")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.operation.is_none()
                            && this.ready(cx)
                            && this.computer.execution_controls_enabled()
                        {
                            this.composer
                                .update(cx, |composer, cx| composer.interrupt_selected(cx));
                        }
                    }))
                    .into_any_element(),
            );
        }

        if settings::cloud_desktop_enabled(cx) {
            more_rows.push(
                popover::menu_row(&theme, false, "computer-monitor")
                    .id("computer-monitor")
                    .child("Open Desktop")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_computer(cx);
                        cx.emit(CloudConversationEvent::Desktop);
                    }))
                    .into_any_element(),
            );
        }
        // SSH, while the computer runs: the main part opens a terminal, the
        // chevron lists every way in.
        let ssh_busy = self.computer.ssh_task.is_some();
        menu = menu.child(
            split_field(&theme)
                .opacity(if running { 1. } else { 0.45 })
                .child(
                    split_main(&theme, "computer-ssh")
                        .role(gpui::Role::Button)
                        .child(
                            icon(icons::TERMINAL)
                                .size(px(14.))
                                .flex_none()
                                .text_color(theme.text_muted),
                        )
                        .child(div().min_w_0().truncate().child(if ssh_busy {
                            "Setting up SSH…"
                        } else {
                            "Open via SSH"
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if running {
                                this.open_ssh(SshAction::Terminal, cx);
                            }
                        })),
                )
                .child(
                    split_chevron(&theme, "computer-ssh-more")
                        .aria_label("Other ways to open via SSH")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if running {
                                this.computer.page = Page::Ssh;
                                cx.notify();
                            }
                        })),
                ),
        );
        if let Some(error) = self.computer.ssh_error.clone() {
            menu = menu.child(
                div()
                    .px(px(8.))
                    .pb(px(4.))
                    .text_color(theme.danger)
                    .child(error),
            );
        }

        // Local sync: the switch, its state, then the folder.
        let sync = &self.computer.sync;
        let loaded = sync["enabled"].is_boolean();
        let enabled = sync["enabled"] == true;
        let sync_busy = self.computer.sync_change.is_some() || self.computer.sync_folder.is_some();
        let phase = match sync["state"].as_str() {
            Some("synced") => ("In sync", Tone::Running),
            Some("syncing") => ("Syncing…", Tone::Busy),
            Some("waiting") => ("Waiting for the computer", Tone::Idle),
            Some("error") => ("Sync failed", Tone::Failed),
            Some("pending") => ("Preparing sync…", Tone::Busy),
            Some("stopping") => ("Stopping sync…", Tone::Busy),
            _ => ("Sync off", Tone::Idle),
        };
        menu = menu.child(popover::menu_separator()).child(
            div()
                .id("cloud-sync-toggle")
                .mt(px(4.))
                .px(px(8.))
                .h(px(28.))
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .role(gpui::Role::Switch)
                .aria_label("Sync to a local directory")
                .aria_toggled(if enabled {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .opacity(if sync_busy || !loaded { 0.5 } else { 1. })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child("Sync to a local directory"),
                )
                .child(crate::settings::widgets::toggle_switch(
                    &theme,
                    enabled,
                    "cloud-sync",
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !sync_busy && loaded {
                        this.change_sync(methods::CLOUD_SYNC_CONFIGURE, !enabled, None, cx);
                    }
                })),
        );
        if enabled {
            menu = menu.child(
                div()
                    .px(px(8.))
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(theme.text_muted)
                    .child(dot(phase.1))
                    .child(if sync_busy {
                        "Updating sync…"
                    } else {
                        phase.0
                    }),
            );
            if let Some(path) = sync["localPath"].as_str().map(str::to_owned) {
                let reveal = path.clone();
                menu = menu.child(
                    split_field(&theme)
                        .child(
                            split_main(&theme, "cloud-sync-reveal")
                                .tip("Open folder")
                                .child(
                                    icon(icons::FOLDER)
                                        .size(px(14.))
                                        .flex_none()
                                        .text_color(theme.text_muted),
                                )
                                .child(div().min_w_0().truncate().child(path))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.reveal_path(std::path::Path::new(&reveal))
                                })),
                        )
                        .child(
                            split_chevron(&theme, "cloud-sync-open-in")
                                .aria_label("Open synced folder in")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.computer.page = Page::OpenIn;
                                    cx.notify();
                                })),
                        ),
                );
            } else {
                menu = menu.child(
                    popover::menu_row(&theme, false, "cloud-sync-folder")
                        .id("cloud-sync-folder")
                        .opacity(if sync_busy { 0.5 } else { 1. })
                        .text_size(crate::typography::text_xs())
                        .child(
                            icon(icons::FOLDER)
                                .size(px(14.))
                                .text_color(theme.text_muted),
                        )
                        .child("Choose folder…")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !sync_busy {
                                this.choose_sync_folder(cx);
                            }
                        })),
                );
            }
            if sync["state"] == "error" {
                menu = menu.child(
                    popover::menu_row(&theme, false, "cloud-sync-retry")
                        .id("cloud-sync-retry")
                        .text_size(crate::typography::text_xs())
                        .child(
                            icon(icons::REFRESH)
                                .size(px(14.))
                                .text_color(theme.text_muted),
                        )
                        .child("Retry sync")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_sync(methods::CLOUD_SYNC_RETRY, true, None, cx)
                        })),
                );
            }
        }
        if let Some(error) = self
            .computer
            .sync_error
            .clone()
            .or_else(|| sync["error"].as_str().map(str::to_owned))
        {
            menu = menu.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(theme.danger)
                    .child(error),
            );
        }

        // Repair and removal.
        let mut repair = Vec::new();
        for action in [
            Action::RetryRestart,
            Action::RetryRecreate,
            Action::Continue,
            Action::Discard,
            Action::Restart,
            Action::Recreate,
            Action::Delete,
        ] {
            if !busy && self.computer.pending.is_none() && allowed(&row, action) {
                let danger = action == Action::Delete;
                repair.push(
                    popover::menu_row(&theme, false, format!("computer-{action:?}"))
                        .id(SharedString::from(format!("computer-{action:?}")))
                        .when(danger, |row| row.text_color(theme.danger))
                        .child(
                            icon(match action {
                                Action::Delete => icons::TRASH_BIN_MINIMALISTIC,
                                Action::Recreate | Action::RetryRecreate => icons::REFRESH,
                                _ => icons::RESTART,
                            })
                            .size(px(14.))
                            .text_color(if danger {
                                theme.danger
                            } else {
                                theme.text_muted
                            }),
                        )
                        .child(action.label())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.computer.confirm = Some(action);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        for action in [Action::Restart, Action::Recreate] {
            let operation = &row[action.summary()];
            if operation["operationId"].is_string() && !busy {
                let phase = operation["state"].as_str().unwrap_or("checking");
                repair.push(
                    popover::menu_row(&theme, false, format!("check-{action:?}"))
                        .id(SharedString::from(format!("check-{action:?}")))
                        .child(
                            icon(icons::REFRESH)
                                .size(px(14.))
                                .text_color(theme.text_muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .child(format!("Check {}", action.label().to_lowercase())),
                        )
                        .child(div().text_color(theme.text_muted).child(phase.to_owned()))
                        .on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.computer_action(action, true, cx)
                            }),
                        )
                        .into_any_element(),
                );
            }
        }
        if let Some(pending) = self.computer.pending.clone().filter(|_| !busy) {
            repair.push(
                popover::menu_row(&theme, false, "computer-retry-original")
                    .id("computer-retry-original")
                    .child(
                        icon(icons::REFRESH)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child("Retry original request")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.computer_action(pending.action, false, cx)
                    }))
                    .into_any_element(),
            );
        }
        repair.push(
            popover::menu_row(&theme, false, "computer-refresh")
                .id("computer-refresh")
                .child(
                    icon(icons::REFRESH)
                        .size(px(14.))
                        .text_color(theme.text_muted),
                )
                .child("Refresh status")
                .on_click(cx.listener(|this, _, _, cx| this.refresh_computer(cx)))
                .into_any_element(),
        );
        if self.computer.page == Page::OpenIn {
            return self.render_open_in_page(cx);
        }
        if self.computer.page == Page::Ssh {
            return self.render_ssh_page(cx);
        }
        if self.computer.page == Page::More {
            // The "More" page replaces the summary page: a way back, then
            // the actions, then whatever they report.
            menu = div()
                .id("native-computer-more")
                .flex()
                .flex_col()
                .text_size(crate::typography::text_xs())
                .child(
                    popover::menu_row(&theme, false, "computer-more-back")
                        .id("computer-more-back")
                        .child(
                            icon(icons::ALT_ARROW_LEFT)
                                .size(px(14.))
                                .text_color(theme.text_muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_color(theme.text_muted)
                                .child("Computer"),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.computer.page = Page::Summary;
                            this.computer.confirm = None;
                            cx.notify();
                        })),
                )
                .child(popover::menu_separator())
                .children(more_rows)
                .child(popover::menu_separator())
                .children(repair);
        }
        if busy {
            menu = menu.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(theme.text_muted)
                    .child("Request in progress…"),
            );
        }
        if let Some(error) = &self.computer.error {
            menu = menu.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_color(theme.danger)
                    .child(error.clone()),
            );
        }
        if let Some(action) = self.computer.confirm {
            menu = menu.child(
                div().p(px(8.)).child(
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
                menu = menu.child(popover::menu_separator()).child(
                    div()
                        .id("computer-setup-output")
                        .mx(px(8.))
                        .my(px(4.))
                        .max_h(px(140.))
                        .overflow_y_scroll()
                        .font_family("Geist Mono")
                        .text_size(px(11.))
                        .text_color(theme.text_muted)
                        .child(output),
                );
            }
        }
        menu.into_any_element()
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
    }
}

#[cfg(test)]
mod wake_label_tests {
    use super::{SshAction, SshEntry, ssh_entries};
    use serde_json::json;
    #[test]
    fn ssh_list_shows_detected_apps_then_copy() {
        use crate::shell::ssh_open::{SshApp, SshTarget};
        let ids = |targets: &[SshTarget]| -> Vec<&'static str> {
            ssh_entries(targets).iter().map(SshEntry::id).collect()
        };
        // Before detection: the system terminal, so the menu still works.
        assert_eq!(ids(&[]), ["terminal", "copy"]);
        let targets = [
            SshTarget::for_test(SshApp::Cursor),
            SshTarget::for_test(SshApp::Zed),
            SshTarget::for_test(SshApp::Ghostty),
        ];
        assert_eq!(ids(&targets), ["cursor", "zed", "ghostty", "copy"]);
        assert_eq!(
            ssh_entries(&targets)[0].action(),
            SshAction::Open(SshTarget::for_test(SshApp::Cursor))
        );
        assert_eq!(SshEntry::Copy.label(), "Copy SSH command");
    }

    #[test]
    fn last_wake_says_whether_memory_was_kept() {
        assert_eq!(
            super::last_wake_label(&json!({"lastWake":{"fromMemory":true,"at":1,"ms":1234}})),
            Some("Woke from memory · 1.2 s".into())
        );
        assert_eq!(
            super::last_wake_label(&json!({"lastWake":{"fromMemory":false,"at":1,"ms":6400}})),
            Some("Restarted on wake · 6.4 s".into())
        );
        // An older runtime reports no wake kind; say nothing rather than guess.
        assert_eq!(
            super::last_wake_label(&json!({"lastWake":{"fromMemory":null,"at":1,"ms":900}})),
            None
        );
        assert_eq!(super::last_wake_label(&json!({})), None);
    }
}
