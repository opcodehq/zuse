//! Cloud Setup catalog presentation. Mutations remain owned by RepositoryPanel.
use super::*;
pub(super) fn avatar(name: &str, url: Option<&str>, size: f32) -> AnyElement {
    let owner = name.split('/').next().unwrap_or(name);
    let fallback = owner.chars().take(2).collect::<String>().to_uppercase();
    let frame = div()
        .size(px(size))
        .flex_none()
        .rounded(px(7.))
        .overflow_hidden()
        .bg(s::frame_bg())
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(9.))
        .text_color(palette::rgb(TEXT));
    if let Some(url) = url {
        frame
            .child(
                img(SharedString::from(url.to_owned()))
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            )
            .into_any_element()
    } else {
        frame.child(fallback).into_any_element()
    }
}
impl RepositoryPanel {
    pub(super) fn github_connected(&self) -> bool {
        self.github
            .as_ref()
            .is_some_and(|g| g.installations.iter().any(|i| !i.suspended))
    }
    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = false;
        window.focus(&self.picker_trigger, cx);
        cx.notify();
    }
    fn picker_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.repository_busy() || !self.can_manage() || !self.github_connected() {
            return;
        }
        self.picker = !self.picker;
        self.selected.clear();
        self.error = None;
        if self.picker {
            self.refresh(cx);
            window.focus(&self.search.focus_handle(cx), cx);
        } else {
            window.focus(&self.picker_trigger, cx);
        }
        cx.notify();
    }
    fn available_repositories(&self, cx: &App) -> (usize, Vec<GithubRepository>) {
        let included = self
            .projects
            .iter()
            .map(|p| {
                p.repository_identity
                    .trim_start_matches("github.com/")
                    .to_lowercase()
            })
            .collect::<BTreeSet<_>>();
        let search = self.search.read(cx).value(cx).trim().to_lowercase();
        let available = self
            .github
            .as_ref()
            .map(|g| {
                g.repositories
                    .iter()
                    .filter(|r| !included.contains(&r.name_with_owner.to_lowercase()))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let count = available.len();
        (
            count,
            available
                .into_iter()
                .filter(|r| {
                    search.is_empty()
                        || r.name_with_owner.to_lowercase().contains(&search)
                        || r.description
                            .as_deref()
                            .unwrap_or("")
                            .to_lowercase()
                            .contains(&search)
                })
                .collect(),
        )
    }
    fn add_action(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.repository_busy();
        let disabled = busy || !self.github_connected() || !self.can_manage();
        let empty = self.projects.is_empty();
        let mut trigger = div().relative().child(
            cs::icon_label_button(
                "add-cloud-repositories",
                "plus",
                if empty { "Repository" } else { "" },
                false,
                disabled,
                false,
            )
            .when(!empty, |v| v.w(px(28.)).px(px(0.)).gap(px(0.)))
            .aria_label("Repository")
            .track_focus(&self.picker_trigger.clone().tab_stop(!disabled))
            .on_click(cx.listener(|this, _, window, cx| this.picker_action(window, cx))),
        );
        if self.picker {
            let (available, repos) = self.available_repositories(cx);
            let empty_matches = repos.is_empty();
            let mut focus = vec![self.search.focus_handle(cx)];
            if !busy {
                focus.push(self.picker_refresh.clone());
            }
            let mut choices = column()
                .id("cloud-repository-choices")
                .max_h(px(288.))
                .overflow_y_scroll()
                .p(px(4.))
                .rounded(px(7.))
                .bg(s::frame_bg().opacity(0.3));
            for repo in repos {
                let name = repo.name_with_owner.clone();
                let selected = self.selected.contains(&name);
                let handle = self
                    .choice_focus
                    .entry(name.clone())
                    .or_insert_with(|| cx.focus_handle())
                    .clone();
                focus.push(handle.clone());
                choices = choices.child(
                    div()
                        .id(SharedString::from(format!(
                            "cloud-repository-choice-{name}"
                        )))
                        .role(Role::Button)
                        .aria_label(SharedString::from(name.clone()))
                        .aria_selected(selected)
                        .track_focus(&handle.clone().tab_stop(true))
                        .tab_index(0)
                        .h(px(28.))
                        .px(px(8.))
                        .rounded(px(7.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .hover(|v| v.bg(s::frame_bg()))
                        .focus_visible(|v| v.border_1().border_color(palette::rgb(0x89ca21)))
                        .child(avatar(&name, repo.owner_avatar_url.as_deref(), 24.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(palette::rgb(TEXT))
                                .child(name.clone()),
                        )
                        .when(repo.is_private, |v| {
                            v.child(icon("access-ask").size(px(12.)))
                        })
                        .when(selected, |v| {
                            v.child(icon("check").size(px(14.)).text_color(palette::tone(
                                hsla(133.9 / 360., 0.606, 0.488, 1.),
                                hsla(131.4 / 360., 0.734, 0.31, 1.),
                            )))
                        })
                        .on_key_down(|e: &KeyDownEvent, _, cx| {
                            if matches!(e.keystroke.key.as_str(), "space" | "enter") {
                                cx.stop_propagation();
                            }
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.selected.remove(&name) {
                                this.selected.insert(name.clone());
                            }
                            cx.notify();
                        })),
                );
            }
            if empty_matches {
                choices = choices.child(
                    div()
                        .px(px(8.))
                        .py(px(20.))
                        .text_center()
                        .text_size(px(11.))
                        .line_height(px(16.))
                        .text_color(palette::rgb(MUTED))
                        .child(if available == 0 {
                            "Every available repository is already included."
                        } else {
                            "No repositories match this search."
                        }),
                );
            }
            let count = self.selected.len();
            let can_add = count > 0 && !busy && self.can_manage();
            if can_add {
                focus.push(self.picker_add.clone());
            }
            let menu = column()
                .id("cloud-repository-picker")
                .role(Role::Dialog)
                .aria_label("Choose repositories")
                .key_context("CloudRepositoryPicker")
                .on_action(cx.listener(|this, _: &CloseRepositoryPicker, window, cx| {
                    this.close_picker(window, cx);
                    cx.stop_propagation();
                }))
                .track_focus(&self.picker_focus)
                .w(px(
                    544_f32.min(f32::from(window.viewport_size().width) - 16.)
                ))
                .p(px(8.))
                .gap(px(8.))
                .rounded(px(10.))
                .bg(palette::tone(hsla(132. / 360., 0.05, 0.105, 1.), white()))
                .border_1()
                .border_color(s::card_border())
                .shadow_lg()
                .occlude()
                .capture_key_down(cx.listener(move |this, e: &KeyDownEvent, window, cx| {
                    match e.keystroke.key.as_str() {
                        "escape" => {
                            this.close_picker(window, cx);
                            cx.stop_propagation();
                        }
                        "tab" => {
                            let current =
                                focus.iter().position(|f| f.is_focused(window)).unwrap_or(0);
                            let next = if e.keystroke.modifiers.shift {
                                (current + focus.len() - 1) % focus.len()
                            } else {
                                (current + 1) % focus.len()
                            };
                            window.focus(&focus[next], cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_picker(window, cx)))
                .child(
                    row()
                        .gap(px(8.))
                        .child(s::field().h(px(28.)).flex_1().child(self.search.clone()))
                        .child(
                            cs::icon_button(
                                "refresh-repository-picker",
                                "refresh",
                                "Refresh GitHub repositories",
                                busy,
                                busy,
                            )
                            .track_focus(&self.picker_refresh.clone().tab_stop(!busy))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                        ),
                )
                .child(choices)
                .child(
                    row()
                        .h(px(28.))
                        .px(px(4.))
                        .justify_between()
                        .gap(px(12.))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(palette::rgb(MUTED))
                                .child(if count == 0 {
                                    format!("{available} available")
                                } else {
                                    format!("{count} selected")
                                }),
                        )
                        .child(
                            cs::button(
                                "connect-selected-repositories",
                                if count == 0 {
                                    "Add".to_owned()
                                } else {
                                    format!("Add {count}")
                                },
                                true,
                                !can_add,
                                false,
                            )
                            .track_focus(&self.picker_add.clone().tab_stop(can_add))
                            .on_click(cx.listener(
                                |this, _, window, cx| {
                                    if this.repository_busy()
                                        || this.selected.is_empty()
                                        || !this.can_manage()
                                    {
                                        return;
                                    }
                                    this.queue = this
                                        .github
                                        .as_ref()
                                        .map(|g| {
                                            g.repositories
                                                .iter()
                                                .filter(|r| {
                                                    this.selected.contains(&r.name_with_owner)
                                                })
                                                .cloned()
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    this.close_picker(window, cx);
                                    this.selected.clear();
                                    this.error = None;
                                    this.notice = None;
                                    this.connect_next(cx);
                                    cx.notify();
                                },
                            )),
                        ),
                );
            trigger = trigger.child(
                div()
                    .absolute()
                    .right(px(0.))
                    .top(relative(1.))
                    .child(deferred(
                        anchored()
                            .anchor(Anchor::TopRight)
                            .offset(point(px(0.), px(4.)))
                            .snap_to_window_with_margin(px(8.))
                            .child(menu),
                    )),
            );
        }
        trigger.into_any_element()
    }
    pub(super) fn repositories(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let busy = self.repository_busy();
        let can_manage = self.can_manage();
        let empty = self.projects.is_empty();
        let mut body = column();
        if empty {
            body = body
                .child(
                    div()
                        .px(px(12.))
                        .py(px(12.))
                        .text_center()
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette::rgb(TEXT).opacity(0.85))
                        .child(if busy {
                            "Loading repositories…"
                        } else {
                            "No repositories included"
                        }),
                )
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .pb(px(12.))
                        .child(self.add_action(window, cx)),
                );
        }
        for (i, project) in self.projects.iter().enumerate() {
            let id = project.project_id.clone();
            let identity = project
                .repository_identity
                .trim_start_matches("github.com/");
            let repo = self.github.as_ref().and_then(|g| {
                g.repositories
                    .iter()
                    .find(|r| r.name_with_owner.eq_ignore_ascii_case(identity))
            });
            let removing = self.pending.values().any(|p| p.kind == "remove");
            body = body.child(
                row()
                    .h(px(40.))
                    .gap(px(10.))
                    .px(px(12.))
                    .when(i > 0, |v| {
                        v.border_t_1()
                            .border_color(palette::rgb(BORDER).opacity(0.4))
                    })
                    .child(avatar(
                        identity,
                        repo.and_then(|r| r.owner_avatar_url.as_deref()),
                        24.,
                    ))
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .child(s::title_text(project.display_name.clone()).truncate())
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .line_height(px(13.5))
                                    .text_color(palette::rgb(MUTED))
                                    .child(format!(
                                        "{} · selected for cloud image",
                                        project.default_branch
                                    )),
                            ),
                    )
                    .child(
                        cs::icon_button(
                            SharedString::from(format!("remove-cloud-repository-{id}")),
                            "close",
                            format!("Remove {}", project.display_name),
                            busy || !can_manage,
                            removing,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.repository_busy() || !this.can_manage() {
                                return;
                            }
                            this.error = None;
                            this.notice = None;
                            this.call(
                                "remove",
                                methods::CLOUD_PROJECT_REMOVE,
                                json!({"projectId":id}),
                                cx,
                            );
                            cx.notify();
                        })),
                    ),
            );
        }
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .id("cloud-repository-error")
                    .role(Role::Alert)
                    .px(px(12.))
                    .py(px(8.))
                    .bg(s::destructive_text().opacity(0.1))
                    .text_size(px(12.))
                    .text_color(s::destructive_text())
                    .child(error.clone()),
            );
        }
        // A picker appears either in the populated header or in the empty card.
        let header_action = if empty {
            None
        } else {
            Some(self.add_action(window, cx))
        };
        cs::group(
            "Repositories",
            Some(
                "Choose the personal, organization, and collaborator repositories included in your private cloud image.",
            ),
            header_action,
            body,
        )
    }
    pub(super) fn github_group(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        if let Some(body) = self.github_operation_rows(cx) {
            return cs::group(
                "GitHub",
                Some(
                    "Connect your own GitHub account, then choose the installation to use with this workspace.",
                ),
                None,
                body,
            );
        }

        let loading = self
            .pending
            .values()
            .any(|p| matches!(p.kind, "github" | "github-install"));
        let configured = self.github.as_ref().is_none_or(|g| g.configured);
        let connected = self.github_connected();
        let mut body = column();
        if !configured {
            body = body.child(cs::setting_row(
                true,
                "GitHub App is not configured",
                "Add the GitHub App credentials to the API, deploy it, then retry.",
                Some(cs::error_badge("Unavailable").into_any_element()),
            ));
        } else if self
            .github
            .as_ref()
            .is_none_or(|g| g.installations.is_empty() || g.user.is_none())
        {
            body = body.child(cs::setting_row(
                true,
                "Connect repositories",
                "Install the Zuse app on GitHub, then choose which repositories cloud agents can work on.",
                Some(
                    cs::icon_label_button(
                        "connect-cloud-github",
                        "github",
                        "Connect GitHub",
                        true,
                        loading,
                        loading,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if !this.repository_busy() {
                            this.call(
                                "github-install",
                                zuse_protocol::methods::CLOUD_GITHUB_INSTALL,
                                serde_json::json!({}),
                                cx,
                            );
                        }
                    }))
                    .into_any_element(),
                ),
            ));
        } else if let Some(github) = &self.github {
            for (i, install) in github.installations.iter().enumerate() {
                let badge = if install.suspended {
                    cs::badge("Suspended", true, false)
                } else if github.user.is_none() {
                    cs::badge("Install GitHub App", true, false)
                } else {
                    cs::badge_with_icon("Connected", "check", window)
                };
                body = body.child(cs::setting_row_content(
                    i == 0,
                    install.account_login.clone(),
                    format!(
                        "{} · {}",
                        install.account_type,
                        if install.repository_selection == "selected" {
                            "selected repositories"
                        } else {
                            "all repositories"
                        }
                    ),
                    Some(badge.into_any_element()),
                    row()
                        .w_full()
                        .gap(px(8.))
                        .child(avatar(
                            &install.account_login,
                            install.avatar_url.as_deref(),
                            20.,
                        ))
                        .child(s::description_text(format!(
                            "{} repositories available across connected installations.",
                            github.repositories.len()
                        ))),
                ));
            }
            if connected {
                body=body.child(cs::setting_row(false,github.user.as_ref().map(|u|format!("Connected as {}",u.login)).unwrap_or_else(||"Connection status".into()),"Repository changes refresh automatically when you return from GitHub. Existing cloud chats are not affected.",Some(cs::icon_label_button("refresh-cloud-github","refresh","Refresh",false,loading,loading).on_click(cx.listener(|this,_,_,cx|this.refresh(cx))).into_any_element())));
            }
        }
        if let Some(disconnect) = self.github_disconnect_row(cx) {
            body = body.child(disconnect);
        }
        cs::group(
            "GitHub",
            Some(
                "Connect GitHub once to choose repositories and let cloud agents commit, push, and open pull requests as you.",
            ),
            None,
            body,
        )
    }
}
impl Render for RepositoryPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.loaded && self.owner.is_some() {
            self.refresh(cx);
        }
        if self.usage_only {
            return self.render_storage(window, cx);
        }
        if self.guide_visible() && !self.agent_recovery_open() {
            return self.render_guide(window, cx);
        }
        let entry = self.guide_entry(cx);
        let repos = self.repositories(window, cx);
        let github = self.github_group(window, cx);
        let agents = self.render_agents(window, cx);
        let environment = self.render_environment(cx);
        let machine = self.render_machine(cx);
        let mut root = column().gap(px(16.)).when_some(entry, |root, entry| root.child(entry));
        if self.agent_recovery_open() {
            return root
                .child(agents)
                .child(github)
                .child(repos)
                .child(machine)
                .child(environment);
        }
        if self.github_connected() {
            root = root.child(repos).child(github);
        } else {
            root = root.child(github).child(repos);
        }
        root.child(agents).child(machine).child(environment)
    }
}
