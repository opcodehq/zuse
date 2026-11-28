//! One live sidebar spans computer connections; each computer retains its execution state.
use super::*;
mod registry;
pub(super) use registry::Registry;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ComputerChat {
    pub computer: String,
    pub chat: String,
}
#[derive(Clone)]
pub(super) struct ComputerSource {
    pub key: String,
    pub label: String,
    pub state: Entity<AppState>,
}

pub(super) fn shared_project_key(space: &zuse_protocol::Space) -> String {
    if let Some(repository) = &space.repository_id {
        format!("repo:{repository}")
    } else if let Some(remote) = &space.remote_repository {
        format!("remote:{remote}")
    } else {
        format!("device:{}:space:{}", space.device_id, space.id)
    }
}

pub(super) struct Workspace {
    key: String,
    label: String,
    collapsed: bool,
    removed: bool,
    pub shell: Entity<Shell>,
    _observation: Subscription,
}

impl Shell {
    pub(super) fn computer_sources(&self, cx: &App) -> Vec<ComputerSource> {
        if let Some(owner) = self
            .sidebar_owner
            .as_ref()
            .and_then(|owner| owner.upgrade())
        {
            return owner.read(cx).computer_sources(cx);
        }
        let mut sources = vec![ComputerSource {
            key: "local".into(),
            label: "This computer".into(),
            state: self.state.clone(),
        }];
        sources.extend(
            self.remote_workspaces
                .iter()
                .map(|workspace| ComputerSource {
                    key: workspace.key.clone(),
                    label: workspace.label.clone(),
                    state: workspace.shell.read(cx).state.clone(),
                }),
        );
        sources
    }

    pub(super) fn open_computer_chat(&mut self, target: ComputerChat, cx: &mut Context<Self>) {
        if let Some(owner) = self.sidebar_owner.clone() {
            cx.defer(move |cx| {
                let _ = owner.update(cx, |owner, cx| owner.open_computer_chat(target, cx));
            });
            return;
        }
        if target.computer == "local" {
            self.open_chat(target.chat, cx);
        } else if let Some(index) = self
            .remote_workspaces
            .iter()
            .position(|workspace| workspace.key == target.computer)
        {
            self.active_remote_workspace = Some(index);
            self.remote_workspaces[index]
                .shell
                .clone()
                .update(cx, |shell, cx| shell.open_chat(target.chat, cx));
            cx.notify();
        }
    }

    pub(super) fn navigate_computer_chat(
        &mut self,
        forward: bool,
        slot: Option<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(owner) = self.sidebar_owner.clone() {
            cx.defer(move |cx| {
                let _ = owner.update(cx, |owner, cx| {
                    owner.navigate_computer_chat(forward, slot, cx);
                });
            });
            return true;
        }
        if self.remote_workspaces.is_empty() {
            return false;
        }
        let active = self
            .active_remote_workspace
            .and_then(|index| self.remote_workspaces.get(index))
            .map(|workspace| workspace.key.as_str())
            .unwrap_or("local");
        let selected = if let Some(index) = self.active_remote_workspace {
            self.remote_workspaces[index]
                .shell
                .read(cx)
                .state
                .read(cx)
                .selected_chat
                .clone()
        } else {
            self.state.read(cx).selected_chat.clone()
        };
        let current = self.computer_navigation.iter().position(|target| {
            target.computer == active && Some(target.chat.as_str()) == selected.as_deref()
        });
        let length = self.computer_navigation.len();
        if length == 0 {
            return true;
        }
        let index = slot.unwrap_or_else(|| match current {
            Some(index) if forward => (index + 1) % length,
            Some(index) => (index + length - 1) % length,
            None if forward => 0,
            None => length - 1,
        });
        if let Some(target) = self.computer_navigation.get(index).cloned() {
            self.open_computer_chat(target, cx);
        }
        true
    }

    pub(super) fn global_thread_matches(
        &self,
        query: &str,
        limit: usize,
        cx: &App,
    ) -> Option<Vec<ComputerChat>> {
        let sources = self.computer_sources(cx);
        if sources.len() <= 1 {
            return None;
        }
        let terms = thread_search::search_terms(query);
        let mut ranked = Vec::new();
        for source in sources {
            let state = source.state.read(cx);
            let device_label = format!(
                "{} {}",
                source.label,
                state
                    .local_device_id
                    .as_deref()
                    .and_then(|device| state.device_name(device))
                    .unwrap_or("")
            );
            for chat in state.chats.iter().filter(|chat| {
                chat.is_top_level()
                    && state
                        .local_device_id
                        .as_deref()
                        .is_none_or(|device| device == chat.device_id)
            }) {
                if query.is_empty() && chat.archived {
                    continue;
                }
                let pull_request = state
                    .change_request_for_chat(chat)
                    .map(|pr| {
                        format!(
                            "#{} {} {} {}",
                            pr.number, pr.title, pr.head_ref, pr.base_ref
                        )
                    })
                    .unwrap_or_default();
                let score = if query.is_empty() {
                    Some(0)
                } else {
                    thread_search::score_thread(
                        &thread_search::ThreadFields {
                            title: chat.title.as_deref().unwrap_or("New session"),
                            project: state
                                .space_for_chat(chat)
                                .map(|space| space.display_name())
                                .unwrap_or(""),
                            branch: crate::change_requests::conversation_branch(
                                chat,
                                &state.spaces,
                            )
                            .unwrap_or(""),
                            device: &device_label,
                            pull_request: &pull_request,
                            archived: chat.archived,
                        },
                        &terms,
                    )
                };
                if let Some(score) = score {
                    ranked.push((
                        ComputerChat {
                            computer: source.key.clone(),
                            chat: chat.id.clone(),
                        },
                        score,
                        chat.last_message_at
                            .unwrap_or(chat.created_at)
                            .timestamp_millis(),
                    ));
                }
            }
        }
        Some(thread_search::rank(ranked, limit))
    }

