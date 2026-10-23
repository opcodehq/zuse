//! Reference account-image readiness and build history for both GPUI targets.
use super::*;
gpui::actions!(cloud_image_dialog, [CancelImageDelete, ConfirmImageDelete]);
pub(super) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CancelImageDelete, Some("CloudImageDialog")),
        KeyBinding::new("enter", ConfirmImageDelete, Some("CloudImageDialog")),
        KeyBinding::new("space", ConfirmImageDelete, Some("CloudImageDialog")),
    ]);
}
use zuse_protocol::cloud_projects::{CloudAccountImage, CloudImageBuild, ImageBuildMode};
#[derive(Default)]
pub(super) struct ImageView {
    value: Option<CloudAccountImage>,
    confirm_delete: Option<String>,
    confirm_focus: Option<[FocusHandle; 3]>,
    delete_focus: Option<FocusHandle>,
    error: Option<String>,
    build_key: Option<(String, String)>,
    poll: Option<Task<()>>,
    failures: u8,
    expanded: BTreeSet<String>,
    settings: BTreeSet<String>,
    count: usize,
}
impl ImageView {
    pub(super) fn interrupted(&mut self) {
        self.poll = None;
        self.failures = 3;
        self.error =
            Some("Account connection interrupted. Refresh to check the image build.".into());
    }
}
fn phase(value: Option<&str>) -> &'static str {
    match value.unwrap_or("queued") {
        "allocate" => "Starting the image builder…",
        "prepare" => "Preparing repositories and agents…",
        "capture" | "sanitizing" => "Preparing the snapshot…",
        "verify" => "Verifying the saved image…",
        "cleanup" => "Cleaning up temporary machines…",
        "promote" => "Making the image available…",
        "retire-images" => "Removing the superseded image…",
        _ => "Waiting for the image builder…",
    }
}
fn running(state: &str) -> bool {
    matches!(state, "queued" | "building" | "sanitizing")
}
fn duration(build: &CloudImageBuild) -> String {
    let seconds = (build.updated_at - build.created_at).max(0) / 1000;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}
