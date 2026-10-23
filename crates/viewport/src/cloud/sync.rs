//! Local mirror controls operate on the host authority, never the guest filesystem.
use super::*;
use crate::shell::{RAISED, TEXT, icon_button};
#[derive(Default)]
pub(super) struct SyncUi {
    open: bool,
    pressed_while_open: bool,
    dismissed_at: Option<std::time::Instant>,
    status: Value,
    pub(super) request: Option<u64>,
    pub(super) metadata: Option<u64>,
    changing: bool,
    error: Option<String>,
    poll: Option<Task<()>>,
}
impl WorkspaceView {
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
        let body = json!({"workspaceId":target["workspaceId"],"expectedAccountId":self.cloud.account,"enabled":enabled,"localPath":path});
        self.cloud.sync.changing = method != methods::CLOUD_SYNC_STATUS;
        if method != methods::CLOUD_SYNC_STATUS {
            self.cloud.sync.error = None;
        }
        self.cloud.sync.request = Some(self.call(method, body));
    }
    pub(super) fn receive_cloud_sync(&mut self, frame: &Value) -> bool {
        if let Some(id) = self.cloud.sync.metadata {
            if frame["id"].as_u64() == Some(id) {
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
                        self.cloud.target = Some(value["workspace"].clone());
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
            self.request_cloud_sync(methods::CLOUD_SYNC_STATUS, false, None);
            if self.cloud.sync.metadata.is_none() {
                if let Some(target) = &self.cloud.target {
                    let body = json!({"workspaceId":target["workspaceId"],"expectedAccountId":self.cloud.account});
                    self.cloud.sync.metadata = Some(self.call(methods::CLOUD_WORKSPACE, body));
                }
            }
            self.cloud.sync.poll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(2))
                        .await;
                    if !this
                        .update(cx, |this, _| {
                            if !this.cloud.sync.open {
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
        } else {
            self.cloud.sync.poll = None;
        }
        cx.notify();
    }
    pub(super) fn cloud_details_button(&self, cx: &mut Context<Self>) -> AnyElement {
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
            "Cloud workspace details",
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
                    this.cloud.sync.dismissed_at = Some(std::time::Instant::now());
                    this.cloud.sync.open = false;
                    this.cloud.sync.poll = None;
                    cx.notify();
                }))
                .child(item("Repository", repo))
                .child(item(
                    "Environment",
                    target["providerId"].as_str().unwrap_or("Cloud").into(),
                ))
                .child(item(
                    "Status",
                    target["state"].as_str().unwrap_or("Cloud").into(),
                ))
                .child(item("Created", created))
                .child(div().h(px(1.)).w_full().bg(rgb(BORDER)))
                .child(
                    control("cloud-sync-toggle", "")
                        .w_full()
                        .min_h(px(32.))
                        .role(Role::Switch)
                        .aria_label("Sync to a local directory")
                        .aria_toggled(if enabled {
                            Toggled::True
                        } else {
                            Toggled::False
                        })
                        .opacity(if busy || !loaded { 0.5 } else { 1. })
                        .child(label("Sync to a local directory").flex_1())
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
                    label("Folder on connected host")
                        .px_2()
                        .text_size(px(11.))
                        .text_color(rgb(MUTED)),
                )
                .child(self.cloud_sync_path.clone())
                .child(
                    control("cloud-sync-use-folder", "Use folder")
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
                menu = menu.child(
                    control("cloud-sync-retry", "Retry sync").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.request_cloud_sync(methods::CLOUD_SYNC_RETRY, true, None);
                            cx.notify();
                        },
                    )),
                );
            }
            if self.cloud.status["state"] == "ready" {
                menu = menu.child(div().h(px(1.)).my_1().bg(rgb(BORDER)))
                    .child(control("pause-cloud-workspace", "Pause workspace").w_full()
                        .on_click(cx.listener(|this, _, _, cx| {
                            let child = this.cloud.child.clone();
                            if let Some(child) = child {
                                let session = child.update(cx, |view, cx| {
                                    if view.file_navigation_blocked(cx) || view.queue_edit_navigation_blocked(cx) { return None; }
                                    view.stash_draft(cx);
                                    Some(view.selected.as_ref().and_then(|chat|chat["id"].as_str()).map(str::to_owned))
                                });
                                if let Some(session) = session {
                                    this.cloud.sync.open = false;
                                    this.cloud.sync.poll = None;
                                    this.pause_cloud(session);
                                    cx.notify();
                                }
                            }
                        })));
            }
            if let Some(error) = self
                .cloud
                .sync
                .error
                .as_deref()
                .or_else(|| self.cloud.sync.status["error"].as_str())
            {
                menu = menu.child(
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