    fn collect_shared_project_rows(
        &mut self,
        label: &str,
        key: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<(String, String, AnyElement, Vec<ComputerChat>)> {
        self.pull_settings(cx);
        let rows = self.render_active_rows_for_computer(theme, Some(label), cx);
        let visible = self.sidebar_visible_order(cx);
        let pins = self.active_sidebar_pins(cx);
        let custom_sections = self.active_sidebar_sections(cx);
        let state = self.state.read(cx);
        let mut output = Vec::new();
        let mut pinned = Vec::new();
        let mut pinned_height = 0.;
        for (index, (row_key, height, element)) in rows.rows.into_iter().enumerate() {
            if index < rows.pinned_count {
                pinned.push(element);
                pinned_height += height;
                continue;
            }
            let (group, title) = rows
                .projects
                .get(&row_key)
                .cloned()
                .unwrap_or_else(|| (format!("{key}:{row_key}"), label.to_owned()));
            let custom = row_key
                .strip_prefix("custom:")
                .and_then(|id| custom_sections.iter().find(|section| section.id == id));
            let navigation = visible
                .iter()
                .filter(|id| {
                    if pins.contains(id) {
                        return false;
                    }
                    if let Some(section) = custom {
                        return section.session_ids.contains(id);
                    }
                    if let Some(chat_id) = row_key.strip_prefix("c:") {
                        return id.as_str() == chat_id;
                    }
                    let Some(group_key) = row_key.strip_prefix("g:") else {
                        return false;
                    };
                    !self.sidebar_collapsed_groups.contains(group_key)
                        && !custom_sections
                            .iter()
                            .any(|section| section.session_ids.contains(id))
                        && state
                            .chats
                            .iter()
                            .find(|chat| chat.id == **id)
                            .is_some_and(|chat| {
                                self.sidebar_group_key(state, chat).is_some_and(|group| {
                                    self.sidebar_group_collapse_key(&group) == group_key
                                })
                            })
                })
                .map(|chat| ComputerChat {
                    computer: key.into(),
                    chat: chat.clone(),
                })
                .collect();
            output.push((group, title, element, navigation));
        }
        if !pinned.is_empty() {
            let navigation = visible
                .iter()
                .filter(|id| pins.contains(id))
                .map(|chat| ComputerChat {
                    computer: key.into(),
                    chat: chat.clone(),
                })
                .collect();
            output.insert(
                0,
                (
                    "pinned".into(),
                    "Pinned".into(),
                    self.render_pinned_section(pinned, pinned_height, theme, cx),
                    navigation,
                ),
            );
        }
        output
    }

    fn render_shared_projects(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut groups: Vec<(String, String, Vec<AnyElement>, Vec<ComputerChat>)> = Vec::new();
        let computers: Vec<_> =
            std::iter::once((None, "local".to_owned(), "This computer".to_owned(), None))
                .chain(
                    self.remote_workspaces
                        .iter()
                        .enumerate()
                        .map(|(index, workspace)| {
                            (
                                Some(index),
                                workspace.key.clone(),
                                workspace.label.clone(),
                                Some(workspace.shell.clone()),
                            )
                        }),
                )
                .collect();
        let previous_navigation = self.computer_navigation.clone();
        let overlay = self
            .active_remote_workspace
            .map(|index| {
                self.remote_workspaces[index]
                    .shell
                    .read(cx)
                    .overlay_owns_keyboard(cx)
            })
            .unwrap_or_else(|| self.overlay_owns_keyboard(cx));
        let hints = self.computer_jump_hints && !overlay;
        self.computer_navigation.clear();
        for (index, key, label, shell) in computers {
            let slots = previous_navigation
                .iter()
                .enumerate()
                .filter(|(_, target)| target.computer == key)
                .map(|(index, target)| (target.chat.clone(), index))
                .collect();
            let collapsed = index.map_or(self.local_computer_collapsed, |index| {
                self.remote_workspaces[index].collapsed
            });
            let (status, rows) = if let Some(shell) = shell {
                shell.update(cx, |shell, cx| {
                    shell.sidebar_is_active = self.active_remote_workspace == index;
                    shell.computer_jump_slots = Some(slots);
                    shell.jump_hints = hints;
                    (
                        shell.computer_status(cx),
                        shell.collect_shared_project_rows(&label, &key, theme, cx),
                    )
                })
            } else {
                self.sidebar_is_active = self.active_remote_workspace.is_none();
                self.computer_jump_slots = Some(slots);
                self.jump_hints = hints;
                (
                    self.computer_status(cx),
                    self.collect_shared_project_rows(&label, &key, theme, cx),
                )
            };
            for (group, title, element, navigation) in rows {
                let part = div()
                    .id(format!("{group}:{key}"))
                    .when(!status.is_empty(), |part| {
                        part.child(
                            div()
                                .px(px(12.))
                                .text_size(crate::typography::text_xs())
                                .text_color(theme.text_faint)
                                .child(status),
                        )
                    })
                    .when(!collapsed, |part| part.child(element))
                    .into_any_element();
                if let Some(existing) = groups.iter_mut().find(|existing| existing.0 == group) {
                    existing.2.push(part);
                    if !collapsed {
                        existing.3.extend(navigation);
                    }
                } else {
                    groups.push((
                        group,
                        title,
                        vec![part],
                        if collapsed { Vec::new() } else { navigation },
                    ));
                }
            }
        }
        let mut elements = Vec::new();
        for (key, label, parts, navigation) in groups {
            let collapse_key = format!("shared:{key}");
            let collapsed = self.sidebar_collapsed_groups.contains(&collapse_key);
            if !collapsed {
                for target in navigation {
                    if !self.computer_navigation.contains(&target) {
                        self.computer_navigation.push(target);
                    }
                }
            }
            elements.push(
                div()
                    .id(format!("shared-project:{key}"))
                    .debug_selector({
                        let key = key.clone();
                        move || format!("shared-project:{key}")
                    })
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(format!("shared-project-toggle:{key}"))
                            .role(gpui::Role::Button)
                            .tab_index(0)
                            .aria_expanded(!collapsed)
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .px(px(8.))
                            .py(px(6.))
                            .text_size(crate::typography::text_sm())
                            .text_color(theme.text)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.sidebar_collapsed_groups.remove(&collapse_key) {
                                    this.sidebar_collapsed_groups.insert(collapse_key.clone());
                                }
                                cx.notify();
                            }))
                            .child(
                                icon(if collapsed {
                                    icons::ALT_ARROW_RIGHT
                                } else {
                                    icons::ALT_ARROW_DOWN
                                })
                                .size(px(11.)),
                            )
                            .child(label),
                    )
                    .when(!collapsed, |group| group.children(parts))
                    .into_any_element(),
            );
        }
        let nav = if let Some(index) = self.active_remote_workspace {
            self.remote_workspaces[index]
                .shell
                .clone()
                .update(cx, |shell, cx| shell.render_sidebar_nav(theme, cx))
        } else {
            self.render_sidebar_nav(theme, cx)
        };
        div()
            .w(px(
                (self.sidebar_content_width() - self.workspace_gutter()).max(0.)
            ))
            .h_full()
            .flex()
            .flex_col()
            .child(nav)
            .child(
                div()
                    .id("multi-computer-sidebar-list")
                    .debug_selector(|| "multi-computer-sidebar-list".into())
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.sidebar_scroll)
                    .children(elements),
            )
            .child(self.render_computer_connections(theme, cx))
            .child(
                div()
                    .px(px(8.))
                    .h(px(36.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(self.render_sidebar_footer_row(theme, cx)),
            )
            .into_any_element()
    }

    pub(super) fn sidebar_owns_device(&self, state: &AppState, device: &str) -> bool {
        (self.sidebar_owner.is_none() && self.remote_workspaces.is_empty())
            || state
                .local_device_id
                .as_deref()
                .is_none_or(|local| local == device)
    }

    pub(super) fn computer_sidebar_chats<'a>(
        &self,
        state: &'a AppState,
        now: chrono::DateTime<Utc>,
        filter: Option<&str>,
    ) -> Vec<(zuse_protocol::ChatIndicator, &'a zuse_protocol::Chat)> {
        state
            .sidebar_chats(now, filter)
            .into_iter()
            .filter(|(_, chat)| self.sidebar_owns_device(state, &chat.device_id))
            .collect()
    }

    /// Several pairing links or routes can resolve to the same computer.
    /// Keep its existing view, including drafts, when the stable identity arrives.
    pub(super) fn reconcile_computer_identities(&mut self, cx: &mut Context<Self>) {
        let mut seen = std::collections::HashMap::new();
        if let Some(local) = self.state.read(cx).local_device_id.clone() {
            seen.insert(local, None);
        }
        let mut selected = self
            .active_remote_workspace
            .and_then(|index| self.remote_workspaces.get(index))
            .map(|workspace| workspace.shell.clone());
        let previous = self.remote_workspaces.len();
        self.remote_workspaces.retain(|workspace| {
            let state = workspace.shell.read(cx).state.read(cx);
            if state.engine().is_none() && !matches!(state.connection, ConnectionStatus::Ready) {
                return true;
            }
            let Some(device) = state.local_device_id.clone() else {
                return true;
            };
            if let Some(existing) = seen.get(&device) {
                if selected.as_ref() == Some(&workspace.shell) {
                    selected = existing.clone();
                }
                return false;
            }
            seen.insert(device, Some(workspace.shell.clone()));
            true
        });
        self.active_remote_workspace = selected.and_then(|selected| {
            self.remote_workspaces
                .iter()
                .position(|workspace| workspace.shell == selected)
        });
        if self.remote_workspaces.is_empty() {
            self.sidebar_is_active = true;
        }
        if self.remote_workspaces.len() != previous {
            cx.notify();
        }
    }

    /// Route a navigation action to its computer without replacing the shared sidebar.
    pub(super) fn activate_sidebar_computer(&mut self, cx: &mut Context<Self>) {
        if let Some(owner) = self.sidebar_owner.clone() {
            let shell = cx.entity();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |owner, cx| {
                    if let Some(index) = owner
                        .remote_workspaces
                        .iter()
                        .position(|workspace| workspace.shell == shell)
                    {
                        owner.active_remote_workspace = Some(index);
                        cx.notify();
                    }
                });
            });
        } else if self.active_remote_workspace.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn open_remote_workspace(
        &mut self,
        target: crate::state::RemoteTarget,
        cx: &mut Context<Self>,
    ) {
        let (key, label) = match &target {
            crate::state::RemoteTarget::Paired(target) => (
                format!(
                    "paired:{}:{}",
                    target.origin(),
                    target.invitation().id.as_str()
                ),
                target.origin().to_owned(),
            ),
            crate::state::RemoteTarget::Account(target) => (
                format!("account:{}:{}", target.subject, target.environment),
                target.label.clone(),
            ),
            crate::state::RemoteTarget::Cloud(_) => return,
        };
        if let Some(index) = self
            .remote_workspaces
            .iter()
            .position(|workspace| workspace.key == key)
        {
            self.active_remote_workspace = Some(index);
            cx.notify();
            return;
        }
        // A newly connected computer should expose its projects directly.
        if self.remote_workspaces.is_empty() {
            self.settings.sidebar_organization = SidebarOrganization::ByProject;
            self.settings.space_filter = None;
            self.schedule_save(cx);
        }
        let mut boot = self.boot.clone();
        boot.target = Some(target);
        let state = cx.new(|cx| {
            let mut state = AppState::new();
            state.watch_clock_transitions(cx);
            state
        });
        crate::desktop_analytics::register(&state, cx);
        AppState::bootstrap(state.clone(), boot.clone(), cx);
        let owner = cx.entity().downgrade();
        let shared_sidebar = self.sidebar_pane.clone();
        let shared_scroll = self.sidebar_scroll.clone();
        let shell = cx.new(|cx| {
            let mut shell = Shell::new(state, boot, cx);
            shell.sidebar_owner = Some(owner);
            shell.sidebar_pane = shared_sidebar;
            shell.sidebar_scroll = shared_scroll;
            shell
        });
        let observation = cx.observe(&shell, |_, _, cx| cx.notify());
        self.remote_workspaces.push(Workspace {
            key,
            label,
            collapsed: false,
            removed: false,
            shell,
            _observation: observation,
        });
        self.active_remote_workspace = Some(self.remote_workspaces.len() - 1);
        cx.notify();
    }

    fn render_computer_connections(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let mut rows = div()
            .id("computer-connection-list")
            .flex_none()
            .max_h(px(120.))
            .overflow_y_scroll()
            .px(px(10.))
            .py(px(4.))
            .flex()
            .flex_col()
            .gap(px(3.));
        let computers = std::iter::once((None, "local", "This computer")).chain(
            self.remote_workspaces
                .iter()
                .enumerate()
                .map(|(index, workspace)| {
                    (
                        Some(index),
                        workspace.key.as_str(),
                        workspace.label.as_str(),
                    )
                }),
        );
        for (index, key, label) in computers {
            let key = key.to_owned();
            let selected = self.active_remote_workspace == index;
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .id(format!("select-computer-{key}"))
                            .role(gpui::Role::Button)
                            .tab_index(0)
                            .cursor_pointer()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .p(px(3.))
                            .rounded(px(4.))
                            .when(selected, |style| style.bg(theme.glass_hover()))
                            .hover(|style| style.bg(theme.glass_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.active_remote_workspace = index;
                                cx.notify();
                            }))
                            .child(
                                icon(icons::REMOTE_SERVER)
                                    .size(px(11.))
                                    .text_color(theme.text_faint),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::text_xs())
                                    .text_color(if selected {
                                        theme.text
                                    } else {
                                        theme.text_muted
                                    })
                                    .child(label.to_owned()),
                            ),
                    )
                    .when(index.is_some(), |row| {
                        row.child(
                            div()
                                .id(format!("remove-computer-{key}"))
                                .role(gpui::Role::Button)
                                .tab_index(0)
                                .aria_label(format!("Remove {label}"))
                                .cursor_pointer()
                                .p(px(3.))
                                .rounded(px(4.))
                                .hover(|style| style.bg(theme.glass_hover()))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.request_remove_computer(&key, cx)
                                }))
                                .child(
                                    icon(icons::CLOSE_CIRCLE)
                                        .size(px(12.))
                                        .text_color(theme.text_faint),
                                ),
                        )
                    }),
            );
        }
        rows.into_any_element()
    }

    fn computer_heading(
        &self,
        index: Option<usize>,
        label: String,
        collapsed: bool,
        status: &'static str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self.active_remote_workspace == index;
        let id = index.map_or_else(
            || "local-computer".to_owned(),
            |index| format!("remote-computer-{index}"),
        );
        div()
            .id(format!("{id}-heading"))
            .debug_selector({
                let id = id.clone();
                move || format!("{id}-heading")
            })
            .flex()
            .items_center()
            .gap(px(6.))
            .py(px(7.))
            .px(px(4.))
            .child(
                div()
                    .id(format!("{id}-toggle"))
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .role(gpui::Role::Button)
                    .tab_index(0)
                    .aria_label(format!(
                        "{} {label}",
                        if collapsed { "Expand" } else { "Collapse" }
                    ))
                    .aria_expanded(!collapsed)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(index) = index {
                            this.remote_workspaces[index].collapsed = !collapsed;
                        } else {
                            this.local_computer_collapsed = !collapsed;
                        }
                        cx.notify();
                    }))
                    .child(
                        icon(if collapsed {
                            icons::ALT_ARROW_RIGHT
                        } else {
                            icons::ALT_ARROW_DOWN
                        })
                        .size(px(11.))
                        .text_color(theme.text_faint),
                    )
                    .child(
                        icon(icons::REMOTE_SERVER)
                            .size(px(13.))
                            .text_color(if active { theme.text } else { theme.text_muted }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::text_xs())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_muted)
                            .child(label),
                    )
                    .when(!status.is_empty(), |el| {
                        el.child(
                            div()
                                .text_size(crate::typography::text_xs())
                                .text_color(theme.text_faint)
                                .child(status),
                        )
                    }),
            )
            .child(
                div()
                    .id(format!("{id}-new-chat"))
                    .role(gpui::Role::Button)
                    .tab_index(0)
                    .aria_label("New thread on this computer")
                    .cursor_pointer()
                    .rounded(px(4.))
                    .p(px(3.))
                    .hover(|el| el.bg(theme.glass_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(index) = index {
                            this.remote_workspaces[index]
                                .shell
                                .clone()
                                .update(cx, |shell, cx| shell.open_new_session(None, cx));
                        } else {
                            this.open_new_session(None, cx);
                        }
                    }))
                    .child(icon(icons::PLUS).size(px(12.)).text_color(theme.text_muted)),
            )
            .into_any_element()
    }

    fn computer_status(&self, cx: &App) -> &'static str {
        match self.state.read(cx).connection {
            ConnectionStatus::Ready => "",
            ConnectionStatus::Connecting => "Connecting…",
            ConnectionStatus::Failed(_) => "Offline",
        }
    }

    pub(super) fn render_multi_computer_sidebar(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.settings.sidebar_organization == SidebarOrganization::ByProject {
            return self.render_shared_projects(theme, cx);
        }
        self.sidebar_is_active = self.active_remote_workspace.is_none();
        self.computer_navigation.clear();
        let SidebarSessionItems {
            list: local,
            moving_row: moving,
        } = self.render_sidebar_session_items(theme, cx);
        if !self.local_computer_collapsed {
            self.computer_navigation
                .extend(
                    self.sidebar_visible_order(cx)
                        .into_iter()
                        .map(|chat| ComputerChat {
                            computer: "local".into(),
                            chat,
                        }),
                );
        }
        let mut groups = vec![
            div()
                .id("computer-local-projects")
                .flex()
                .flex_col()
                .child(self.computer_heading(
                    None,
                    "This computer".into(),
                    self.local_computer_collapsed,
                    self.computer_status(cx),
                    theme,
                    cx,
                ))
                .when(!self.local_computer_collapsed, |el| {
                    el.child(local).children(moving)
                })
                .into_any_element(),
        ];
        // Clone handles first: every row still dispatches to the computer that owns it.
        let computers: Vec<_> = self
            .remote_workspaces
            .iter()
            .enumerate()
            .map(|(index, workspace)| {
                (
                    index,
                    workspace.label.clone(),
                    workspace.collapsed,
                    workspace.shell.clone(),
                )
            })
            .collect();
        for (index, label, collapsed, shell) in computers {
            let key = self.remote_workspaces[index].key.clone();
            let (label, status, items, moving) = shell.update(cx, |shell, cx| {
                let label = {
                    let state = shell.state.read(cx);
                    state
                        .devices
                        .iter()
                        .find(|device| Some(device.id.as_str()) == state.local_device_id.as_deref())
                        .map(|device| device.name.clone())
                        .unwrap_or(label)
                };
                shell.sidebar_is_active = self.active_remote_workspace == Some(index);
                let status = shell.computer_status(cx);
                let SidebarSessionItems {
                    list: items,
                    moving_row: moving,
                } = shell.render_sidebar_session_items(theme, cx);
                if !collapsed {
                    self.computer_navigation.extend(
                        shell
                            .sidebar_visible_order(cx)
                            .into_iter()
                            .map(|chat| ComputerChat {
                                computer: key.clone(),
                                chat,
                            }),
                    );
                }
                (label, status, items, moving)
            });
            groups.push(
                div()
                    .id(("computer-remote-projects", index))
                    .debug_selector(move || format!("computer-remote-projects-{index}"))
                    .flex()
                    .flex_col()
                    .child(self.computer_heading(Some(index), label, collapsed, status, theme, cx))
                    .when(!collapsed, |el| el.child(items).children(moving))
                    .into_any_element(),
            );
        }
        let nav = if let Some(index) = self.active_remote_workspace {
            self.remote_workspaces[index]
                .shell
                .clone()
                .update(cx, |shell, cx| shell.render_sidebar_nav(theme, cx))
        } else {
            self.render_sidebar_nav(theme, cx)
        };
        let list = div()
            .id("multi-computer-sidebar-list")
            .debug_selector(|| "multi-computer-sidebar-list".into())
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.sidebar_scroll)
            .px(px(Theme::SPACE_SM))
            .children(groups);
        div()
            .w(px(
                (self.sidebar_content_width() - self.workspace_gutter()).max(0.)
            ))
            .h_full()
            .flex()
            .flex_col()
            .child(nav)
            .child(
                crate::edge_fade::edge_faded(SIDEBAR_GLASS_FADE_BAND, true, true, list)
                    .fade_overflow_y(&self.sidebar_scroll),
            )
            .child(self.render_computer_connections(theme, cx))
            .child(
                div()
                    .px(px(8.))
                    .h(px(36.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .child(self.render_sidebar_footer_row(theme, cx)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn computer_connections_reuse_one_window_and_retain_workspace_drafts(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_tokio::init(cx);
            settings::init(settings::UiSettings::default(), dir.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let boot = EngineBootConfig {
            target: None,
            data_dir: dir.path().into(),
            ipc_port: 0,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: None,
            default_harness: zuse_protocol::HarnessId::Mock,
        };
        let authority = zuse_remote::authority::Authority::open(dir.path()).unwrap();
        let (_, invitation) = authority.create("Remote test").unwrap();
        let target = crate::state::RemoteTarget::Paired(
            zuse_remote::PairedTarget::new("http://127.0.0.1:1", invitation).unwrap(),
        );
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(state, boot, cx)
        });
        window
            .update(cx, |shell, _, cx| {
                let local_state = shell.state.clone();
                let local_composer = shell.composer.clone();
                local_composer.update(cx, |composer, cx| {
                    composer.suggest_prompt("Keep my local draft", cx)
                });
                shell.open_remote_workspace(target.clone(), cx);
                assert_eq!(cx.windows().len(), 1);
                assert_eq!(shell.remote_workspaces.len(), 1);
                let remote = shell.remote_workspaces[0].shell.clone();
                remote.update(cx, |shell, cx| {
                    shell.composer.update(cx, |composer, cx| {
                        composer.suggest_prompt("Keep my remote draft", cx)
                    });
                });
                shell.active_remote_workspace = None;
                shell.open_remote_workspace(target, cx);
                assert_eq!(shell.active_remote_workspace, Some(0));
                assert_eq!(shell.remote_workspaces.len(), 1);
                assert_eq!(shell.remote_workspaces[0].shell, remote);
                assert_eq!(shell.state, local_state);
                assert_eq!(shell.composer, local_composer);
                assert!(local_composer.read(cx).has_draft(cx));
                assert!(remote.read(cx).composer.read(cx).has_draft(cx));
                assert_eq!(cx.windows().len(), 1);
            })
            .unwrap();
    }
    struct SidebarHost(Entity<Shell>, bool);
    impl Render for SidebarHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            if self.1 {
                div().size_full().child(self.0.clone())
            } else {
                div()
                    .w(px(300.))
                    .h(px(850.))
                    .child(self.0.read(cx).sidebar_pane.clone())
            }
        }
    }

    fn seed_computer(shell: &mut Shell, name: &str, cx: &mut Context<Shell>) {
        shell.settings.sidebar_organization = SidebarOrganization::ByProject;
        shell.reduced_motion = true;
        shell.state.update(cx, |state, cx| {
            state.connection = ConnectionStatus::Ready;
            state.workspace_scope = Some(WorkspaceScope::Local);
            state.local_device_id = Some(name.into());
            state.spaces = ["project", "empty"].into_iter().map(|id| serde_json::from_value(serde_json::json!({
                "id": id, "deviceId": name, "path": format!("/{name}/{id}"), "createdAt": Utc::now()
            })).unwrap()).collect();
            state.chats = vec![serde_json::from_value(serde_json::json!({
                "id": format!("{name}-chat"), "title": format!("{name} thread"), "spaceId": "project",
                "deviceId": name, "archived": false, "createdAt": Utc::now()
            })).unwrap()];
            cx.notify();
        });
    }

    #[gpui::test]
    fn one_sidebar_shows_every_computer_and_routes_chat_selection_to_its_owner(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            settings::init(settings::UiSettings::default(), dir.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let boot = EngineBootConfig {
            target: None,
            data_dir: dir.path().into(),
            ipc_port: 0,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: None,
            default_harness: zuse_protocol::HarnessId::Mock,
        };
        let (host, cx) = cx.add_window_view(|_, cx| {
            let root = cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let mut shell = Shell::new(state, boot.clone(), cx);
                seed_computer(&mut shell, "local", cx);
                shell
            });
            root.update(cx, |root, cx| {
                for name in ["macbook", "server"] {
                    let owner = cx.entity().downgrade();
                    let sidebar = root.sidebar_pane.clone();
                    let scroll = root.sidebar_scroll.clone();
                    let shell = cx.new(|cx| {
                        let state = cx.new(|_| AppState::new());
                        let mut shell = Shell::new(state, boot.clone(), cx);
                        shell.sidebar_owner = Some(owner);
                        shell.sidebar_pane = sidebar;
                        shell.sidebar_scroll = scroll;
                        seed_computer(&mut shell, name, cx);
                        shell
                    });
                    let observation = cx.observe(&shell, |_, _, cx| cx.notify());
                    root.remote_workspaces.push(Workspace {
                        key: name.into(),
                        label: name.into(),
                        collapsed: false,
                        removed: false,
                        shell,
                        _observation: observation,
                    });
                }
                root.active_remote_workspace = Some(0);
                cx.notify();
            });
            SidebarHost(root, false)
        });
        let root = host.read_with(cx, |host, _| host.0.clone());
        let redraw = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
        };
        redraw(cx);
        for selector in [
            "computer-checkout-This computer-project",
            "computer-checkout-macbook-project",
            "computer-checkout-server-project",
            "chat-local-chat",
            "chat-macbook-chat",
            "chat-server-chat",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "Missing {selector} from the shared sidebar"
            );
        }
        assert!(cx.debug_bounds("multi-computer-sidebar-list").is_some());
        assert!(
            cx.debug_bounds("computer-workspaces").is_none(),
            "The bottom switcher must be gone"
        );
        let remote = root.read_with(cx, |root, _| root.remote_workspaces[0].shell.clone());
        root.update(cx, |root, cx| {
            assert_eq!(root.sidebar_pane, remote.read(cx).sidebar_pane);
            assert_ne!(
                root.active_sidebar_pin_profile_key(cx),
                remote.read(cx).active_sidebar_pin_profile_key(cx)
            );
            let rows = root.render_active_rows(&Theme::default(), cx);
            assert!(
                rows.rows.iter().any(|(key, _, _)| key == "g:project:empty"),
                "Projects without chats must be visible"
            );
            // Synced registries can already contain another computer's chats.
            // Each computer section must show only that computer's rows.
            let foreign = remote.read(cx).state.read(cx).chats[0].clone();
            root.state.update(cx, |state, cx| {
                state.chats.push(foreign);
                cx.notify();
            });
            let state = root.state.read(cx);
            let visible = root.computer_sidebar_chats(state, Utc::now(), None);
            assert_eq!(visible.len(), 1);
            assert_eq!(visible[0].1.id, "local-chat");
        });
        root.update(cx, |root, cx| {
            root.state.update(cx, |state, cx| {
                state.spaces[0].repository_id = Some("shared-fixture".into());
                cx.notify();
            });
            let shared = root.remote_workspaces[0].shell.clone();
            shared.update(cx, |shell, cx| {
                shell.state.update(cx, |state, cx| {
                    state.spaces[0].repository_id = Some("shared-fixture".into());
                    cx.notify();
                })
            });
            let matches = root.global_thread_matches("macbook", 20, cx).unwrap();
            assert_eq!(matches.len(), 1);
            assert_eq!(matches[0].computer, "macbook");
            assert_eq!(
                root.global_thread_matches("thread", 20, cx).unwrap().len(),
                3
            );
        });
        redraw(cx);
        assert!(
            cx.debug_bounds("shared-project:repo:shared-fixture")
                .is_some()
        );
        root.read_with(cx, |root, _| assert_eq!(root.computer_navigation.len(), 3));
        let point = cx.debug_bounds("chat-server-chat").unwrap().center();
        cx.simulate_click(point, gpui::Modifiers::default());
        root.read_with(cx, |root, cx| {
            assert_eq!(root.active_remote_workspace, Some(1));
            assert_eq!(
                root.remote_workspaces[1]
                    .shell
                    .read(cx)
                    .state
                    .read(cx)
                    .selected_chat
                    .as_deref(),
                Some("server-chat")
            );
            assert_ne!(
                root.state.read(cx).selected_chat.as_deref(),
                Some("server-chat")
            );
        });
        redraw(cx);
        let point = cx.debug_bounds("chat-local-chat").unwrap().center();
        cx.simulate_click(point, gpui::Modifiers::default());
        root.read_with(cx, |root, cx| {
            assert_eq!(root.active_remote_workspace, None);
            assert_eq!(
                root.state.read(cx).selected_chat.as_deref(),
                Some("local-chat")
            );
            assert_eq!(
                root.remote_workspaces[1]
                    .shell
                    .read(cx)
                    .state
                    .read(cx)
                    .selected_chat
                    .as_deref(),
                Some("server-chat")
            );
        });
        // A computer going offline retains its cached projects and chats beside the others.
        remote.update(cx, |remote, cx| {
            remote.state.update(cx, |state, cx| {
                state.connection = ConnectionStatus::Failed("Disconnected".into());
                cx.notify();
            })
        });
        redraw(cx);
        assert!(cx.debug_bounds("chat-macbook-chat").is_some());
        let heading = cx
            .debug_bounds("computer-checkout-macbook-repo:shared-fixture")
            .unwrap();
        cx.simulate_click(
            gpui::point(heading.left() + px(80.), heading.center().y),
            gpui::Modifiers::default(),
        );
        redraw(cx);
        remote.read_with(cx, |remote, _| {
            assert!(
                remote
                    .sidebar_collapsed_groups
                    .contains("project:repo:shared-fixture")
            );
        });
        root.read_with(cx, |root, _| {
            assert!(
                !root
                    .computer_navigation
                    .iter()
                    .any(|target| target.chat == "macbook-chat")
            )
        });
        assert!(cx.debug_bounds("chat-local-chat").is_some());
        assert!(cx.debug_bounds("chat-server-chat").is_some());
        // Exercise the production Shell renderer too: an offline selected
        // computer must never replace the shared sidebar with its error gate.
        root.update(cx, |root, cx| {
            root.active_remote_workspace = Some(0);
            cx.notify();
        });
        host.update(cx, |host, cx| {
            host.1 = true;
            cx.notify();
        });
        redraw(cx);
        assert!(cx.debug_bounds("multi-computer-sidebar-list").is_some());
        assert!(cx.debug_bounds("chat-local-chat").is_some());
        assert!(cx.debug_bounds("chat-server-chat").is_some());
        let point = cx.debug_bounds("chat-local-chat").unwrap().center();
        cx.simulate_click(point, gpui::Modifiers::default());
        redraw(cx);
        root.read_with(cx, |root, _| assert_eq!(root.active_remote_workspace, None));
        assert!(cx.debug_bounds("multi-computer-sidebar-list").is_some());
        let point = cx.debug_bounds("chat-server-chat").unwrap().center();
        cx.simulate_click(point, gpui::Modifiers::default());
        redraw(cx);
        root.read_with(cx, |root, _| {
            assert_eq!(root.active_remote_workspace, Some(1))
        });
        assert!(cx.debug_bounds("multi-computer-sidebar-list").is_some());
        assert!(cx.debug_bounds("chat-local-chat").is_some());
        // Alternate routes and fresh pairing links cannot duplicate this
        // computer or replace an already-open remote computer's draft.
        root.update(cx, |root, cx| {
            let original_server = root.remote_workspaces[1].shell.clone();
            original_server.update(cx, |server, cx| {
                server.composer.update(cx, |composer, cx| {
                    composer.suggest_prompt("Keep this remote draft", cx)
                })
            });
            for (name, expected) in [("local", None), ("server", Some(1))] {
                let boot = root.boot.clone();
                let duplicate = cx.new(|cx| {
                    let state = cx.new(|_| AppState::new());
                    let mut shell = Shell::new(state, boot, cx);
                    seed_computer(&mut shell, name, cx);
                    shell
                });
                let observation = cx.observe(&duplicate, |_, _, cx| cx.notify());
                root.remote_workspaces.push(Workspace {
                    key: format!("alternate-{name}"),
                    label: name.into(),
                    collapsed: false,
                    removed: false,
                    shell: duplicate,
                    _observation: observation,
                });
                root.active_remote_workspace = Some(2);
                root.reconcile_computer_identities(cx);
                assert_eq!(root.remote_workspaces.len(), 2);
                assert_eq!(root.active_remote_workspace, expected);
                assert_eq!(root.remote_workspaces[1].shell, original_server);
            }
            assert!(original_server.read(cx).composer.read(cx).has_draft(cx));
            assert!(root.navigate_computer_chat(true, None, cx));
            assert_eq!(root.active_remote_workspace, None);
            assert!(root.navigate_computer_chat(true, Some(1), cx));
            assert_eq!(root.active_remote_workspace, Some(1));
            assert!(original_server.read(cx).composer.read(cx).has_draft(cx));
        });
    }
}
