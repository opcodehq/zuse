//! Threads and project-grouped navigation share selection and composer drafts.
use super::*;
use crate::assets::icon;
use crate::localization::message as tr;
use crate::shell::{
    BG, BORDER, FAINT, MUTED, RAISED, TEXT, column, icon_button, label, provider_icon, row,
};
use zuse_i18n::messages as msg;
use zuse_protocol::presentation::{SidebarView, project_identity};

struct SidebarProjectGroup {
    key: String,
    path: String,
    name: String,
    chats: Vec<Value>,
    cloud_tasks: Vec<Value>,
    cloud_project: Option<Value>,
}

impl WorkspaceView {
    pub(super) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = true;
        self.search_open = !self.search_open;
        window.focus(
            &if self.search_open {
                self.session_search.focus_handle(cx)
            } else {
                self.input.focus_handle(cx)
            },
            cx,
        );
        cx.notify();
    }
    pub(super) fn start_new_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // On a task computer this is another conversation on the same
        // computer. A project header explicitly creates a new task instead.
        if (self.cloud.guest || self.cloud.cached) && self.selected.is_some() {
            self.start_thread_conversation(cx);
            return;
        }
        let no_local_project = self.selected.is_none()
            && self.project.is_none()
            && self
                .spaces
                .iter()
                .all(|space| zuse_protocol::projectless::is_projectless_cwd(Some(&space.path)));
        if self.cloud.mode || no_local_project {
            if let Some(project) = self.cloud_draft_project() {
                self.start_cloud_project_session(project, window, cx);
                return;
            }
            if self.cloud.mode {
                self.settings = true;
                self.select_settings_section(crate::settings_view::Section::Cloud, cx);
                cx.notify();
                return;
            }
        }
        if !self.require_connection(cx)
            || self.file_navigation_blocked(cx)
            || self.queue_edit_navigation_blocked(cx)
        {
            return;
        }
        if !self.new_chat(cx) {
            return;
        }
        self.model_picker = false;
        self.access_picker = false;
        self.add_menu = false;
        window.focus(&self.input.focus_handle(cx), cx);
    }
    fn start_project_session(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(navigation) = &self.cloud.local_navigation {
            let _=self.requests.unbounded_send(json!({"ui":"newLocalProjectConversation","path":path,"deviceId":navigation.device,"epoch":navigation.epoch,"accountId":self.cloud.account,"organizationId":self.cloud.organization}));
            return;
        }
        if !self.require_connection(cx)
            || self.file_navigation_blocked(cx)
            || self.queue_edit_navigation_blocked(cx)
        {
            return;
        }
        if !self.new_chat(cx) {
            return;
        }
        self.project = if path.is_empty() {
            None
        } else {
            Some(json!({"path":path,"name":path.rsplit('/').next().unwrap_or(&path)}))
        };
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }
    pub(super) fn conversation_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(selected) = &self.selected else {
            return Empty.into_any_element();
        };
        let root = selected["threadId"]
            .as_str()
            .or_else(|| selected["id"].as_str())
            .unwrap_or_default();
        let mut chats: Vec<_> = self
            .chats
            .iter()
            .filter(|chat| {
                chat["threadId"].as_str().or_else(|| chat["id"].as_str()) == Some(root)
                    && chat["parentChatId"].is_null()
                    && chat["archived"] != true
            })
            .collect();
        chats.sort_by_key(|chat| {
            (
                chat["id"] != root,
                chat["createdAt"].as_str().unwrap_or_default(),
            )
        });
        row().w_full().min_w_0().h(px(36.)).px_3().gap_1()
            .child(row().id("thread-conversations").min_w_0().overflow_x_scroll().gap_1()
                .children(chats.into_iter().enumerate().map(|(index, chat)| {
                    let active = chat["id"] == selected["id"];
                    let chat = chat.clone();
                    let title = chat["title"].as_str().map(str::to_owned).unwrap_or_else(|| format!("Chat {}", index+1));
                    let provider = chat["config"]["harness"].as_str().unwrap_or_default();
                    row().id(SharedString::from(format!("thread-conversation-{}", chat["id"].as_str().unwrap_or_default())))
                        .h(px(28.)).px_2().gap_2().rounded(px(7.)).cursor_pointer()
                        .child(icon(provider_icon(provider)).size(px(14.)).text_color(rgb(MUTED)))
                        .child(div().min_w_0().truncate().text_size(px(12.)).child(title))
                        .role(Role::Tab).aria_selected(active).max_w(px(180.)).flex_shrink_0()
                        .when(active, |d| d.bg(rgb(RAISED)).text_color(rgb(TEXT)))
                        .on_click(cx.listener(move |this, _, _, cx| { if this.cloud.cached { let _ = this.requests.unbounded_send(json!({"ui":"selectCachedCloudSession","sessionId":chat["id"]})); } else { this.select(chat.clone(), cx); } }))
                })))
            .when(selected["archived"] != true, |d| d.child(icon_button("add-thread-conversation", "plus", "New chat in this thread")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.start_thread_conversation(cx);
                }))))
            .into_any_element()
    }

    pub(super) fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        self.update_sidebar_controls(cx);
        let query = if self.search_open {
            self.session_search.read(cx).value(cx).to_lowercase()
        } else {
            String::new()
        };
        let navigation_chats = self
            .cloud
            .local_navigation
            .as_ref()
            .map(|v| v.chats.as_slice())
            .unwrap_or(self.chats.as_slice());
        let mut rows = navigation_chats
            .iter()
            .filter(|c| {
                c["archived"].as_bool().unwrap_or(false) == self.show_archive
                    && c["parentChatId"].is_null()
                    && c["threadId"].is_null()
            })
            // Zots list their own threads; projects don't repeat them.
            .filter(|c| {
                !zuse_protocol::presentation::zot_panel::zot_owns_chat(
                    &self.zots.items,
                    c["id"].as_str().unwrap_or_default(),
                )
            })
            .filter(|c| {
                query.is_empty()
                    || ["title", "cwd", "branch", "lastMessagePreview"]
                        .iter()
                        .any(|field| {
                            c[field]
                                .as_str()
                                .is_some_and(|s| s.to_lowercase().contains(&query))
                        })
            })
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| {
            let at = |c: &Value| {
                c["lastMessageAt"]
                    .as_str()
                    .or_else(|| c["createdAt"].as_str())
                    .unwrap_or_default()
                    .to_owned()
            };
            at(b)
                .cmp(&at(a))
                .then_with(|| a["id"].as_str().cmp(&b["id"].as_str()))
        });
        let projects = self.preferences.sidebar_view == SidebarView::Projects;
        let cloud_rows: Vec<_> = self
            .cloud
            .workspaces
            .iter()
            .filter(|workspace| {
                !self.show_archive
                    && !matches!(
                        workspace["state"].as_str(),
                        Some("deleted" | "archived" | "archiving")
                    )
                    && !matches!(
                        workspace["desiredState"].as_str(),
                        Some("deleted" | "archived")
                    )
                    && (query.is_empty()
                        || format!(
                            "{} {}",
                            workspace["title"].as_str().unwrap_or_default(),
                            workspace["branch"].as_str().unwrap_or_default()
                        )
                        .to_lowercase()
                        .contains(&query))
            })
            .collect();
        let project_groups = if projects {
            self.project_groups(&rows, query.is_empty(), &query)
        } else {
            vec![]
        };
        let empty = if projects {
            project_groups.is_empty()
        } else {
            rows.is_empty() && cloud_rows.is_empty()
        };
        column()
            .w_full()
            .h_full()
            .flex_shrink_0()
            .bg(rgb(BG))
            .rounded(px(crate::workspace_style::RADIUS))
            .overflow_hidden()
            .child(
                row()
                    .h(px(36.))
                    .pl(px(12.))
                    .pr(px(4.))
                    .gap(px(4.))
                    .child(
                        label(tr(cx, msg::chat::top_bar_zuse_beta))
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(TEXT)),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button("sidebar-collapse", "split-columns", "Hide sidebar")
                            .size(px(28.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sidebar_open = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                column()
                    .px(px(6.))
                    .py(px(4.))
                    .gap(px(2.))
                    .child(
                        sidebar_action(
                            "sidebar-new-chat",
                            crate::localization::text("chat-chat_view_new_chat", cx),
                            "pen-new-square",
                        )
                        .child(label("Ctrl+N").text_size(px(12.)).text_color(rgb(MUTED)))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.start_new_session(window, cx)),
                        ),
                    )
                    .child(
                        sidebar_action(
                            "sidebar-new-project",
                            crate::localization::text("projects-projects_sidebar_new_project", cx),
                            "folder-with-files",
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_project_setup(cx);
                        })),
                    )
                    .when(self.cloud_account["user"].is_object(), |v| {
                        v.child(
                            sidebar_action(
                                "sidebar-plugins",
                                crate::localization::text("plugins-plugins_back", cx),
                                "settings-plugins",
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_plugins_browse(cx);
                            })),
                        )
                    }),
            )
            .child(self.zot_sidebar(cx))
            .when(self.search_open, |d| {
                d.child(
                    row()
                        .mx_3()
                        .mb_3()
                        .px_2()
                        .py_1()
                        .gap_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(BORDER))
                        .bg(rgb(BG))
                        .child(icon("magnifer").size(px(14.)).text_color(rgb(FAINT)))
                        .child(div().flex_1().min_w_0().child(self.session_search.clone()))
                        .child(
                            icon_button("clear-session-search", "close", "Close search").on_click(
                                cx.listener(|this, _, window, cx| this.toggle_search(window, cx)),
                            ),
                        ),
                )
            })
            .child(
                // Section header shares the Zots header's quiet style; its
                // controls stay out of the way until the header is hovered.
                row()
                    .id("threads-header")
                    .group("threads-header")
                    .h(px(28.))
                    .mx_2()
                    .px_2()
                    .gap_1()
                    .child(
                        row()
                            .id("sidebar-view-switch")
                            .role(Role::Button)
                            .aria_label("Switch threads and projects")
                            .cursor_pointer()
                            .gap_1()
                            .text_size(px(11.))
                            .text_color(rgb(FAINT))
                            .hover(|d| d.text_color(rgb(MUTED)))
                            .child(if self.show_archive {
                                crate::localization::text("native-archived_threads", cx)
                            } else if projects {
                                crate::localization::text("projects-projects_sidebar_projects", cx)
                            } else {
                                crate::localization::text("projects-projects_sidebar_threads", cx)
                            })
                            .child(icon("alt-arrow-down").size(px(11.)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preferences.sidebar_view = if projects {
                                    SidebarView::Threads
                                } else {
                                    SidebarView::Projects
                                };
                                this.save_preferences();
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        row()
                            .gap(px(2.))
                            .when(!self.show_archive, |d| {
                                d.invisible().group_hover("threads-header", |d| d.visible())
                            })
                            .when(projects, |d| {
                                d.child(
                                    icon_button("add-sidebar-project", "plus", "Open project")
                                        .size(px(20.))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.open_project_setup(cx);
                                        })),
                                )
                            })
                            .child(
                                icon_button(
                                    "archive-view",
                                    "archive-minimalistic",
                                    if self.show_archive {
                                        "Back to threads"
                                    } else {
                                        "Show archived threads"
                                    },
                                )
                                .size(px(20.))
                                .when(self.show_archive, |d| {
                                    d.bg(rgb(RAISED)).text_color(rgb(TEXT))
                                })
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.show_archive = !this.show_archive;
                                        cx.notify();
                                    },
                                )),
                            ),
                    ),
            )
            .child(
                column()
                    .id("sessions")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .gap_1()
                    .when(empty, |d| {
                        d.child(
                            column()
                                .p_4()
                                .gap_2()
                                .child(label(if query.is_empty() {
                                    crate::localization::text(
                                        "projects-projects_sidebar_no_threads_yet",
                                        cx,
                                    )
                                } else {
                                    crate::localization::text("native-no_matching_threads", cx)
                                }))
                                .child(
                                    label(if query.is_empty() {
                                        crate::localization::text(
                                            "native-start_conversation_shortcut",
                                            cx,
                                        )
                                    } else {
                                        crate::localization::text(
                                            "native-try_title_project_branch",
                                            cx,
                                        )
                                    })
                                    .text_size(px(11.))
                                    .text_color(rgb(FAINT)),
                                ),
                        )
                    })
                    .when(!projects, |d| {
                        d.children(rows.iter().map(|chat| self.session_row(chat, false, cx)))
                            .children(
                                cloud_rows
                                    .iter()
                                    .map(|workspace| self.cloud_session_row(workspace, cx)),
                            )
                    })
                    .when(projects, |d| {
                        d.children(project_groups.into_iter().map(|group| {
                            let SidebarProjectGroup {
                                key,
                                path,
                                name,
                                chats,
                                cloud_tasks,
                                cloud_project,
                            } = group;
                            let collapsed = self.preferences.collapsed_projects.contains(&key);
                            let toggle = key.clone();
                            let create = path.clone();
                            let create_cloud = cloud_project.clone();
                            let explicit_cloud = cloud_project.filter(|_| !path.is_empty());
                            let archive_space = self
                                .spaces
                                .iter()
                                .find(|s| zuse_protocol::view::project_key(s) == key)
                                .map(|s| s.id.clone());
                            let context_space = archive_space.clone();
                            column()
                                .flex_shrink_0()
                                .gap_1()
                                .mb_2()
                                .child(
                                    row()
                                        .id(SharedString::from(format!("project-group-{key}")))
                                        .cursor_pointer()
                                        .h(px(34.))
                                        .px_2()
                                        .gap_2()
                                        .rounded_md()
                                        .hover(|d| d.bg(rgb(0x222222)))
                                        .on_mouse_down(
                                            MouseButton::Right,
                                            cx.listener(
                                                move |this, event: &MouseDownEvent, window, cx| {
                                                    if let Some(id) = context_space.clone() {
                                                        this.project_archive_menu(
                                                            id,
                                                            event.position,
                                                            window,
                                                            cx,
                                                        );
                                                        cx.stop_propagation();
                                                    }
                                                },
                                            ),
                                        )
                                        .child(
                                            icon(if collapsed {
                                                "alt-arrow-right"
                                            } else {
                                                "alt-arrow-down"
                                            })
                                            .size(px(10.))
                                            .text_color(rgb(FAINT)),
                                        )
                                        .child(icon("folder").size(px(14.)).text_color(rgb(MUTED)))
                                        .child(
                                            div()
                                                .flex_1()
                                                .min_w_0()
                                                .text_ellipsis()
                                                .text_size(px(12.))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(name),
                                        )
                                        .child(
                                            label((chats.len() + cloud_tasks.len()).to_string())
                                                .text_size(px(10.))
                                                .text_color(rgb(FAINT)),
                                        )
                                        .when_some(archive_space, |d, id| {
                                            d.child(
                                                icon_button(
                                                    SharedString::from(format!(
                                                        "project-menu-{id}"
                                                    )),
                                                    "more-horizontal",
                                                    crate::localization::text(
                                                        "native-project_actions",
                                                        cx,
                                                    ),
                                                )
                                                .size(px(22.))
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation()
                                                })
                                                .on_click(cx.listener(
                                                    move |this, event: &ClickEvent, window, cx| {
                                                        cx.stop_propagation();
                                                        this.project_archive_menu(
                                                            id.clone(),
                                                            event.position(),
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                            )
                                        })
                                        .when_some(explicit_cloud, |d, project| {
                                            d.child(
                                                icon_button(
                                                    SharedString::from(format!(
                                                        "new-cloud-project-thread-{key}"
                                                    )),
                                                    "cloud-dither",
                                                    "New cloud chat in this project",
                                                )
                                                .size(px(22.))
                                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                    cx.stop_propagation()
                                                })
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.start_cloud_project_session(
                                                            project.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                            )
                                        })
                                        .child(
                                            icon_button(
                                                SharedString::from(format!(
                                                    "new-project-thread-{key}"
                                                )),
                                                "plus",
                                                "New thread in this project",
                                            )
                                            .size(px(22.))
                                            .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                                cx.stop_propagation()
                                            })
                                            .on_click(
                                                cx.listener(move |this, _, window, cx| {
                                                    cx.stop_propagation();
                                                    if create.is_empty() {
                                                        if let Some(project) = &create_cloud {
                                                            this.start_cloud_project_session(
                                                                project.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                        }
                                                    } else {
                                                        this.start_project_session(
                                                            create.clone(),
                                                            window,
                                                            cx,
                                                        );
                                                    }
                                                }),
                                            ),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if let Some(index) = this
                                                .preferences
                                                .collapsed_projects
                                                .iter()
                                                .position(|p| p == &toggle)
                                            {
                                                this.preferences.collapsed_projects.remove(index);
                                            } else {
                                                this.preferences
                                                    .collapsed_projects
                                                    .push(toggle.clone());
                                            }
                                            this.save_preferences();
                                            cx.notify();
                                        })),
                                )
                                .when(!collapsed, |d| {
                                    d.when(chats.is_empty() && cloud_tasks.is_empty(), |d| {
                                        d.child(
                                            label(tr(cx, msg::native::no_threads_in_this_project))
                                                .pl_6()
                                                .py_2()
                                                .text_size(px(10.))
                                                .text_color(rgb(FAINT)),
                                        )
                                    })
                                    .children(
                                        chats.iter().map(|chat| self.session_row(chat, true, cx)),
                                    )
                                    .children(
                                        cloud_tasks
                                            .iter()
                                            .map(|workspace| self.cloud_session_row(workspace, cx)),
                                    )
                                })
                        }))
                    }),
            )
            .child(
                row()
                    .h(px(36.))
                    .px(px(8.))
                    .gap(px(2.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.sidebar_controls.clone()),
                    )
                    .child(
                        crate::sidebar_controls::footer_button("settings", "Settings")
                            .w(px(28.))
                            .child(icon("settings-minimalistic").size(px(16.)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_plugins_browse();
                                this.settings = !this.settings;
                                if this.settings {
                                    this.refresh_worktree_settings();
                                    this.refresh_title_settings();
                                }
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
    fn project_groups(
        &self,
        rows: &[&Value],
        include_empty: bool,
        query: &str,
    ) -> Vec<SidebarProjectGroup> {
        let navigation_spaces = self
            .cloud
            .local_navigation
            .as_ref()
            .map(|v| v.spaces.as_slice())
            .unwrap_or(self.spaces.as_slice());
        let projects = if self.cloud.local_navigation.is_some() {
            navigation_spaces
                .iter()
                .filter(|space| !zuse_protocol::projectless::is_projectless_cwd(Some(&space.path)))
                .map(|space| json!({"path":space.path,"name":space.display_name()}))
                .collect()
        } else {
            self.known_projects()
        };
        let roots = projects
            .iter()
            .filter_map(|p| p["path"].as_str())
            .collect::<Vec<_>>();
        let mut groups = std::collections::BTreeMap::<String, SidebarProjectGroup>::new();
        for chat in rows {
            let identity = project_identity(
                chat["cwd"].as_str(),
                chat["spaceId"].as_str(),
                navigation_spaces,
                &roots,
            );
            groups
                .entry(identity.key.clone())
                .or_insert_with(|| SidebarProjectGroup {
                    key: identity.key,
                    path: identity.path,
                    name: identity.name,
                    chats: Vec::new(),
                    cloud_tasks: Vec::new(),
                    cloud_project: None,
                })
                .chats
                .push((*chat).clone());
        }
        if include_empty && !self.show_archive {
            for root in roots.iter().copied() {
                let space = navigation_spaces.iter().find(|s| s.path == root);
                let identity = project_identity(
                    Some(root),
                    space.map(|s| s.id.as_str()),
                    navigation_spaces,
                    &roots,
                );
                groups
                    .entry(identity.key.clone())
                    .or_insert(SidebarProjectGroup {
                        key: identity.key,
                        path: identity.path,
                        name: identity.name,
                        chats: Vec::new(),
                        cloud_tasks: Vec::new(),
                        cloud_project: None,
                    });
            }
        }
        if !self.show_archive {
            for project in &self.cloud.projects {
                let Some(identity) =
                    zuse_protocol::presentation::cloud_project_group(project, navigation_spaces)
                else {
                    continue;
                };
                let cloud_tasks: Vec<_> = self
                    .cloud
                    .workspaces
                    .iter()
                    .filter(|workspace| {
                        workspace["projectId"].as_str() == Some(identity.project_id.as_str())
                            && !matches!(
                                workspace["state"].as_str(),
                                Some("deleted" | "archived" | "archiving")
                            )
                            && !matches!(
                                workspace["desiredState"].as_str(),
                                Some("deleted" | "archived")
                            )
                            && (query.is_empty()
                                || format!(
                                    "{} {} {}",
                                    identity.name,
                                    workspace["title"].as_str().unwrap_or_default(),
                                    workspace["branch"].as_str().unwrap_or_default()
                                )
                                .to_lowercase()
                                .contains(query))
                    })
                    .cloned()
                    .collect();
                if !query.is_empty() && cloud_tasks.is_empty() {
                    continue;
                }
                let path = identity
                    .local_space
                    .as_ref()
                    .and_then(|id| navigation_spaces.iter().find(|space| &space.id == id))
                    .map(|space| space.path.clone())
                    .unwrap_or_default();
                let group =
                    groups
                        .entry(identity.key.clone())
                        .or_insert_with(|| SidebarProjectGroup {
                            key: identity.key,
                            path,
                            name: identity.name,
                            chats: Vec::new(),
                            cloud_tasks: Vec::new(),
                            cloud_project: None,
                        });
                group.cloud_tasks.extend(cloud_tasks);
                group.cloud_project = Some(project.clone());
            }
        }
        let mut groups: Vec<_> = groups.into_values().collect();
        groups.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.key.cmp(&b.key))
        });
        groups
    }
    fn cloud_session_row(&self, workspace: &Value, cx: &mut Context<Self>) -> AnyElement {
        let workspace = workspace.clone();
        let title = workspace["title"]
            .as_str()
            .or_else(|| workspace["branch"].as_str())
            .unwrap_or("New cloud chat")
            .to_owned();
        let state = match workspace["state"].as_str() {
            Some("paused") => "Sleeping",
            Some("pausing") => "Going to sleep",
            Some("ready") => "Ready",
            Some("failed" | "error") => "Needs attention",
            _ => "Starting",
        };
        row()
            .id(SharedString::from(format!(
                "cloud-thread-{}",
                workspace["workspaceId"].as_str().unwrap_or_default()
            )))
            .h(px(32.))
            .px_2()
            .gap_2()
            .rounded_md()
            .cursor_pointer()
            .hover(|d| d.bg(rgb(RAISED)))
            .child(icon("cloud-dither").size(px(13.)).text_color(rgb(MUTED)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(12.))
                    .child(title),
            )
            .child(label(state).text_size(px(10.)).text_color(rgb(FAINT)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_cloud(workspace.clone());
                cx.notify();
            }))
            .into_any_element()
    }
    fn session_row(&self, chat: &Value, nested: bool, cx: &mut Context<Self>) -> AnyElement {
        let selected = !self.usage_open
            && self.project_archive.project.is_none()
            && self
                .selected
                .as_ref()
                .is_some_and(|c| c["id"] == chat["id"]);
        let status = self
            .sessions
            .iter()
            .find(|s| s["chatId"] == chat["id"])
            .and_then(|s| s["status"].as_str())
            .unwrap_or_default();
        let active = matches!(status, "working" | "awaitingInput");
        let age = chat["lastMessageAt"]
            .as_str()
            .or_else(|| chat["createdAt"].as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|at| {
                let m = chrono::Utc::now()
                    .signed_duration_since(at)
                    .num_minutes()
                    .max(0);
                if m < 1 {
                    "now".into()
                } else if m < 60 {
                    format!("{m}m")
                } else if m < 1440 {
                    format!("{}h", m / 60)
                } else {
                    format!("{}d", m / 1440)
                }
            })
            .unwrap_or_default();
        let project = chat["cwd"]
            .as_str()
            .filter(|s| !s.starts_with("~/.zuse/scratch/"))
            .and_then(|s| s.trim_end_matches('/').rsplit('/').next())
            .unwrap_or("No project");
        let harness = chat["config"]["harness"].as_str().unwrap_or("zuse");
        let projected = self
            .cloud
            .local_navigation
            .as_ref()
            .is_some_and(|navigation| navigation.chats.iter().any(|row| row["id"] == chat["id"]));
        let value = chat.clone();
        let context = chat.clone();
        let actions = chat.clone();
        column()
            .id(SharedString::from(format!(
                "chat-{}",
                chat["id"].as_str().unwrap_or_default()
            )))
            .role(Role::Button)
            .aria_label(SharedString::from(
                chat["title"].as_str().unwrap_or("New thread").to_owned(),
            ))
            .cursor_pointer()
            .px_2()
            .py_2()
            .gap_1()
            .rounded_lg()
            .when(nested, |d| d.ml_3())
            .when(selected, |d| d.bg(rgb(0x2a2a2a)))
            .hover(|d| d.bg(rgb(if selected { 0x303030 } else { 0x202020 })))
            .when(!nested, |d| {
                d.child(
                    row()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .text_size(px(10.))
                                .text_color(rgb(FAINT))
                                .child(format!("{project} · {}", self.chat_host_label(chat))),
                        )
                        .child(label(age.clone()).text_size(px(10.)).text_color(rgb(FAINT))),
                )
            })
            .child(
                row()
                    .gap_2()
                    .child(
                        icon(provider_icon(harness))
                            .size(px(14.))
                            .text_color(rgb(0xd79a72)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_ellipsis()
                            .text_color(rgb(if selected { TEXT } else { 0xd0d0d0 }))
                            .child(chat["title"].as_str().unwrap_or("New thread").to_owned()),
                    )
                    .when(!projected,|row|row.child(
                        icon_button(
                            SharedString::from(format!(
                                "thread-actions-{}",
                                chat["id"].as_str().unwrap_or_default()
                            )),
                            "more-horizontal",
                            "Thread actions",
                        )
                        .size(px(22.))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_thread_actions(actions.clone(), window, cx);
                            },
                        )),
                    ))
                    .when(active, |d| {
                        d.child(div().size(px(5.)).rounded_full().bg(rgb(
                            if status == "awaitingInput" {
                                0xe8b36e
                            } else {
                                0x89ca21
                            },
                        )))
                    })
                    .when(nested && !active, |d| {
                        d.child(label(age).text_size(px(9.)).text_color(rgb(FAINT)))
                    }),
            )
            .when(
                chat["branch"].as_str().is_some_and(|s| !s.is_empty()),
                |d| {
                    d.child(
                        row()
                            .pl(px(22.))
                            .gap_1()
                            .child(icon("git-branch").size(px(10.)).text_color(rgb(FAINT)))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(FAINT))
                                    .text_ellipsis()
                                    .child(chat["branch"].as_str().unwrap_or_default().to_owned()),
                            ),
                    )
                },
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    if !projected {this.open_thread_actions(context.clone(), window, cx);}
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                if projected {
                    if let Some(navigation)=&this.cloud.local_navigation {
                        let _=this.requests.unbounded_send(json!({"ui":"selectLocalConversation","chatId":value["id"],"deviceId":navigation.device,"epoch":navigation.epoch,"accountId":this.cloud.account,"organizationId":this.cloud.organization}));
                    }
                    return;
                }
                this.select(value.clone(), cx);
                if this
                    .selected
                    .as_ref()
                    .is_some_and(|c| c["id"] == value["id"])
                {
                    window.focus(&this.input.focus_handle(cx), cx);
                }
            }))
            .into_any_element()
    }
}

fn sidebar_action(
    id: &'static str,
    label: impl Into<SharedString>,
    glyph: &'static str,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .w_full()
        .min_h(px(24.))
        .px(px(8.))
        .py(px(2.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(7.))
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(rgb(MUTED))
        .cursor_pointer()
        .hover(|v| v.bg(rgb(RAISED)).text_color(rgb(TEXT)))
        .focus_visible(|v| v.border_1().border_color(rgb(0x89ca21)))
        .child(icon(glyph).size(px(16.)))
        .child(div().flex_1().min_w_0().truncate().child(label))
}
