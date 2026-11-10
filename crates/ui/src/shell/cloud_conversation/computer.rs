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
    pub provider: Option<String>,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SshAction {
    Terminal,
    Copy,
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
    ssh: Option<super::ssh::Prepared>,
    ssh_task: Option<Task<()>>,
    ssh_error: Option<String>,
    /// The apps this computer can open a folder in (the shell detects them).
    open_targets: Vec<crate::shell::open_in::OpenTarget>,
    /// The machine's last CPU, memory and disk snapshot, and the poll that
    /// keeps it current while the summary shows it.
    pub(super) resources: Value,
    resources_poll: Option<Task<()>>,
    resources_wanted: Option<std::time::Instant>,
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
        let provider = row["providerId"].as_str().map(|id| match id {
            "boxd" => "Boxd".to_owned(),
            "e2b" => "E2B".to_owned(),
            "daytona" => "Daytona".to_owned(),
            other => {
                let mut chars = other.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            }
        });
        ComputerSummary {
            label,
            tone,
            sleeping: state == "paused",
            repository,
            provider,
            branch: self.branch_label(),
            size: row["computerSizeId"]
                .as_str()
                .or_else(|| self.config["sizeId"].as_str())
                .filter(|size| !size.is_empty())
                .map(|size| {
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
    /// The apps the synced folder can open in, as the shell detected them.
    pub(in crate::shell) fn set_open_targets(
        &mut self,
        targets: Vec<crate::shell::open_in::OpenTarget>,
    ) {
        self.computer.open_targets = targets;
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
        self.computer.ssh_task = Some(cx.spawn(async move |this, cx| {
            let result: Result<super::ssh::Prepared, String> = async {
                let key = cx
                    .background_executor()
                    .spawn(async { super::ssh::ensure_key() })
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
                let host_workspace = workspace.clone();
                cx.background_executor()
                    .spawn(async move { super::ssh::install_host(&host_workspace, &user, port) })
                    .await?;
                Ok(super::ssh::Prepared {
                    host: super::ssh::host_alias(&workspace),
                    path,
                })
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.computer.ssh_task = None;
                match result {
                    Ok(prepared) => {
                        this.computer.ssh = Some(prepared.clone());
                        match action {
                            SshAction::Copy => {
                                cx.write_to_clipboard(ClipboardItem::new_string(format!(
                                    "ssh {}",
                                    prepared.host
                                )));
                                this.close_computer(cx);
                            }
                            SshAction::Terminal => {
                                let command = super::ssh::terminal_command(&prepared);
                                cx.background_executor()
                                    .spawn(async move { super::ssh::open_terminal(&command) })
                                    .detach();
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
    /// SSH: open a terminal on the computer, or copy the command for any
    /// terminal or editor (the host is in ~/.ssh/config once prepared).
    fn render_ssh_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let busy = self.computer.ssh_task.is_some();
        let host = super::ssh::host_alias(&self.target.workspace);
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
            .child(
                popover::menu_row(&theme, false, "computer-ssh-terminal")
                    .id("computer-ssh-terminal")
                    .opacity(if busy { 0.5 } else { 1. })
                    .child(icon(icons::TERMINAL).size(px(14.)).text_color(theme.text_muted))
                    .child("Open in Terminal")
                    .on_click(cx.listener(|this, _, _, cx| this.open_ssh(SshAction::Terminal, cx))),
            )
            .child(
                popover::menu_row(&theme, false, "computer-ssh-copy")
                    .id("computer-ssh-copy")
                    .opacity(if busy { 0.5 } else { 1. })
                    .child(icon(icons::COPY).size(px(14.)).text_color(theme.text_muted))
                    .child("Copy SSH command")
                    .on_click(cx.listener(|this, _, _, cx| this.open_ssh(SshAction::Copy, cx))),
            )
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
    /// The synced folder's apps: each opens it there, then copy its path.
    fn render_open_in_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let path = self.synced_folder();
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
            .into_any_element()
    }
    pub(super) fn render_computer(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.watch_sync(cx);
        let theme = Theme::of(cx).for_popup();
        let row = self.computer.metadata.clone();
        let busy = self.operation.is_some();
        let summary = self.computer_summary();
        let detail = |label: &'static str, glyph: &'static str, value: AnyElement| {
            div()
                .px(px(8.))
                .h(px(28.))
                .flex()
                .items_center()
                .gap(px(8.))
                .child(icon(glyph).size(px(13.)).text_color(theme.text_muted))
                .child(div().flex_none().child(label))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .justify_end()
                        .text_color(theme.text_muted)
                        .child(value),
                )
        };
        let text = |value: String| div().truncate().child(value).into_any_element();
        let status = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .size(px(6.))
                    .rounded_full()
                    .bg(summary.tone.color(&theme)),
            )
            .child(summary.label)
            .into_any_element();
        let mut menu = div()
            .id("native-computer-panel")
            .flex()
            .flex_col()
            .text_size(crate::typography::text_xs())
            .child(detail(
                "Repository",
                icons::FOLDER,
                text(summary.repository.clone().unwrap_or_else(|| "—".into())),
            ))
            .when_some(summary.branch.clone(), |menu, branch| {
                menu.child(detail("Branch", icons::GIT_BRANCH, text(branch)))
            })
            .child(detail(
                "Environment",
                icons::DITHER_CLOUD,
                text(summary.provider.clone().unwrap_or_else(|| "Cloud".into())),
            ))
            .child(detail("Status", icons::MONITOR, status))
            .when_some(summary.last_wake.clone(), |menu, wake| {
                menu.child(detail("Last wake", icons::CLOCK_CIRCLE, text(wake)))
            })
            .when_some(summary.created.clone(), |menu, created| {
                menu.child(detail("Created", icons::CALENDAR, text(created)))
            });

        // Setup progress, only while a stage is still open.
        let steps = preparation_steps(&row);
        if steps
            .iter()
            .any(|step| step.state != zuse_viewport::CloudPreparationStepState::Completed)
        {
            menu = menu.child(popover::menu_separator());
            for step in steps {
                use zuse_viewport::CloudPreparationStepState as Step;
                let glyph: AnyElement = match step.state {
                    Step::Running => crate::loaders::mini_mono_spinner(
                        format!("native-preparation-{}", step.key),
                        2.,
                        theme.text_muted,
                        cx.entity_id(),
                        cx,
                    )
                    .into_any_element(),
                    Step::Completed => icon(icons::CHECK)
                        .size(px(12.))
                        .text_color(theme.success)
                        .into_any_element(),
                    Step::Failed => icon(icons::CLOSE)
                        .size(px(12.))
                        .text_color(theme.danger)
                        .into_any_element(),
                    _ => div()
                        .size(px(5.))
                        .rounded_full()
                        .bg(theme.text_muted.opacity(0.5))
                        .into_any_element(),
                };
                menu = menu.child(
                    div()
                        .px(px(8.))
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .size(px(13.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(glyph),
                        )
                        .child(div().flex_1().child(step.title))
                        .child(div().text_color(theme.text_muted).child(step.state.label())),
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

        // SSH, while the computer runs.
        let running = matches!(summary.tone, Tone::Running);
        menu = menu.child(popover::menu_separator()).child(
            popover::menu_row(&theme, false, "computer-ssh")
                .id("computer-ssh")
                .opacity(if running { 1. } else { 0.45 })
                .child(
                    icon(icons::TERMINAL)
                        .size(px(14.))
                        .text_color(theme.text_muted),
                )
                .child(div().flex_1().child("Open via SSH"))
                .child(
                    icon(icons::ALT_ARROW_RIGHT)
                        .size(px(12.))
                        .text_color(theme.text_muted),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if running {
                        this.computer.page = Page::Ssh;
                        cx.notify();
                    }
                })),
        );

        // Local sync.
        let sync = &self.computer.sync;
        let loaded = sync["enabled"].is_boolean();
        let enabled = sync["enabled"] == true;
        let sync_busy = self.computer.sync_change.is_some() || self.computer.sync_folder.is_some();
        let phase = match sync["state"].as_str() {
            Some("synced") => ("Up to date", Tone::Running),
            Some("syncing") => ("Syncing…", Tone::Busy),
            Some("waiting") => ("Waiting for the computer", Tone::Idle),
            Some("error") => ("Sync failed", Tone::Failed),
            Some("pending") => ("Preparing sync…", Tone::Busy),
            Some("stopping") => ("Stopping sync…", Tone::Busy),
            _ => ("Sync off", Tone::Idle),
        };
        menu = menu.child(popover::menu_separator()).child(
            popover::menu_row(&theme, false, "cloud-sync-toggle")
                .id("cloud-sync-toggle")
                .role(gpui::Role::Switch)
                .aria_label("Sync to a local folder")
                .aria_toggled(if enabled {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .opacity(if sync_busy || !loaded { 0.5 } else { 1. })
                .child(
                    icon(icons::REFRESH)
                        .size(px(14.))
                        .text_color(theme.text_muted),
                )
                .child(div().flex_1().child("Sync to a local folder"))
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
                    .child(div().size(px(6.)).rounded_full().bg(phase.1.color(&theme)))
                    .child(if sync_busy {
                        "Updating sync…"
                    } else {
                        phase.0
                    }),
            );
            if let Some(path) = sync["localPath"].as_str().map(str::to_owned) {
                let reveal = path.clone();
                let copy = path.clone();
                menu = menu.child(
                    div()
                        .mx(px(8.))
                        .my(px(4.))
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.muted_fill().opacity(0.4))
                        .font_family("Geist Mono")
                        .child(
                            div()
                                .id("cloud-sync-reveal")
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .cursor_pointer()
                                .hover(|el| el.bg(theme.element_hover))
                                .tip("Open folder")
                                .child(
                                    icon(icons::FOLDER_OPEN)
                                        .size(px(13.))
                                        .text_color(theme.text_muted),
                                )
                                .child(div().min_w_0().truncate().child(path))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.reveal_path(std::path::Path::new(&reveal))
                                })),
                        )
                        .child(
                            div()
                                .id("cloud-sync-copy")
                                .h_full()
                                .w(px(28.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .border_l_1()
                                .border_color(theme.border)
                                .cursor_pointer()
                                .hover(|el| el.bg(theme.element_hover))
                                .tip("Copy path")
                                .child(icon(icons::COPY).size(px(13.)).text_color(theme.text_muted))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                                })),
                        ),
                );
            }
            if self.synced_folder().is_some() {
                menu = menu.child(
                    popover::menu_row(&theme, false, "cloud-sync-open-in")
                        .id("cloud-sync-open-in")
                        .child(
                            icon(icons::FOLDER_OPEN)
                                .size(px(14.))
                                .text_color(theme.text_muted),
                        )
                        .child(div().flex_1().child("Open synced folder in…"))
                        .child(
                            icon(icons::ALT_ARROW_RIGHT)
                                .size(px(12.))
                                .text_color(theme.text_muted),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.computer.page = Page::OpenIn;
                            cx.notify();
                        })),
                );
            }
            menu = menu.child(
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
            );
            if sync["state"] == "error" {
                menu = menu.child(
                    popover::menu_row(&theme, false, "cloud-sync-retry")
                        .id("cloud-sync-retry")
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
        } else {
            menu = menu.child(popover::menu_separator()).child(
                popover::menu_row(&theme, false, "computer-more")
                    .id("computer-more")
                    .child(
                        icon(icons::MORE_HORIZONTAL)
                            .size(px(14.))
                            .text_color(theme.text_muted),
                    )
                    .child(div().flex_1().child("More"))
                    .child(
                        icon(icons::ALT_ARROW_RIGHT)
                            .size(px(12.))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.computer.page = Page::More;
                        cx.notify();
                    })),
            );
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

#[cfg(test)]
mod wake_label_tests {
    use serde_json::json;
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
