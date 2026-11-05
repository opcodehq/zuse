//! Local mirror controls operate on the host authority, never the guest filesystem.
use super::*;
use crate::localization::message as tr;
use crate::shell::{RAISED, TEXT, icon_button};
use zuse_i18n::messages as msg;
#[derive(Clone, PartialEq, Eq)]
struct PreparationLogScope {
    account: String,
    organization: Option<String>,
    namespace: Option<String>,
    workspace: String,
    child: EntityId,
}
fn preparation_changed(before: &Value, after: &Value) -> bool {
    ["state", "statusCode", "preparation", "startupPhase"]
        .iter()
        .any(|key| before[*key] != after[*key])
}
#[derive(Default)]
pub(super) struct SyncUi {
    pub(super) open: bool,
    pressed_while_open: bool,
    dismissed_at: Option<web_time::Instant>,
    status: Value,
    pub(super) request: Option<u64>,
    pub(super) metadata: Option<u64>,
    pub(super) delete: Option<u64>,
    pub(super) log_request: Option<u64>,
    pub(super) logs: Value,
    pub(super) log_error: Option<String>,
    log_refresh_pending: bool,
    log_scope: Option<PreparationLogScope>,
    delete_command: Option<String>,
    confirm_delete: bool,
    changing: bool,
    error: Option<String>,
    pub(super) poll: Option<Task<()>>,
}
impl WorkspaceView {
    fn computer_controls(&self, cx: &App) -> super::computer_controls::Controls {
        let child = self.cloud.child.as_ref().map(|child| child.read(cx));
        super::computer_controls::available(
            self.cloud.target.as_ref().unwrap_or(&Value::Null),
            self.cloud.status["state"] == "ready",
            child
                .as_ref()
                .is_some_and(|view| view.cloud_server_commands()),
            child.as_ref().is_some_and(|view| view.selected.is_some()),
            self.cloud.pause.is_some() || self.cloud.sync.delete.is_some(),
        )
    }
    fn computer_menu_state(&self, cx: &App) -> super::computer_menu::State {
        let controls = self.computer_controls(cx);
        let target = self.cloud.target.as_ref().unwrap_or(&Value::Null);
        let state = target["state"].as_str().unwrap_or_default();
        let operation = self.cloud.pause.is_some() || self.cloud.sync.delete.is_some();
        let connected = self.cloud.status["state"] == "ready"
            && self
                .cloud
                .child
                .as_ref()
                .is_some_and(|child| child.read(cx).connected && !child.read(cx).cached_only);
        let working = self
            .cloud
            .child
            .as_ref()
            .is_some_and(|child| child.read(cx).is_working());
        let preparing = matches!(state, "queued" | "provisioning" | "setup" | "starting");
        let stop = controls.stop && (working || preparing);
        super::computer_menu::State {
            tools_ready: connected && !operation,
            tools_reason: if operation {
                "A computer operation is in progress."
            } else if matches!(state, "paused" | "stopped") {
                "Resume this computer to use its tools."
            } else if matches!(state, "deleting" | "deleted" | "archived") {
                "This computer is no longer available."
            } else if preparing {
                "The computer is starting."
            } else if !connected {
                "Reconnecting to this computer…"
            } else {
                ""
            },
            stop,
            stop_reason: if stop {
                "Stops this conversation and pauses its queue."
            } else if !super::computer_controls::editable(target) {
                "Edit access is required to stop the agent."
            } else if operation {
                "Wait for the current computer operation."
            } else if !working {
                "Agent is not running."
            } else {
                "Reconnect to verify the current agent."
            },
            sleep: controls.sleep,
            resume: controls.resume,
            delete: controls.delete,
        }
    }
    fn perform_computer_menu_action(
        &mut self,
        action: super::computer_menu::Action,
        cx: &mut Context<Self>,
    ) {
        use super::computer_menu::Action;
        // Recheck live eligibility: a retained pointer handler may predate a connection transition.
        let state = self.computer_menu_state(cx);
        match action {
            Action::Terminal | Action::Files | Action::Previews => {
                if !state.tools_ready {
                    return;
                }
                let Some(child) = self.cloud.child.clone() else {
                    return;
                };
                child.update(cx, |view, cx| {
                    view.right_open = true;
                    view.right_tab = match action {
                        Action::Terminal => "terminal",
                        Action::Files => "files",
                        _ => "browser",
                    }
                    .into();
                    cx.notify();
                });
                self.cloud.sync.open = false;
            }
            Action::Stop => {
                if !state.stop {
                    return;
                }
                if let Some(child) = &self.cloud.child {
                    child.update(cx, |view, cx| view.interrupt(cx));
                }
            }
            Action::Sleep => {
                if !state.sleep {
                    return;
                }
                if let Some(child) = self.cloud.child.clone() {
                    let session = child.update(cx, |view, cx| {
                        if view.file_navigation_blocked(cx)
                            || view.queue_edit_navigation_blocked(cx)
                        {
                            return None;
                        }
                        view.stash_draft(cx);
                        Some(
                            view.selected
                                .as_ref()
                                .and_then(|chat| chat["id"].as_str())
                                .map(str::to_owned),
                        )
                    });
                    if let Some(session) = session {
                        self.cloud.sync.open = false;
                        self.cloud.sync.poll = None;
                        self.pause_cloud(session);
                    }
                }
            }
            Action::Resume => {
                if !state.resume {
                    return;
                }
                self.connect_cloud();
                self.cloud.sync.open = false;
            }
            Action::Delete => {
                if !state.delete {
                    return;
                }
                self.cloud.sync.confirm_delete = true;
            }
        }
        cx.notify();
    }
    pub(super) fn refresh_computer_metadata(&mut self) {
        if self.cloud.sync.metadata.is_some() {
            return;
        }
        if let Some(target) = &self.cloud.target {
            self.cloud.sync.error = None;
            let body = json!({"workspaceId":target["workspaceId"],"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization});
            self.cloud.sync.metadata = Some(self.call(methods::CLOUD_WORKSPACE, body));
        }
    }
    fn preparation_log_scope(&self) -> Option<PreparationLogScope> {
        Some(PreparationLogScope {
            account: self.cloud.account.clone()?,
            organization: self.cloud.organization.clone(),
            namespace: self.cloud.namespace.clone(),
            workspace: self.cloud.target.as_ref()?["workspaceId"]
                .as_str()?
                .to_owned(),
            child: self.cloud.child.as_ref()?.entity_id(),
        })
    }
    pub(super) fn request_cloud_preparation_logs(&mut self, cx: &App) {
        if !self
            .cloud
            .child
            .as_ref()
            .is_some_and(|child| child.read(cx).cloud.preparation_open)
        {
            self.cloud.sync.log_refresh_pending = false;
            return;
        }
        if self.cloud.sync.log_request.is_some() {
            return;
        }
        let Some(scope) = self.preparation_log_scope() else {
            self.cloud.sync.log_refresh_pending = false;
            return;
        };
        let body = json!({"workspaceId":scope.workspace,"expectedAccountId":scope.account,"expectedOrganizationId":scope.organization,"includePreparationLogs":true});
        self.cloud.sync.log_error = None;
        self.cloud.sync.log_refresh_pending = false;
        self.cloud.sync.log_scope = Some(scope);
        self.cloud.sync.log_request = Some(self.call(methods::CLOUD_WORKSPACE, body));
    }
    pub(super) fn flush_cloud_preparation_logs(&mut self, cx: &App) {
        if self.cloud.sync.log_refresh_pending {
            self.request_cloud_preparation_logs(cx);
        }
    }
    fn request_cloud_sync(&mut self, method: &'static str, enabled: bool, path: Option<String>) {
        if self.cloud.sync.request.is_some() {
            if method == methods::CLOUD_SYNC_STATUS || self.cloud.sync.changing {
                return;
            }
            if let Some(id) = self.cloud.sync.request.take() {
                self.cancel(id);
                self.pending.remove(&id);
            }
        }
        let Some(target) = &self.cloud.target else {
            return;
        };
        let body = json!({"workspaceId":target["workspaceId"],"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization,"enabled":enabled,"localPath":path});
        self.cloud.sync.changing = method != methods::CLOUD_SYNC_STATUS;
        if method != methods::CLOUD_SYNC_STATUS {
            self.cloud.sync.error = None;
        }
        self.cloud.sync.request = Some(self.call(method, body));
    }
    pub(super) fn receive_cloud_sync(&mut self, frame: &Value) -> bool {
        if let Some(id) = self.cloud.sync.log_request {
            if frame["id"].as_u64() == Some(id) {
                let expected = self.cloud.sync.log_scope.clone();
                let same_view = expected.is_some() && expected == self.preparation_log_scope();
                if let Some(value) = frame.get("ok") {
                    if same_view
                        && expected.as_ref().is_some_and(|scope| {
                            value["workspace"]["workspaceId"].as_str()
                                == Some(scope.workspace.as_str())
                                && value["accountId"].as_str() == Some(scope.account.as_str())
                                && value["organizationId"].as_str() == scope.organization.as_deref()
                        })
                    {
                        self.cloud.sync.logs = value["preparationLogs"].clone();
                    } else if same_view {
                        self.cloud.sync.log_error =
                            Some("Preparation output response did not match this computer.".into());
                    }
                }
                if same_view && let Some(error) = frame.get("err") {
                    self.cloud.sync.log_error = Some(
                        error
                            .as_str()
                            .or_else(|| error["message"].as_str())
                            .unwrap_or("Preparation output is unavailable.")
                            .into(),
                    );
                }
                if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true
                {
                    self.pending.remove(&id);
                    self.cloud.sync.log_request = None;
                    self.cloud.sync.log_scope = None;
                    if !same_view {
                        self.cloud.sync.log_refresh_pending = false;
                    }
                }
                return true;
            }
        }
        if let Some(id) = self.cloud.sync.delete {
            if frame["id"].as_u64() == Some(id) {
                if let Some(error) = frame.get("err") {
                    self.cloud.sync.error = Some(error.as_str().or_else(|| error["message"].as_str())
                        .unwrap_or("Deletion was not confirmed. Retry to reconcile the original operation.").into());
                }
                if let Some(value) = frame.get("ok") {
                    if value["workspace"]["workspaceId"]
                        == self
                            .cloud
                            .target
                            .as_ref()
                            .map(|t| t["workspaceId"].clone())
                            .unwrap_or(Value::Null)
                    {
                        self.pending.remove(&id);
                        self.cloud.sync.delete = None;
                        self.close_cloud();
                        self.refresh_cloud();
                        return true;
                    }
                    self.cloud.sync.error = Some("Deletion response did not match this computer. Refresh to check its state.".into());
                }
                if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true
                {
                    self.pending.remove(&id);
                    self.cloud.sync.delete = None;
                }
                return true;
            }
        }
        if let Some(id) = self.cloud.sync.metadata {
            if frame["id"].as_u64() == Some(id) {
                if frame.get("err").is_some() {
                    self.cloud.sync.error=Some("Computer access could not be verified. Refresh its details to retry; no control was sent.".into());
                }
                if let Some(value) = frame.get("ok").or_else(|| frame.get("item")) {
                    if value["workspace"]["workspaceId"]
                        == self
                            .cloud
                            .target
                            .as_ref()
                            .map(|t| &t["workspaceId"])
                            .unwrap_or(&Value::Null)
                            .clone()
                    {
                        self.cloud.sync.log_refresh_pending |=
                            self.cloud.target.as_ref().is_some_and(|before| {
                                preparation_changed(before, &value["workspace"])
                            });
                        self.cloud.target = Some(value["workspace"].clone());
                        self.sync_cloud_repair_metadata();
                        self.sync_cloud_recreation_metadata();
                    }
                }
                if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true
                {
                    self.pending.remove(&id);
                    self.cloud.sync.metadata = None;
                }
                return true;
            }
        }
        let Some(id) = self.cloud.sync.request else {
            return false;
        };
        if frame["id"].as_u64() != Some(id) {
            return false;
        }
        if let Some(value) = frame.get("ok").or_else(|| frame.get("item")) {
            if value["accountId"].as_str() == self.cloud.account.as_deref() {
                self.cloud.sync.status = value["status"].clone();
            }
        }
        if let Some(error) = frame.get("err") {
            self.cloud.sync.error = Some(
                error
                    .as_str()
                    .unwrap_or("Cloud sync request failed.")
                    .into(),
            );
        }
        if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true {
            self.pending.remove(&id);
            self.cloud.sync.request = None;
            self.cloud.sync.changing = false;
        }
        true
    }
    fn toggle_cloud_details(&mut self, cx: &mut Context<Self>) {
        // Outside dismissal runs on mouse-down before the trigger's click.
        // Preserve that dismissal through the same short exit interval as native menus.
        let dismissed = std::mem::take(&mut self.cloud.sync.pressed_while_open);
        self.cloud.sync.dismissed_at = None;
        self.cloud.sync.open = !dismissed && !self.cloud.sync.open;
        if self.cloud.sync.open {
            if self.cloud.organization.is_none() {
                self.request_cloud_sync(methods::CLOUD_SYNC_STATUS, false, None);
            }
            self.refresh_computer_metadata();
            if !self.cloud.account_events.live {
                self.cloud.sync.poll = Some(cx.spawn(async move |this, cx| {
                    for _ in 0..60 {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2))
                            .await;
                        if !this
                            .update(cx, |this, _| {
                                if this.cloud.account_events.live
                                    || !this.cloud.sync.open
                                    || !matches!(
                                        this.cloud.sync.status["state"].as_str(),
                                        Some("pending" | "syncing" | "stopping")
                                    )
                                {
                                    return false;
                                }
                                this.request_cloud_sync(methods::CLOUD_SYNC_STATUS, false, None);
                                true
                            })
                            .unwrap_or(false)
                        {
                            return;
                        }
                    }
                }));
            }
        } else {
            self.cloud.sync.poll = None;
        }
        cx.notify();
    }
    pub(super) fn cloud_details_button(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.cloud.sync.changing;
        let enabled = self.cloud.sync.status["enabled"] == true;
        let loaded = self.cloud.sync.status["enabled"].is_boolean();
        let phase = match self.cloud.sync.status["state"].as_str() {
            Some("synced") => "Up to date",
            Some("syncing") => "Syncing…",
            Some("waiting") => "Waiting for cloud connection",
            Some("error") => "Sync failed",
            Some("pending") => "Preparing sync…",
            Some("stopping") => "Stopping sync…",
            Some("idle") => "Sync off",
            _ => "Loading…",
        };
        let target = self.cloud.target.as_ref().unwrap_or(&Value::Null);
        let preparation_issue =
            super::preparation::issue(target["statusCode"].as_str().unwrap_or_default());
        let controls = self.computer_controls(cx);
        let item = |name: &'static str, value: String| {
            row()
                .w_full()
                .min_h(px(30.))
                .gap_2()
                .child(label(name))
                .child(div().flex_1())
                .child(label(value).truncate().text_color(rgb(MUTED)))
        };
        let repo = self
            .cloud
            .projects
            .iter()
            .find(|p| p["projectId"] == target["projectId"])
            .and_then(|p| p["displayName"].as_str())
            .or_else(|| target["repositoryDisplayName"].as_str())
            .unwrap_or("Cloud repository")
            .to_owned();
        let created = target["createdAt"]
            .as_i64()
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|time| time.format("%b %-d, %H:%M UTC").to_string())
            .unwrap_or_default();
        let button = icon_button(
            "cloud-workspace-details",
            "cloud-dither",
            crate::localization::text("native-cloud_workspace_details", cx),
        )
        .aria_expanded(self.cloud.sync.open)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _, _| {
                this.cloud.sync.pressed_while_open = this.cloud.sync.open
                    || this
                        .cloud
                        .sync
                        .dismissed_at
                        .is_some_and(|at| at.elapsed() < std::time::Duration::from_millis(150));
            }),
        )
        .on_click(cx.listener(|this, _, _, cx| this.toggle_cloud_details(cx)));
        let mut view = row().relative().child(button);
        if self.cloud.sync.open {
            let mut menu = column()
                .id("cloud-workspace-menu")
                .absolute()
                .top_full()
                .right_0()
                .mt_2()
                .w(px(320.))
                .max_h((window.viewport_size().height - px(64.)).max(px(160.)))
                .overflow_y_scroll()
                .p_2()
                .gap_1()
                .rounded(px(10.))
                .bg(rgb(RAISED))
                .border_1()
                .border_color(rgb(BORDER))
                .shadow_lg()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.cloud.sync.dismissed_at = Some(web_time::Instant::now());
                    this.cloud.sync.open = false;
                    this.cloud.sync.poll = None;
                    cx.notify();
                }))
                .child(label("Computer").px_2().py_1())
                .child(super::computer_menu::actions(
                    self.computer_menu_state(cx),
                    {
                        let entity = cx.entity().downgrade();
                        move |action, cx| {
                            let _ = entity.update(cx, |view, cx| {
                                view.perform_computer_menu_action(action, cx)
                            });
                        }
                    },
                ))
                .child(item("Repository", repo))
                .child(item(
                    "Provider",
                    target["providerId"].as_str().unwrap_or("Cloud").into(),
                ))
                .child(item(
                    "Status",
                    target["state"].as_str().unwrap_or("Cloud").into(),
                ))
                .child(item("Created", created))
                .when_some(target["branch"].as_str(), |menu, branch| {
                    menu.child(item("Branch", branch.to_owned()))
                })
                .when_some(target["runtimeState"].as_str(), |menu, runtime| {
                    menu.child(item("Runtime", runtime.to_owned()))
                })
                .when_some(target["environmentVersion"].as_str(), |menu, version| {
                    menu.child(item("Environment", version.to_owned()))
                })
                .when_some(target["size"].as_str(), |menu, size| {
                    menu.child(item("Size", size.to_owned()))
                });
            if let Some(agent) = target["agent"].as_str() {
                let title = match agent {
                    "claude" => "Claude",
                    "codex" => "Codex",
                    "grok" => "Grok",
                    _ => "Coding agent",
                };
                let selected = self.cloud.connections.iter().find(|row| {
                    row["connectionId"] == target["agentConnectionId"] && row["agent"] == agent
                });
                let label = selected
                    .map(|row| {
                        super::agent_caption::selected_label(
                            row["accountLabel"].as_str(),
                            row["connectionScope"].as_str(),
                            agent,
                        )
                    })
                    .unwrap_or_else(|| "Selected account unavailable".into());
                menu = menu
                    .child(item("Coding agent", format!("{title} · {label}")))
                    .child(
                        control("computer-selected-agent", "Review selected account")
                            .w_full()
                            .on_click(
                                cx.listener(|this, _, _, cx| this.open_selected_cloud_agent(cx)),
                            ),
                    );
            }
            if !super::computer_controls::editable(target) {
                menu = menu.child(
                    label(if self.cloud.sync.metadata.is_some() {
                        "Checking computer access…"
                    } else {
                        "Computer controls require verified edit access."
                    })
                    .px_2()
                    .text_size(px(11.)),
                );
            }
            if let Some(issue) = preparation_issue {
                menu = menu
                    .child(label(issue.title).px_2().py_1().text_color(rgb(0xeb8f8f)))
                    .child(label(issue.detail).px_2().text_size(px(11.)));
                if let Some(action) = issue.recovery.label() {
                    menu = menu.child(
                        control("computer-preparation-recovery", action)
                            .w_full()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if issue.recovery == super::preparation::Recovery::Agent {
                                    this.open_selected_cloud_agent(cx);
                                    return;
                                }
                                this.cloud.sync.open = false;
                                this.settings = true;
                                this.select_settings_section(
                                    crate::settings_view::Section::Cloud,
                                    cx,
                                );
                                cx.notify();
                            })),
                    );
                }
            }
            if self.cloud.organization.is_none() {
                menu = menu
                    .child(div().h(px(1.)).w_full().bg(rgb(BORDER)))
                    .child(
                        control("cloud-sync-toggle", "")
                            .w_full()
                            .min_h(px(32.))
                            .role(Role::Switch)
                            .aria_label(tr(cx, msg::connections::cloud_workspace_info_sync_to_a_local_directory))
                            .aria_toggled(if enabled {
                                Toggled::True
                            } else {
                                Toggled::False
                            })
                            .opacity(if busy || !loaded { 0.5 } else { 1. })
                            .child(label(tr(cx, msg::connections::cloud_workspace_info_sync_to_a_local_directory)).flex_1())
                            .child(
                                div()
                                    .w(px(28.))
                                    .h(px(16.))
                                    .rounded_full()
                                    .bg(rgb(if enabled { 0x89ca21 } else { 0x555555 }))
                                    .px(px(2.))
                                    .flex()
                                    .items_center()
                                    .when(enabled, |d| d.justify_end())
                                    .child(div().size(px(12.)).rounded_full().bg(rgb(TEXT))),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !busy && loaded {
                                    this.request_cloud_sync(
                                        methods::CLOUD_SYNC_CONFIGURE,
                                        !enabled,
                                        None,
                                    );
                                    cx.notify();
                                }
                            })),
                    )
                    .child(
                        label(if busy { "Updating sync…" } else { phase })
                            .px_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED)),
                    );
                if let Some(path) = self.cloud.sync.status["localPath"].as_str() {
                    menu = menu.child(
                        label(path.to_owned())
                            .px_2()
                            .py_1()
                            .truncate()
                            .text_size(px(11.)),
                    );
                }
                // Browser directories belong to the connected host, not the browser device.
                menu = menu
                    .child(
                        label(tr(cx, msg::native::folder_on_connected_host))
                            .px_2()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED)),
                    )
                    .child(self.cloud_sync_path.clone())
                    .child(
                        control("cloud-sync-use-folder", tr(cx, msg::native::use_folder))
                            .opacity(if busy { 0.5 } else { 1. })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let path = this.cloud_sync_path.read(cx).value(cx);
                                if !busy && !path.trim().is_empty() {
                                    this.request_cloud_sync(
                                        methods::CLOUD_SYNC_CONFIGURE,
                                        true,
                                        Some(path),
                                    );
                                    cx.notify();
                                }
                            })),
                    );
                if enabled {
                    menu = menu.child(control("cloud-sync-retry", tr(cx, msg::native::retry_sync)).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.request_cloud_sync(methods::CLOUD_SYNC_RETRY, true, None);
                            cx.notify();
                        }),
                    ));
                }
            }
            menu = menu.child(
                control("computer-record-command", "Save to environment setup…")
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        let project = this
                            .cloud
                            .target
                            .as_ref()
                            .and_then(|target| target["projectId"].as_str())
                            .map(str::to_owned);
                        this.settings = true;
                        this.select_settings_section(crate::settings_view::Section::Cloud, cx);
                        this.repository_panel.update(cx, |panel, cx| {
                            panel.record_installation_command(project, cx)
                        });
                        this.cloud.sync.open = false;
                        cx.notify();
                    })),
            );
            if let Some(retry) = self.allocation_retry_controls(cx) {
                menu = menu
                    .child(div().h(px(1.)).my_1().bg(rgb(BORDER)))
                    .child(retry);
            }
            if let Some(recreation) = self.cloud_recreation_controls(cx) {
                menu = menu
                    .child(div().h(px(1.)).my_1().bg(rgb(BORDER)))
                    .child(recreation);
            }
            if let Some(repair) = self.cloud_repair_controls(cx) {
                menu = menu
                    .child(div().h(px(1.)).my_1().bg(rgb(BORDER)))
                    .child(repair);
            }
            menu = menu.child(div().h(px(1.)).my_1().bg(rgb(BORDER)));
            if self.cloud.sync.delete.is_some() {
                menu = menu.child(label("Deleting computer…").px_2().text_size(px(11.)));
            } else if controls.delete && self.cloud.sync.confirm_delete {
                menu = menu.child(label("Permanently delete this computer, its disk, stored cloud history and recovery data? Save any work you need first.").px_2().py_1().text_size(px(11.)))
                    .child(row().gap_2()
                        .child(control("computer-cancel-delete", "Keep computer").on_click(cx.listener(|this, _, _, cx| {
                            this.cloud.sync.confirm_delete = false; cx.notify();
                        })))
                        .child(control("computer-confirm-delete", if self.cloud.sync.delete.is_some() { "Deleting…" } else { "Delete permanently" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.computer_controls(cx).delete { return; }
                                let Some(target) = &this.cloud.target else { return; };
                                let workspace = target["workspaceId"].clone();
                                let command = this.cloud.sync.delete_command.get_or_insert_with(|| uuid::Uuid::new_v4().to_string()).clone();
                                this.cloud.sync.error = None;
                                this.cloud.sync.delete = Some(this.call(methods::CLOUD_DELETE, json!({"workspaceId":workspace,"expectedAccountId":this.cloud.account,"expectedOrganizationId":this.cloud.organization,"commandId":command})));
                                cx.notify();
                            }))));
            }
            if let Some(error) = self
                .cloud
                .sync
                .error
                .as_deref()
                .or_else(|| self.cloud.sync.status["error"].as_str())
            {
                menu = menu
                    .child(
                        control("computer-refresh-details", "Refresh computer details")
                            .w_full()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_computer_metadata();
                                cx.notify();
                            })),
                    )
                    .child(
                        label(error.to_owned())
                            .px_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(rgb(0xeb8f8f)),
                    );
            }
            view = view.child(gpui::deferred(menu));
        }
        view.into_any_element()
    }
}

