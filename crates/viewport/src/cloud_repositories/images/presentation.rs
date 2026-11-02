//! Image readiness, provider history and retained-image billing views.
use super::*;
#[path = "delete_dialog.rs"]
mod delete_dialog;
fn status(state: &str) -> Div {
    match state {
        "ready" => cs::badge("Ready", false, false),
        "failed" => cs::error_badge("Failed"),
        "auth-broken" => cs::error_badge("Authentication"),
        "building" => cs::badge("Building", true, false),
        "outdated" => cs::badge("Unbuilt changes", true, false),
        _ => cs::badge("Not built", true, false),
    }
}
impl RepositoryPanel {
    fn close_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.image.history_open = false;
        if let Some(f) = &self.image.history_trigger {
            window.focus(f, cx);
        }
        cx.notify();
    }
    fn history_handle(&mut self, key: impl Into<String>, cx: &mut Context<Self>) -> FocusHandle {
        self.history_handle_enabled(key, true, cx)
    }
    fn history_handle_enabled(
        &mut self,
        key: impl Into<String>,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        let handle = self
            .image
            .history_handles
            .entry(key.into())
            .or_insert_with(|| cx.focus_handle())
            .clone();
        if enabled {
            self.image.history_order.push(handle.clone());
        }
        handle
    }
    pub(in crate::cloud_repositories) fn render_image(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let busy = self.image_pending();
        let starting = self.pending.values().any(|p| p.kind == "image-build");
        let image = self.image.value.as_ref();
        let state = image.map(|i| i.state.as_str()).unwrap_or("unknown");
        let building = starting || state == "building";
        let enabled = !busy
            && !building
            && state != "unknown"
            && state != "auth-broken"
            && self.can_manage()
            && !self.projects.is_empty();
        let mut body = column();
        if image.is_none() {
            body = body.child(cs::setting_row(
                true,
                if self.image.error.is_some() {
                    "Image status unavailable"
                } else {
                    "Checking cloud image…"
                },
                "Checking whether this provider needs a build.",
                None,
            ));
        } else if state == "ready" && !starting {
            body=body.child(cs::setting_row(true,"Cloud image ready","Ready for new cloud chats. Agent authentication is delivered when a workspace starts.",Some(row().gap(px(6.)).child(cs::badge("Ready",false,false)).child(cs::button("build-cloud-image","Rebuild",false,!enabled,false).text_size(px(11.)).on_click(cx.listener(move|this,_,_,cx|if enabled{this.build_image("rebuild",cx)}))).into_any_element())));
        } else {
            let failed = matches!(state, "failed" | "auth-broken");
            let (title, description) = if building {
                (
                    "Building cloud image".to_owned(),
                    if starting {
                        "Starting the image build…".to_owned()
                    } else {
                        match image.and_then(|i| i.progress_phase.as_deref()) {
                            Some("queued") => "Waiting for a build worker…".into(),
                            Some("sanitizing") => "Preparing the snapshot…".into(),
                            _ => "Preparing repositories and agents".into(),
                        }
                    },
                )
            } else {
                match state{
                "not-built"=>("Cloud image required".into(),if self.projects.is_empty(){"Add a repository before building your first cloud image.".into()}else{format!("{} {} ready to include.",self.projects.len(),if self.projects.len()==1{"repository is"}else{"repositories are"})}),
                "failed"=>("Cloud image build failed".into(),format!("The previous image remains available. {}",image.and_then(|i|i.error_code.as_deref()).unwrap_or("Open the latest build for details."))),
                "auth-broken"=>("Rebuild required".into(),"Reconnect the affected agent. Image status will refresh automatically and show whether a rebuild is needed.".into()),
                _=>("Unbuilt changes".into(),if image.is_some_and(|i|i.builds.iter().find(|b|b.active).is_some_and(|b| b.repositories.len()!=self.projects.len()||self.projects.iter().any(|p|!b.repositories.iter().any(|r|r.project_id==p.project_id)||i.built_at.is_some_and(|at|p.updated_at>at)))){"Changed: repositories.".into()}else{"Changed: runtime or toolchain.".into()})}
            };
            let label = if self.image.build_key.is_some() {
                "Retry request"
            } else {
                match state {
                    "not-built" => "Build image",
                    "auth-broken" => "Rebuild image",
                    "failed" => "Retry build",
                    _ => "Update image",
                }
            };
            let mode = if state == "auth-broken" {
                "rebuild"
            } else {
                "update"
            };
            let mut row = row()
                .gap(px(8.))
                .px(px(12.))
                .py(px(8.))
                .bg(if failed {
                    palette::tone(
                        hsla(349.3 / 360., 0.311, 0.176, 1.),
                        hsla(0., 1., 0.973, 1.),
                    )
                } else {
                    palette::tone(
                        hsla(20.8 / 360., 0.289, 0.176, 1.),
                        hsla(22.1 / 360., 1., 0.963, 1.),
                    )
                })
                .child(
                    icon(if building {
                        "loading"
                    } else if failed {
                        "close-circle"
                    } else {
                        "danger-triangle"
                    })
                    .size(px(16.))
                    .text_color(if failed {
                        s::destructive_text()
                    } else {
                        hsla(21.6 / 360., 0.982, 0.567, 1.)
                    }),
                )
                .child(
                    column()
                        .flex_1()
                        .min_w_0()
                        .child(s::title_text(title))
                        .child(div().mt(px(2.)).child(s::description_text(description))),
                );
            if !building {
                if state == "auth-broken" {
                    row = row.child(cs::error_badge("Authentication"));
                }
                row = row.child(
                    cs::icon_label_button(
                        "build-cloud-image",
                        "refresh",
                        label,
                        true,
                        !enabled,
                        starting,
                    )
                    .text_size(px(11.))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if enabled {
                            this.build_image(mode, cx)
                        }
                    })),
                );
            }
            body = body.child(row);
        }
        let trigger = self
            .image
            .history_trigger
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        body = body.child(
            div()
                .border_t_1()
                .border_color(palette::rgb(BORDER))
                .px(px(12.))
                .py(px(8.))
                .child(
                    row()
                        .gap(px(4.))
                        .child(
                            row()
                                .flex_1()
                                .min_w_0()
                                .gap(px(8.))
                                .px(px(8.))
                                .h(px(28.))
                                .child(
                                    div()
                                        .size(px(12.))
                                        .rounded_full()
                                        .border_1()
                                        .border_color(palette::rgb(0x89ca21))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(
                                            div()
                                                .size(px(6.))
                                                .rounded_full()
                                                .bg(palette::rgb(0x89ca21)),
                                        ),
                                )
                                .child(
                                    icon("cloud-boxd")
                                        .size(px(16.))
                                        .text_color(palette::rgb(TEXT)),
                                )
                                .child(s::title_text("boxd"))
                                .child(cs::badge("Recommended", false, true))
                                .child(s::description_text("Billed by Zuse")),
                        )
                        .child(
                            div()
                                .id("cloud-image-history-trigger")
                                .role(Role::Button)
                                .aria_label("View boxd build logs")
                                .track_focus(&trigger.clone().tab_stop(true))
                                .tab_index(0)
                                .h(px(28.))
                                .px(px(8.))
                                .rounded(px(7.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .cursor_pointer()
                                .hover(|v| v.bg(s::frame_bg().opacity(0.5)))
                                .focus_visible(|v| {
                                    v.border_1().border_color(palette::rgb(0x89ca21))
                                })
                                .child(s::description_text("Zuse image"))
                                .child(status(state))
                                .child(s::description_text("Logs"))
                                .child(icon("alt-arrow-right").size(px(14.)))
                                .on_key_down(|e: &KeyDownEvent, _, cx| {
                                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                                        cx.stop_propagation();
                                    }
                                })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.image.history_open = true;
                                    this.image.previous_open = false;
                                    this.image.history_page = 0;
                                    if let Some(build) =
                                        this.image.value.as_ref().and_then(|i| i.builds.first())
                                    {
                                        this.image.expanded.insert(build.build_id.clone());
                                    }
                                    let handle = cx.focus_handle();
                                    window.focus(&handle, cx);
                                    this.image.history_focus = Some(handle);
                                    cx.notify();
                                })),
                        ),
                ),
        );
        if let Some(error) = &self.image.error {
            body = body.child(
                row()
                    .px(px(12.))
                    .py(px(8.))
                    .gap(px(12.))
                    .bg(s::destructive_text().opacity(0.1))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(s::destructive_text())
                            .child(error.clone()),
                    )
                    .child(
                        cs::button("refresh-cloud-image", "Retry", false, busy, false).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.image.failures = 0;
                                this.refresh_image(cx);
                            }),
                        ),
                    ),
            )
        }
        let mut view = cs::group(
            "Cloud image",
            Some(
                "Build an image for the selected provider. Existing chats keep their current environment.",
            ),
            None,
            body,
        );
        if self.image.history_open {
            view = view.child(self.render_history(window, cx));
        }
        view
    }
    pub(in crate::cloud_repositories) fn render_storage(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(storage) = self.image.value.as_ref().and_then(|i| i.storage.clone()) else {
            return div();
        };
        let deleting = storage.state == "deleting";
        let building = self
            .image
            .value
            .as_ref()
            .is_some_and(|i| i.state == "building");
        let busy = self.image_pending();
        let enabled = !busy && !building && !deleting && self.can_manage();
        let focus = self
            .image
            .delete_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let id = storage.snapshot_id.clone();
        let mut body=column().child(cs::setting_row(true,"Retained images","Storage uses your $35 cloud allowance. Additional usage has the existing 5% markup and overage cap.",Some(cs::badge("1",false,true).into_any_element())));
        let mut action = row();
        {
            action = action.child(
                cs::button("delete-cloud-image", "Delete image", false, !enabled, false)
                    .track_focus(&focus.clone().tab_stop(enabled))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !enabled {
                            return;
                        }
                        let focus = [
                            cx.focus_handle(),
                            cx.focus_handle().tab_stop(true),
                            cx.focus_handle().tab_stop(true),
                        ];
                        window.focus(&focus[1], cx);
                        this.image.confirm_focus = Some(focus);
                        this.image.confirm_delete = Some(id.clone());
                        cx.notify();
                    })),
            );
        }
        body=body.child(cs::setting_row(false,"boxd",if deleting{"Image deletion pending. Existing workspaces are preserved."}else if storage.billing_enabled{"Deleting this image requires a rebuild for future workspaces. Existing workspace disks and authentication are preserved."}else{"Storage billing has not started. Deleting this image requires a rebuild for future workspaces."},Some(action.into_any_element())));
        body = body.child(cs::setting_row(
            false,
            "Storage usage this period",
            "Included in your cloud usage totals.",
            Some(
                cs::badge(
                    self.storage_usage
                        .map(zuse_model::cloud_billing::usd)
                        .unwrap_or_else(|| "—".into()),
                    false,
                    true,
                )
                .into_any_element(),
            ),
        ));
        if let Some(error) = storage.last_error {
            body = body.child(cs::note(&format!("Cleanup will retry: {error}"), true));
        }
        if let Some(error) = &self.image.error {
            body = body.child(
                row()
                    .px(px(12.))
                    .py(px(8.))
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(s::destructive_text())
                            .child(error.clone()),
                    )
                    .child(
                        cs::button("retry-image-storage-status", "Retry", false, busy, false)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.image.failures = 0;
                                this.refresh_image(cx);
                            })),
                    ),
            )
        }
        let view = s::frame()
            .child(s::header(
                "Image storage",
                format!(
                    "{} per 30 days, prorated while retained.",
                    zuse_model::cloud_billing::usd(storage.monthly_cost_micros)
                ),
            ))
            .child(s::card().child(body));
        self.with_image_delete(view, window, cx)
    }
    fn render_history(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.image.history_order.clear();
        let close = self.history_handle("close", cx);
        let builds = self
            .image
            .value
            .as_ref()
            .map(|i| i.builds.clone())
            .unwrap_or_default();
        let mut body = column()
            .id("cloud-build-history-scroll")
            .max_h(px(
                (f32::from(window.viewport_size().height) - 112.).max(100.)
            ))
            .overflow_y_scroll()
            .px(px(4.))
            .pb(px(8.));
        if let Some(build) = builds.first() {
            body = body.child(self.render_build(build, true, cx));
        } else {
            body = body.child(
                div()
                    .px(px(12.))
                    .py(px(8.))
                    .text_size(px(12.))
                    .text_color(palette::rgb(MUTED))
                    .child("No image builds yet."),
            );
        }
        if builds.len() > 1 {
            let open = self.image.previous_open;
            let handle = self.history_handle("previous", cx);
            body = body.child(
                div()
                    .id("previous-cloud-builds")
                    .role(Role::Button)
                    .aria_label("Previous builds")
                    .aria_expanded(open)
                    .track_focus(&handle.clone().tab_stop(true))
                    .tab_index(0)
                    .h(px(28.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .border_t_1()
                    .border_color(palette::rgb(BORDER))
                    .text_size(px(11.))
                    .text_color(palette::rgb(MUTED))
                    .cursor_pointer()
                    .hover(|v| v.text_color(palette::rgb(TEXT)))
                    .child(
                        icon(if open {
                            "alt-arrow-up"
                        } else {
                            "alt-arrow-down"
                        })
                        .size(px(12.)),
                    )
                    .child(format!("Previous builds {}", builds.len() - 1))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.image.previous_open = !this.image.previous_open;
                        cx.notify();
                    })),
            );
            if open {
                let pages = (builds.len() - 1).div_ceil(5);
                let page = self.image.history_page.min(pages - 1);
                for build in builds.iter().skip(1 + page * 5).take(5) {
                    body = body.child(self.render_build(build, false, cx));
                }
                if pages > 1 {
                    let previous = self.history_handle_enabled("previous-page", page > 0, cx);
                    let next = self.history_handle_enabled("next-page", page + 1 < pages, cx);
                    body = body.child(
                        row()
                            .h(px(36.))
                            .px(px(12.))
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(palette::rgb(MUTED))
                                    .child(format!("Page {} of {pages}", page + 1)),
                            )
                            .child(
                                row()
                                    .gap(px(4.))
                                    .child(
                                        cs::button(
                                            "previous-image-build-page",
                                            "Previous",
                                            false,
                                            page == 0,
                                            false,
                                        )
                                        .track_focus(&previous.clone().tab_stop(page > 0))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.image.history_page =
                                                    this.image.history_page.saturating_sub(1);
                                                cx.notify();
                                            }),
                                        ),
                                    )
                                    .child(
                                        cs::button(
                                            "next-image-build-page",
                                            "Next",
                                            false,
                                            page + 1 >= pages,
                                            false,
                                        )
                                        .track_focus(&next.clone().tab_stop(page + 1 < pages))
                                        .on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.image.history_page = (page + 1).min(pages - 1);
                                                cx.notify();
                                            }),
                                        ),
                                    ),
                            ),
                    );
                }
            }
        }
        let card = column()
            .id("cloud-image-history-dialog")
            .relative()
            .role(Role::Dialog)
            .aria_label("View boxd build logs")
            .w(px(
                576_f32.min(f32::from(window.viewport_size().width) - 32.)
            ))
            .max_h(px(f32::from(window.viewport_size().height) - 32.))
            .rounded(px(12.))
            .when(!palette::is_light(), |v| {
                v.border_1().border_color(white().opacity(0.1))
            })
            .bg(palette::tone(hsla(132. / 360., 0.05, 0.105, 1.), white()))
            .text_color(palette::rgb(TEXT))
            .shadow_lg()
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_history(window, cx)))
            .child(
                row()
                    .items_start()
                    .p(px(16.))
                    .pb(px(10.))
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(15.))
                            .line_height(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("View boxd build logs"),
                    ),
            )
            .child(body)
            .child(
                div().absolute().top(px(10.)).right(px(10.)).child(
                    cs::icon_button("close-cloud-image-history", "close", "Close", false, false)
                        .track_focus(&close.clone().tab_stop(true))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.close_history(window, cx)),
                        ),
                ),
            );
        let root = self.image.history_focus.as_ref().unwrap().clone();
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(point(px(0.), px(0.)))
                .child(
                    div()
                        .w(window.viewport_size().width)
                        .h(window.viewport_size().height)
                        .bg(rgba(0x00000052))
                        .flex()
                        .items_center()
                        .justify_center()
                        .occlude()
                        .track_focus(&root)
                        .key_context("CloudImageHistory")
                        .on_action(cx.listener(|this, _: &CloseImageHistory, window, cx| {
                            this.close_history(window, cx);
                            cx.stop_propagation();
                        }))
                        .capture_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                            match e.keystroke.key.as_str() {
                                "escape" => this.close_history(window, cx),
                                "tab" => {
                                    let handles = &this.image.history_order;
                                    if !handles.is_empty() {
                                        let current =
                                            handles.iter().position(|f| f.is_focused(window));
                                        let next = match current {
                                            Some(i) if e.keystroke.modifiers.shift => {
                                                (i + handles.len() - 1) % handles.len()
                                            }
                                            Some(i) => (i + 1) % handles.len(),
                                            None if e.keystroke.modifiers.shift => {
                                                handles.len() - 1
                                            }
                                            None => 0,
                                        };
                                        window.focus(&handles[next], cx);
                                    }
                                }
                                _ => return,
                            }
                            cx.stop_propagation();
                        }))
                        .child(card),
                ),
        )
        .into_any_element()
    }
    fn render_build(
        &mut self,
        build: &CloudImageBuild,
        first: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = build.build_id.clone();
        let open = self.image.expanded.contains(&id);
        let settings = self.image.settings.contains(&id);
        let handle = self.history_handle(id.clone(), cx);
        let badge = match build.state.as_str() {
            "ready" => cs::badge("Succeeded", false, false),
            "failed" => cs::error_badge("Failed"),
            _ => cs::badge("In progress", true, false),
        };
        let mut header = div()
            .id(SharedString::from(format!("cloud-image-build-{id}")))
            .role(Role::Button)
            .aria_label(SharedString::from(build_title(build)))
            .aria_expanded(open)
            .track_focus(&handle.clone().tab_stop(true))
            .tab_index(0)
            .h(px(28.))
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .hover(|v| v.bg(s::frame_bg().opacity(0.4)))
            .focus_visible(|v| {
                v.border_1()
                    .border_color(palette::rgb(0x89ca21))
                    .px(px(11.))
            })
            .on_key_down(|event: &KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "space" | "enter") {
                    cx.stop_propagation();
                }
            })
            .child(
                icon(if build.state == "ready" {
                    "check-circle"
                } else if build.state == "failed" {
                    "close-circle"
                } else {
                    "loading"
                })
                .size(px(14.)),
            )
            .child(
                s::title_text(build_title(build))
                    .flex_1()
                    .min_w_0()
                    .truncate(),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(palette::rgb(MUTED))
                    .child(format!(
                        "{} · {}",
                        chrono_date(build.created_at),
                        duration(build)
                    )),
            );
        if build.active {
            header = header.child(cs::badge("Active", false, false));
        }
        let toggle = id.clone();
        header = header
            .child(badge)
            .child(
                icon(if open {
                    "alt-arrow-up"
                } else {
                    "alt-arrow-down"
                })
                .size(px(12.)),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if !this.image.expanded.remove(&toggle) {
                    this.image.expanded.insert(toggle.clone());
                }
                cx.notify();
            }));
        let mut view = column()
            .when(!first, |v| {
                v.border_t_1()
                    .border_color(palette::rgb(BORDER).opacity(0.6))
            })
            .child(header);
        if open {
            let logs = build
                .log_text
                .clone()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| {
                    if build.state == "failed" {
                        format!(
                            "Logs were not retained for this older build.\nError: {}",
                            build.error_code.as_deref().unwrap_or("unknown")
                        )
                    } else if running(&build.state) {
                        phase(build.progress_phase.as_deref()).into()
                    } else {
                        "Build completed. No output was retained.".into()
                    }
                });
            let copy = logs.clone();
            let copy_focus = self.history_handle(format!("copy-{id}"), cx);
            let settings_focus = self.history_handle(format!("settings-{id}"), cx);
            let mut details = column().px(px(12.)).py(px(8.)).gap(px(8.));
            details = details
                .child(
                    div()
                        .relative()
                        .rounded(px(7.))
                        .bg(s::frame_bg().opacity(0.3))
                        .p(px(10.))
                        .child(
                            div()
                                .id(SharedString::from(format!("image-build-log-{id}")))
                                .max_h(px(288.))
                                .overflow_y_scroll()
                                .pr(px(32.))
                                .font_family("Geist Mono")
                                .text_size(px(10.))
                                .line_height(px(16.))
                                .text_color(palette::rgb(TEXT).opacity(0.85))
                                .child(logs),
                        )
                        .child(
                            div().absolute().top(px(4.)).right(px(4.)).child(
                                cs::icon_button(
                                    SharedString::from(format!("copy-image-logs-{id}")),
                                    "copy",
                                    "Copy build logs",
                                    false,
                                    false,
                                )
                                .track_focus(&copy_focus.clone().tab_stop(true))
                                .on_click(cx.listener(
                                    move |_, _, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    },
                                )),
                            ),
                        ),
                )
                .child(
                    cs::icon_label_button(
                        SharedString::from(format!("image-build-settings-{id}")),
                        if settings {
                            "alt-arrow-down"
                        } else {
                            "alt-arrow-right"
                        },
                        "Build settings",
                        false,
                        false,
                        false,
                    )
                    .text_size(px(11.))
                    .justify_start()
                    .track_focus(&settings_focus.clone().tab_stop(true))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.image.settings.remove(&id) {
                            this.image.settings.insert(id.clone());
                        }
                        cx.notify();
                    })),
                );
            if settings {
                details = details.child(
                    row()
                        .gap(px(12.))
                        .text_size(px(11.))
                        .child(
                            div()
                                .w(px(112.))
                                .text_color(palette::rgb(MUTED))
                                .child("Runtime"),
                        )
                        .child(build.runtime_version.clone()),
                );
                for repository in &build.repositories {
                    details = details.child(
                        row()
                            .gap(px(8.))
                            .h(px(32.))
                            .child(super::super::presentation::avatar(
                                &repository.display_name,
                                None,
                                24.,
                            ))
                            .child(s::description_text(repository.display_name.clone()).flex_1())
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(palette::rgb(MUTED))
                                    .child(repository.default_branch.clone()),
                            ),
                    );
                }
            }
            view = view.child(details);
        }
        view
    }
}
fn chrono_date(at: i64) -> String {
    chrono::DateTime::from_timestamp_millis(at)
        .map(|at| at.format("%-m/%-d/%Y, %-I:%M:%S %p").to_string())
        .unwrap_or_else(|| "Unknown date".into())
}
