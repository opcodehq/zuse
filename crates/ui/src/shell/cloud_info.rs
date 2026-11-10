//! Cloud environment details and host-owned local sync, matching the reference top-bar menu.
use super::*;
use crate::localization::message as tr;
use serde_json::{Value, json};
use zuse_i18n::messages as msg;

#[derive(Default)]
pub(super) struct CloudInfo {
    menu: popover::Popup<()>,
    target: Option<(String, String)>,
    status: Value,
    metadata: Value,
    error: Option<String>,
    poll: Option<Task<()>>,
    mutation: Option<Task<()>>,
    folder: Option<Task<()>>,
    revision: u64,
}
impl Shell {
    fn cloud_info_target(&self, cx: &App) -> Option<(String, String)> {
        let state = self.state.read(cx);
        Some((
            state.cloud_user_id()?.to_owned(),
            state.engine()?.cloud_workspace()?,
        ))
    }
    fn close_cloud_info(&mut self, cx: &mut Context<Self>) {
        self.cloud_info.poll = None;
        if self.cloud_info.menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.cloud_info.menu);
        }
        cx.notify();
    }
    fn open_cloud_info(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.cloud_info_target(cx) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        self.cloud_info.menu.open(());
        self.cloud_info.error = None;
        self.cloud_info.poll = Some(cx.spawn(async move |this, cx| {
            let mut first = true;
            loop {
                let ready = this
                    .update(cx, |this, cx| {
                        this.cloud_info_target(cx).as_ref() == Some(&target)
                            && this.cloud_info.menu.get().is_some()
                            && this.cloud_info.mutation.is_none()
                    })
                    .unwrap_or(false);
                if ready {
                    let revision = this
                        .update(cx, |this, _| this.cloud_info.revision)
                        .unwrap_or(0);
                    let root = engine.clone();
                    let body = json!({"expectedAccountId":target.0,"workspaceId":target.1});
                    let metadata = first;
                    let job = Tokio::spawn(cx, async move {
                        let status = root
                            .client()
                            .call_scoped(methods::CLOUD_SYNC_STATUS, body.clone())
                            .await?;
                        let row = if metadata {
                            let mut row = root
                                .client()
                                .call_scoped(methods::CLOUD_WORKSPACE, body)
                                .await?;
                            if let Ok(context) = root
                                .client()
                                .call_scoped(methods::CLOUD_CONTEXT, json!({}))
                                .await
                            {
                                if let Some(project) =
                                    context["projects"].as_array().and_then(|rows| {
                                        rows.iter().find(|p| {
                                            p["projectId"] == row["workspace"]["projectId"]
                                        })
                                    })
                                {
                                    row["workspace"]["repositoryDisplayName"] =
                                        project["displayName"].clone();
                                }
                            }
                            Some(row)
                        } else {
                            None
                        };
                        Ok::<_, zuse_rpc::RpcError>((status, row))
                    });
                    let result = job.await;
                    if !this
                        .update(cx, |this, cx| {
                            if this.cloud_info_target(cx).as_ref() != Some(&target) {
                                return false;
                            }
                            if this.cloud_info.mutation.is_some()
                                || this.cloud_info.revision != revision
                            {
                                return true;
                            }
                            match result {
                                Ok(Ok((value, row)))
                                    if value["accountId"].as_str() == Some(&target.0) =>
                                {
                                    this.cloud_info.status = value["status"].clone();
                                    if let Some(row) = row {
                                        this.cloud_info.metadata = row["workspace"].clone();
                                    }
                                }
                                Ok(Err(error)) => this.cloud_info.error = Some(error.to_string()),
                                _ => {
                                    this.cloud_info.error =
                                        Some("Could not read cloud sync status.".into())
                                }
                            }
                            cx.notify();
                            true
                        })
                        .unwrap_or(false)
                    {
                        return;
                    }
                    first = false;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }));
        cx.notify();
    }
    fn change_cloud_sync(
        &mut self,
        method: &'static str,
        enabled: bool,
        path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_info.mutation.is_some() {
            return;
        }
        let Some(target) = self.cloud_info_target(cx) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().map(|e| e.controller()) else {
            return;
        };
        let body = json!({"expectedAccountId":target.0,"workspaceId":target.1,"enabled":enabled,"localPath":path});
        let job = Tokio::spawn(cx, async move {
            engine.client().call_scoped(method, body).await
        });
        self.cloud_info.error = None;
        self.cloud_info.revision += 1;
        self.cloud_info.mutation = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                if this.cloud_info_target(cx).as_ref() != Some(&target) {
                    return;
                }
                this.cloud_info.mutation = None;
                match result {
                    Ok(Ok(value)) if value["accountId"].as_str() == Some(&target.0) => {
                        this.cloud_info.status = value["status"].clone()
                    }
                    Ok(Err(error)) => this.cloud_info.error = Some(error.to_string()),
                    _ => {
                        this.cloud_info.error =
                            Some("Could not update cloud sync. Retry to check its state.".into())
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn choose_cloud_sync_folder(&mut self, cx: &mut Context<Self>) {
        if self.cloud_info.mutation.is_some() || self.cloud_info.folder.is_some() {
            return;
        }
        let target = self.cloud_info_target(cx);
        let receiver = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose Cloud Sync Folder".into()),
        });
        self.cloud_info.folder = Some(cx.spawn(async move |this, cx| {
            let result = receiver.await;
            let _ = this.update(cx, |this, cx| {
                if this.cloud_info_target(cx) != target {
                    return;
                }
                this.cloud_info.folder = None;
                match result {
                    Ok(Ok(Some(paths))) => {
                        if let Some(path) = paths.first() {
                            this.change_cloud_sync(
                                methods::CLOUD_SYNC_CONFIGURE,
                                true,
                                Some(path.to_string_lossy().into_owned()),
                                cx,
                            );
                        }
                    }
                    Ok(Err(error)) => this.cloud_info.error = Some(error.to_string()),
                    _ => {}
                }
                cx.notify();
            });
        }));
    }
    pub(super) fn render_cloud_info(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if let Some(conversation) = self.cloud_conversation.clone() {
            // The menu can open the synced folder in this computer's apps.
            self.ensure_open_targets(cx);
            let targets = self.open_in.targets.clone();
            return Some(conversation.update(cx, |view, cx| {
                view.set_open_targets(targets);
                view.render_header_control(cx)
            }));
        }
        let target = self.cloud_info_target(cx)?;
        if self.cloud_info.target.as_ref() != Some(&target) {
            self.cloud_info = CloudInfo {
                target: Some(target),
                ..Default::default()
            };
        }
        let open = self.cloud_info.menu.get().is_some();
        let mut button = header_icon_button(
            "cloud-workspace-details",
            icons::DITHER_CLOUD,
            "Cloud workspace details",
            theme,
            cx.listener(|this, _, _, cx| {
                if this.cloud_info.menu.take_press_was_open() {
                    this.close_cloud_info(cx);
                } else {
                    this.open_cloud_info(cx);
                }
            }),
        )
        .role(gpui::Role::Button)
        .aria_label(tr(cx, msg::native::cloud_workspace_details))
        .aria_expanded(open)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, _| {
                window.prevent_default();
                this.cloud_info.menu.note_trigger_press();
            }),
        );
        if open {
            let card = self.render_cloud_info_menu(theme, cx);
            button = button.relative().child(popover::anchored_menu_below_end(
                "cloud-workspace-menu",
                card,
                self.cloud_info.menu.closing_since(),
            ));
        }
        Some(button.into_any_element())
    }
    fn render_cloud_info_menu(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let theme = theme.for_popup();
        let enabled = self.cloud_info.status["enabled"] == true;
        let busy = self.cloud_info.mutation.is_some() || self.cloud_info.folder.is_some();
        let loaded = self.cloud_info.status["enabled"].is_boolean();
        let state = self.cloud_info.status["state"]
            .as_str()
            .unwrap_or("loading");
        let phase = match state {
            "synced" => "Up to date",
            "syncing" => "Syncing…",
            "waiting" => "Waiting for cloud connection",
            "error" => "Sync failed",
            "pending" => "Preparing sync…",
            "stopping" => "Stopping sync…",
            "idle" => "Sync off",
            _ => "Loading…",
        };
        let path = self.cloud_info.status["localPath"]
            .as_str()
            .map(str::to_owned);
        let error = self
            .cloud_info
            .error
            .clone()
            .or_else(|| self.cloud_info.status["error"].as_str().map(str::to_owned));
        let row = |label: &'static str, value: String| {
            div()
                .px(px(8.))
                .py(px(7.))
                .flex()
                .gap(px(12.))
                .child(label)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_right()
                        .truncate()
                        .text_color(theme.text_muted)
                        .child(value),
                )
        };
        let metadata = &self.cloud_info.metadata;
        let repo = metadata["repositoryDisplayName"]
            .as_str()
            .or_else(|| metadata["repositoryUrl"].as_str())
            .unwrap_or("Cloud repository")
            .to_owned();
        let environment = metadata["providerId"]
            .as_str()
            .unwrap_or("Cloud")
            .to_owned();
        let status = match metadata["state"].as_str() {
            Some("ready") => "Running",
            Some("paused") => "Paused",
            Some("pausing") => "Pausing…",
            Some("failed") => "Failed",
            _ => "Starting…",
        }
        .to_owned();
        let created = metadata["createdAt"]
            .as_i64()
            .and_then(chrono::DateTime::from_timestamp_millis)
            .map(|time| time.format("%b %-d, %H:%M UTC").to_string())
            .unwrap_or_else(|| "Loading…".into());
        let mut card = popover::popover_card(&theme)
            .w(px(320.))
            .flex()
            .flex_col()
            .text_size(crate::typography::text_xs())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_cloud_info(cx)))
            .child(row("Repository", repo))
            .child(row("Environment", environment))
            .child(row("Status", status))
            .child(row("Created", created))
            .child(popover::menu_separator())
            .child(
                popover::menu_row(&theme, false, "pause-cloud-workspace")
                    .id("pause-cloud-workspace")
                    .child(icon(icons::DITHER_CLOUD).size(px(14.)))
                    .child(tr(cx, msg::native::pause_workspace))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_cloud_info(cx);
                        let composer = this.composer.clone();
                        window.defer(cx, move |window, cx| {
                            composer.update(cx, |composer, cx| composer.pause_cloud(window, cx));
                        });
                    })),
            )
            .child(popover::menu_separator())
            .child(
                popover::menu_row(&theme, false, "cloud-sync-toggle")
                    .id("cloud-sync-toggle")
                    .role(gpui::Role::Switch)
                    .aria_label(tr(
                        cx,
                        msg::connections::cloud_workspace_info_sync_to_a_local_directory,
                    ))
                    .aria_toggled(if enabled {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .opacity(if busy || !loaded { 0.5 } else { 1. })
                    .child(div().flex_1().child(tr(
                        cx,
                        msg::connections::cloud_workspace_info_sync_to_a_local_directory,
                    )))
                    .child(crate::settings::widgets::toggle_switch(
                        &theme,
                        enabled,
                        "cloud-sync",
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !busy && loaded {
                            this.change_cloud_sync(
                                methods::CLOUD_SYNC_CONFIGURE,
                                !enabled,
                                None,
                                cx,
                            );
                        }
                    })),
            )
            .child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_color(theme.text_muted)
                    .child(if busy { "Updating sync…" } else { phase }),
            );
        if let Some(path) = path {
            let copy = path.clone();
            card = card.child(
                popover::menu_row(&theme, false, "cloud-sync-path")
                    .id("cloud-sync-path")
                    .aria_label(tr(cx, msg::native::copy_local_sync_folder))
                    .child(icon(icons::FOLDER).size(px(14.)))
                    .child(div().min_w_0().flex_1().truncate().child(path))
                    .child(icon(icons::COPY).size(px(14.)))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()))
                    })),
            );
        }
        card = card.child(
            popover::menu_row(&theme, false, "cloud-sync-folder")
                .id("cloud-sync-folder")
                .child(tr(cx, msg::native::choose_folder))
                .opacity(if busy { 0.5 } else { 1. })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !busy {
                        this.choose_cloud_sync_folder(cx);
                    }
                })),
        );
        if enabled || error.is_some() {
            card = card.child(
                popover::menu_row(&theme, false, "cloud-sync-retry")
                    .id("cloud-sync-retry")
                    .child(tr(cx, msg::native::retry_sync))
                    .opacity(if busy { 0.5 } else { 1. })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !busy {
                            this.change_cloud_sync(methods::CLOUD_SYNC_RETRY, enabled, None, cx);
                        }
                    })),
            );
        }
        if let Some(error) = error {
            card = card.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_color(theme.danger)
                    .child(error),
            );
        }
        card.into_any_element()
    }
}