fn build_title(build: &CloudImageBuild) -> &'static str {
    match build.mode {
        ImageBuildMode::Update => "Image update",
        ImageBuildMode::Rebuild => "Clean rebuild",
    }
}
impl RepositoryPanel {
    fn image_pending(&self) -> bool {
        self.pending
            .values()
            .any(|p| matches!(p.kind, "image" | "image-build" | "image-delete"))
    }
    pub(super) fn refresh_image(&mut self, cx: &mut Context<Self>) {
        if self.owner.is_none() || self.image_pending() {
            return;
        }
        self.image.poll = None;
        self.call("image", methods::CLOUD_IMAGE, json!({}), cx);
        cx.notify();
    }
    fn build_image(&mut self, mode: &str, cx: &mut Context<Self>) {
        if self.image_pending()
            || !self.can_manage()
            || self.projects.is_empty()
            || self
                .image
                .value
                .as_ref()
                .is_none_or(|v| v.state == "building" || v.state == "auth-broken")
        {
            return;
        }
        let (mode, key) = self
            .image
            .build_key
            .get_or_insert_with(|| (mode.to_owned(), uuid::Uuid::new_v4().to_string()))
            .clone();
        self.image.error = None;
        self.image.poll = None;
        self.call(
            "image-build",
            methods::CLOUD_IMAGE_BUILD,
            json!({"mode":mode,"providerId":"boxd","idempotencyKey":key}),
            cx,
        );
        cx.notify();
    }
    pub(super) fn receive_image(
        &mut self,
        pending: Pending,
        result: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        let parsed = result.and_then(|value| {
            if value["accountId"].as_str() != Some(pending.owner.0.as_str())
                || value["organizationId"].as_str() != pending.owner.1.as_deref()
            {
                return Err("Your account changed. Refresh cloud images.".into());
            }
            serde_json::from_value::<CloudAccountImage>(value["data"].clone())
                .map_err(|_| "Image status could not be verified. Refresh and try again.".into())
        });
        match parsed {
            Ok(image) => {
                self.image.failures = 0;
                self.image.error = None;
                if pending.kind == "image-build" {
                    self.image.build_key = None;
                }
                // Keep an explicitly opened attempt stable while status polling changes timestamps.
                self.image
                    .expanded
                    .retain(|id| image.builds.iter().any(|b| &b.build_id == id));
                self.image
                    .settings
                    .retain(|id| image.builds.iter().any(|b| &b.build_id == id));
                self.image.value = Some(image);
            }
            Err(error) => {
                self.image.failures = self.image.failures.saturating_add(1);
                self.image.error = Some(error);
            }
        }
        if self.image.value.as_ref().is_some_and(|v| {
            v.state == "building" || v.storage.as_ref().is_some_and(|s| s.state == "deleting")
        }) && self.image.failures < 3
        {
            self.image.poll = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.image.poll = None;
                    this.refresh_image(cx);
                });
            }));
        }
        cx.notify();
    }
    pub(super) fn render_image(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let busy = self.image_pending();
        let starting = self.pending.values().any(|p| p.kind == "image-build");
        let image = self.image.value.as_ref();
        let state = image.map(|v| v.state.as_str()).unwrap_or("unknown");
        let building = starting || state == "building";
        let (title, description) = if building {
            (
                "Building cloud image".to_owned(),
                if starting {
                    "Starting the image build…".into()
                } else {
                    phase(image.and_then(|v| v.progress_phase.as_deref())).into()
                },
            )
        } else {
            match state {
                "ready" => (
                    "Cloud image ready".into(),
                    "Agent authentication is delivered when a workspace starts.".into(),
                ),
                "failed" => (
                    "Cloud image build failed".into(),
                    format!(
                        "{}{}",
                        if image.is_some_and(|v| v.generation.is_some()) {
                            "The previous image remains available. "
                        } else {
                            ""
                        },
                        image
                            .and_then(|v| v.error_code.as_deref())
                            .unwrap_or("Open the latest build for details.")
                    ),
                ),
                "not-built" => (
                    "Cloud image required".into(),
                    if self.projects.is_empty() {
                        "Add a repository before building your first cloud image.".into()
                    } else {
                        format!("{} repositories ready to include.", self.projects.len())
                    },
                ),
                "outdated" => (
                    "Unbuilt changes".into(),
                    "Repositories or the runtime changed. Update the cloud image.".into(),
                ),
                "auth-broken" => (
                    "Rebuild required".into(),
                    "Reconnect the affected agent before rebuilding the image.".into(),
                ),
                _ => (
                    if self.image.error.is_some() {
                        "Image status unavailable".into()
                    } else {
                        "Checking cloud image…".into()
                    },
                    "Checking the saved image for this account.".into(),
                ),
            }
        };
        let mut readiness = s::setting_row(true)
            .items_center()
            .child(s::details(title, description));
        if state == "ready" && !starting {
            readiness = readiness
                .child(s::description_text("Ready").text_color(crate::palette::rgb(0x8ccca4)));
        }
        let enabled = !busy
            && !building
            && state != "unknown"
            && state != "auth-broken"
            && self.can_manage()
            && !self.projects.is_empty();
        let action = if self.image.build_key.is_some() {
            "Retry request"
        } else {
            match state {
                "ready" => "Rebuild",
                "not-built" => "Build image",
                "failed" => "Retry build",
                _ => "Update image",
            }
        };
        let mode = if state == "ready" {
            "rebuild"
        } else {
            "update"
        };
        if !building && state != "unknown" {
            readiness = readiness.child(
                s::button("build-cloud-image", action)
                    .when(!enabled, |b| b.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if enabled {
                            this.build_image(mode, cx);
                        }
                    })),
            );
        }
        let mut view = s::frame()
            .child(
                row()
                    .justify_between()
                    .child(s::header(
                        "Cloud image",
                        "A private image with your selected repositories and installed agents.",
                    ))
                    .child(
                        s::button("refresh-cloud-image", "Refresh")
                            .when(busy, |b| b.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.image.failures = 0;
                                this.refresh_image(cx);
                            })),
                    ),
            )
            .child(s::card().child(readiness));
        if let Some(error) = &self.image.error {
            view = view.child(div().px_3().py_2().text_size(px(11.)).child(error.clone()));
        }
        if let Some(storage) = self.image.value.as_ref().and_then(|i| i.storage.clone()) {
            let deleting = storage.state == "deleting";
            let description = if deleting {
                "The saved image is being removed. Existing workspaces are preserved.".to_owned()
            } else if storage.billing_enabled {
                format!(
                    "{} per month for the retained image.",
                    zuse_model::cloud_billing::usd(storage.monthly_cost_micros)
                )
            } else {
                "The image is retained. Storage charges are not enabled.".into()
            };
            let focus = self
                .image
                .delete_focus
                .get_or_insert_with(|| cx.focus_handle())
                .clone();
            let id = storage.snapshot_id.clone();
            let enabled = !busy && !building && !deleting && self.can_manage();
            let mut row = s::setting_row(true).items_center().child(s::details(
                if deleting {
                    "Deleting saved image…"
                } else {
                    "Saved image storage"
                },
                description,
            ));
            if !deleting {
                row = row.child(
                    s::button("delete-cloud-image", "Delete image")
                        .track_focus(&focus)
                        .when(!enabled, |b| b.opacity(0.5))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if !enabled {
                                return;
                            }
                            let focus = [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()];
                            window.focus(&focus[1], cx);
                            this.image.confirm_focus = Some(focus);
                            this.image.confirm_delete = Some(id.clone());
                            cx.notify();
                        })),
                );
            }
            if let Some(error) = storage.last_error {
                row = row.child(
                    s::description_text(format!("Cleanup will retry: {error}"))
                        .text_color(crate::palette::rgb(0xe5a0a0)),
                );
            }
            view = view.child(div().mt_2().child(s::card().child(row)));
        }
        if let Some(image) = self.image.value.clone() {
            let count = self.image.count.max(5);
            if !image.builds.is_empty() {
                let mut history =
                    s::card().child(div().px_3().py_2().child(s::title_text("Build history")));
                for build in image.builds.iter().take(count) {
                    history = history.child(self.render_build(build, cx));
                }
                if image.builds.len() > count {
                    history = history.child(
                        s::button(
                            "more-cloud-image-builds",
                            format!("Show {} more", (image.builds.len() - count).min(5)),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.image.count = this.image.count.max(5) + 5;
                            cx.notify();
                        })),
                    );
                }
                view = view.child(div().mt_2().child(history));
            }
        }
        if let Some(id) = self.image.confirm_delete.clone() {
            if let Some(focus) = self.image.confirm_focus.clone() {
                let card=s::card().id("image-delete-dialog").w(px(420.)).p_5().role(Role::Dialog).aria_label("Delete saved image?")
                    .child(div().text_size(px(18.)).child("Delete saved image?"))
                    .child(s::description_text("This removes the saved cloud image and stops its storage charges. Existing workspaces, conversations and files are preserved. Build another image before starting new workspaces.").mt_3())
                    .child(row().mt_4().gap_3().justify_end()
                        .child(s::button("cancel-image-delete","Cancel").track_focus(&focus[1]).tab_index(0).on_click(cx.listener(|this,_,window,cx|this.dismiss_image_delete(window,cx))))
                        .child(s::button("confirm-image-delete","Delete image").track_focus(&focus[2]).tab_index(0).text_color(crate::palette::rgb(0xe5a0a0)).on_click(cx.listener(move|this,_,window,cx|{this.dismiss_image_delete(window,cx);this.delete_image(&id,cx);}))))
                    ;
                let overlay = div()
                    .w(window.viewport_size().width)
                    .h(window.viewport_size().height)
                    .occlude()
                    .bg(rgba(0x00000099))
                    .flex()
                    .items_center()
                    .justify_center()
                    .track_focus(&focus[0])
                    .key_context("CloudImageDialog")
                    .on_action(cx.listener(|this, _: &CancelImageDelete, window, cx| {
                        this.dismiss_image_delete(window, cx)
                    }))
                    .on_action(cx.listener(|this, _: &ConfirmImageDelete, window, cx| {
                        let confirmed = this
                            .image
                            .confirm_focus
                            .as_ref()
                            .is_some_and(|f| f[2].is_focused(window));
                        let id = this.image.confirm_delete.clone();
                        this.dismiss_image_delete(window, cx);
                        if confirmed {
                            if let Some(id) = id {
                                this.delete_image(&id, cx);
                            }
                        }
                    }))
                    .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => this.dismiss_image_delete(window, cx),
                            "tab" => {
                                if let Some(f) = &this.image.confirm_focus {
                                    window
                                        .focus(&f[if f[1].is_focused(window) { 2 } else { 1 }], cx);
                                }
                            }
                            "enter" | "space" => {
                                let confirmed = this
                                    .image
                                    .confirm_focus
                                    .as_ref()
                                    .is_some_and(|f| f[2].is_focused(window));
                                let id = this.image.confirm_delete.clone();
                                this.dismiss_image_delete(window, cx);
                                if confirmed {
                                    if let Some(id) = id {
                                        this.delete_image(&id, cx);
                                    }
                                }
                            }
                            _ => {}
                        }
                        cx.stop_propagation();
                    }))
                    .child(card);
                view = view.child(deferred(
                    anchored()
                        .anchor(Anchor::TopLeft)
                        .position(point(px(0.), px(0.)))
                        .child(overlay),
                ));
            }
        }
        view
    }
    fn dismiss_image_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image.confirm_delete = None;
        if let Some(f) = &self.image.delete_focus {
            window.focus(f, cx);
        }
        cx.notify();
    }
    fn delete_image(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.image_pending()
            || !self.can_manage()
            || self
                .image
                .value
                .as_ref()
                .and_then(|i| i.storage.as_ref())
                .is_none_or(|s| s.snapshot_id != id)
        {
            return;
        }
        self.image.error = None;
        self.image.poll = None;
        self.call(
            "image-delete",
            methods::CLOUD_IMAGE_DELETE,
            json!({"snapshotId":id}),
            cx,
        );
        cx.notify();
    }
    fn render_build(&self, build: &CloudImageBuild, cx: &mut Context<Self>) -> Div {
        let id = build.build_id.clone();
        let open = self.image.expanded.contains(&id);
        let settings = self.image.settings.contains(&id);
        let (status, color) = match build.state.as_str() {
            "ready" => ("Succeeded", 0x8ccca4),
            "failed" => ("Failed", 0xe5a0a0),
            _ => ("In progress", 0xd5bd7a),
        };
        let timestamp = chrono_date(build.created_at);
        let mut header = s::button(
            SharedString::from(format!("cloud-image-build-{id}")),
            build_title(build),
        )
        .w_full()
        .justify_start()
        .px_3()
        .rounded_none()
        .child(div().flex_1())
        .child(s::description_text(format!(
            "{timestamp} · {}",
            duration(build)
        )))
        .when(build.active, |v| {
            v.child(s::description_text("Active").text_color(crate::palette::rgb(0x8ccca4)))
        })
        .child(s::description_text(status).text_color(crate::palette::rgb(color)))
        .child(crate::assets::icon(if open {
            "alt-arrow-down"
        } else {
            "alt-arrow-right"
        }));
        header = header.on_click(cx.listener(move |this, _, _, cx| {
            if !this.image.expanded.remove(&id) {
                this.image.expanded.insert(id.clone());
            }
            cx.notify();
        }));
        let mut view = column()
            .border_t_1()
            .border_color(s::card_border())
            .child(header);
        if open {
            let logs = build
                .log_text
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| {
                    if build.state == "failed" {
                        format!(
                            "Logs were not retained for this build.\nError: {}",
                            build.error_code.as_deref().unwrap_or("unknown")
                        )
                    } else if running(&build.state) {
                        phase(build.progress_phase.as_deref()).into()
                    } else {
                        "Build completed. No output was retained.".into()
                    }
                });
            let copy = logs.clone();
            let id = build.build_id.clone();
            let mut details = column().px_3().py_2().gap_2();
            if running(&build.state) {
                details =
                    details.child(s::description_text(phase(build.progress_phase.as_deref())));
            }
            if let Some(error) = &build.error_code {
                details = details.child(
                    s::description_text(error.clone()).text_color(crate::palette::rgb(0xe5a0a0)),
                );
            }
            details = details.child(
                column()
                    .rounded_md()
                    .bg(s::frame_bg())
                    .p_2()
                    .child(
                        row().justify_end().child(
                            s::button(
                                SharedString::from(format!("copy-image-logs-{id}")),
                                "Copy logs",
                            )
                            .on_click(cx.listener(
                                move |_, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                                },
                            )),
                        ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("image-build-log-{id}")))
                            .max_h(px(288.))
                            .overflow_y_scroll()
                            .text_size(px(10.))
                            .line_height(px(16.))
                            .child(logs),
                    ),
            );
            details = details.child(
                s::button(
                    SharedString::from(format!("image-build-settings-{id}")),
                    if settings {
                        "Hide build settings"
                    } else {
                        "Build settings"
                    },
                )
                .justify_start()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.image.settings.remove(&id) {
                        this.image.settings.insert(id.clone());
                    }
                    cx.notify();
                })),
            );
            if settings {
                details = details.child(s::description_text(format!(
                    "Runtime {}",
                    build.runtime_version
                )));
                for repository in &build.repositories {
                    details = details.child(s::description_text(format!(
                        "{} · {}",
                        repository.repository_identity.replace("github.com/", ""),
                        repository.default_branch
                    )));
                }
            }
            view = view.child(details);
        }
        view
    }
}
fn chrono_date(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at)
        .map(|date| date.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "Unknown date".into())
}