#[cfg(test)]
mod preparation_log_tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;

    fn workspace(stage: &str) -> Value {
        json!({"workspaceId":"workspace","chatId":"thread","initialSessionId":"session",
            "state":"setup","statusCode":if stage=="failed" {"credential_preparation_failed"}else{"repository-syncing"},
            "startupPhase":"syncing-repository","preparation":{
                "repository":{"state":if stage=="parallel" {"running"}else{"completed"}},
                "credentials":{"state":if stage=="failed" {"failed"}else if stage=="parallel" {"running"}else{"completed"}},
                "environment":{"state":if stage=="environment" {"running"}else if stage=="agent" {"completed"}else{"pending"}},
                "agent":{"state":if stage=="agent" {"running"}else{"pending"}}}})
    }
    fn seed(view: &mut WorkspaceView, window: &mut Window, cx: &mut Context<WorkspaceView>) {
        let (requests, _outgoing) = futures::channel::mpsc::unbounded();
        let (_events, incoming) = futures::channel::mpsc::unbounded();
        let child = cx.new(|cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: incoming,
                },
                window,
                cx,
            )
        });
        child.update(cx, |child, _| child.cloud.preparation_open = true);
        view.cloud.child = Some(child);
        view.cloud.account = Some("owner".into());
        view.cloud.organization = Some("org".into());
        view.cloud.target = Some(workspace("failed"));
        view.cloud.status = json!({"state":"connecting"});
        view.cloud.sync.logs = json!({"log":{"stdout":"","stderr":"Old failure"}});
        // Keep unrelated catalogue reads outside this focused memory transport.
        view.cloud.context = Some(9000);
        view.cloud.list = Some(9001);
        view.cloud.account_events.request = Some(view.call(methods::CLOUD_EVENTS, json!({})));
    }
    fn metadata_event(view: &mut WorkspaceView, stage: &str, cx: &mut Context<WorkspaceView>) {
        let feed = view.cloud.account_events.request.unwrap();
        assert!(view.receive_cloud(&json!({"id":feed,"item":{"accountId":"owner","organizationId":"org",
            "data":{"type":"events","events":[{"kind":"workspace","workspaceId":"workspace","resourceId":"workspace","payload":{}}]}}}),cx));
        let metadata = view
            .cloud
            .sync
            .metadata
            .expect("workspace event must request authoritative metadata");
        assert!(view.receive_cloud(&json!({"id":metadata,"ok":{"accountId":"owner","organizationId":"org","workspace":workspace(stage)}}),cx));
    }
    fn logs(id: u64, stdout: &str, stderr: &str) -> Value {
        json!({"id":id,"ok":{"accountId":"owner","organizationId":"org","workspace":{"workspaceId":"workspace"},
            "preparationLogs":{"log":{"stdout":stdout,"stderr":stderr,"truncated":false}}}})
    }
    fn log_reads(outgoing: &mut futures::channel::mpsc::UnboundedReceiver<Value>) -> Vec<Value> {
        let mut reads = vec![];
        while let Ok(frame) = outgoing.try_recv() {
            if let Some(method) = frame["method"].as_str() {
                assert!(
                    matches!(
                        method,
                        methods::CLOUD_EVENTS
                            | methods::CLOUD_CONTEXT
                            | methods::CLOUD_WORKSPACES
                            | methods::CLOUD_WORKSPACE
                    ),
                    "preparation output refresh must never dispatch execution/lifecycle: {method}"
                );
            }
            if frame["method"] == methods::CLOUD_WORKSPACE
                && frame["params"]["includePreparationLogs"] == true
            {
                reads.push(frame);
            }
        }
        reads
    }
    #[gpui::test]
    fn workspace_events_refresh_open_activity_and_coalesce_latest_log_read(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut outgoing) = futures::channel::mpsc::unbounded();
        let (_events, incoming) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: incoming,
                },
                window,
                cx,
            )
        });
        window
            .update(cx, |view, window, cx| {
                seed(view, window, cx);
                metadata_event(view, "parallel", cx);
                let first = view
                    .cloud
                    .sync
                    .log_request
                    .expect("authoritative stage change must refresh open Activity output");
                let reads = log_reads(&mut outgoing);
                assert_eq!(reads.len(), 1);
                assert_eq!(reads[0]["params"]["expectedAccountId"], "owner");
                assert_eq!(reads[0]["params"]["expectedOrganizationId"], "org");
                metadata_event(view, "environment", cx);
                metadata_event(view, "agent", cx);
                assert_eq!(view.cloud.sync.log_request, Some(first));
                assert!(
                    log_reads(&mut outgoing).is_empty(),
                    "pending log reads coalesce stage updates"
                );
                assert!(
                    view.receive_cloud(&logs(first, "Older startup output", "Old failure"), cx)
                );
                let trailing = view
                    .cloud
                    .sync
                    .log_request
                    .expect("pending stage updates need one trailing refresh");
                assert_ne!(first, trailing);
                assert_eq!(log_reads(&mut outgoing).len(), 1);
                assert!(view.receive_cloud(&logs(trailing, "Latest startup output", ""), cx));
                assert!(view.cloud.sync.log_request.is_none());
                assert!(
                    log_reads(&mut outgoing).is_empty(),
                    "log replies must not refresh themselves"
                );
                let child = view.cloud.child.as_ref().unwrap().read(cx);
                assert_eq!(
                    child.cloud.guest_status["preparationLog"]["stdout"],
                    "Latest startup output"
                );
                assert_eq!(child.cloud.guest_status["preparationLog"]["stderr"], "");
                assert_eq!(
                    view.cloud.status["state"], "connecting",
                    "no CloudEnvironment transition is required"
                );
                metadata_event(view, "agent", cx);
                assert!(
                    view.cloud.sync.log_request.is_none(),
                    "unchanged authoritative preparation does not refetch output"
                );
                assert!(log_reads(&mut outgoing).is_empty());
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.cloud
                        .child
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .cloud
                        .guest_status["preparationLog"]["stdout"],
                    "Latest startup output"
                );
                assert!(log_reads(&mut outgoing).is_empty());
            })
            .unwrap();
    }
    #[gpui::test]
    fn closed_activity_and_changed_scope_prevent_trailing_log_reads(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut outgoing) = futures::channel::mpsc::unbounded();
        let (_events, incoming) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: incoming,
                },
                window,
                cx,
            )
        });
        window
            .update(cx, |view, window, cx| {
                seed(view, window, cx);
                metadata_event(view, "parallel", cx);
                let first = view
                    .cloud
                    .sync
                    .log_request
                    .expect("open Activity must read changed stage output");
                log_reads(&mut outgoing);
                metadata_event(view, "environment", cx);
                log_reads(&mut outgoing);
                view.cloud
                    .child
                    .as_ref()
                    .unwrap()
                    .update(cx, |child, _| child.cloud.preparation_open = false);
                assert!(view.receive_cloud(&logs(first, "No trailing read while closed", ""), cx));
                assert!(view.cloud.sync.log_request.is_none());
                assert!(log_reads(&mut outgoing).is_empty());
                view.cloud
                    .child
                    .as_ref()
                    .unwrap()
                    .update(cx, |child, _| child.cloud.preparation_open = true);
                metadata_event(view, "agent", cx);
                let second = view.cloud.sync.log_request.unwrap();
                log_reads(&mut outgoing);
                let mut wrong_workspace = logs(second, "Wrong-workspace output", "");
                wrong_workspace["ok"]["workspace"]["workspaceId"] = json!("another-workspace");
                assert!(view.receive_cloud(&wrong_workspace, cx));
                assert_ne!(
                    view.cloud.sync.logs["log"]["stdout"],
                    "Wrong-workspace output"
                );
                assert!(view.cloud.sync.log_error.is_some());
                assert!(log_reads(&mut outgoing).is_empty());
                metadata_event(view, "parallel", cx);
                let second = view.cloud.sync.log_request.unwrap();
                log_reads(&mut outgoing);
                metadata_event(view, "environment", cx);
                log_reads(&mut outgoing);
                view.cloud.account = Some("another-owner".into());
                assert!(
                    view.receive_cloud(&logs(second, "Wrong-scope output must be ignored", ""), cx)
                );
                assert_ne!(
                    view.cloud.sync.logs["log"]["stdout"],
                    "Wrong-scope output must be ignored"
                );
                assert!(view.cloud.sync.log_request.is_none());
                assert!(log_reads(&mut outgoing).is_empty());
                view.close_cloud();
                assert!(view.cloud.sync.log_request.is_none());
            })
            .unwrap();
        cx.run_until_parked();
    }
}
