//! Session navigation and floating agent controls for the browser workspace.
use super::*;
use crate::assets::icon;
use crate::shell::{
    BG, BORDER, FAINT, MUTED, RAISED, SURFACE, TEXT, column, control, icon_button, label,
    provider_icon, row,
};

impl WorkspaceView {
    pub(super) fn toggle_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_open = true;
        self.search_open = !self.search_open;
        if self.search_open {
            window.focus(&self.session_search.focus_handle(cx), cx);
        }
        cx.notify();
    }

    pub(super) fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let now = chrono::Utc::now();
        let current = self.selected.as_ref().map(|c| &c["id"]);
        let query = if self.search_open {
            self.session_search.read(cx).value(cx).to_lowercase()
        } else {
            String::new()
        };
        let rows = self
            .chats
            .iter()
            .filter(|c| c["archived"].as_bool().unwrap_or(false) == self.show_archive)
            .filter(|c| {
                query.is_empty()
                    || ["title", "cwd", "branch", "lastMessagePreview"]
                        .iter()
                        .any(|key| {
                            c[key]
                                .as_str()
                                .is_some_and(|s| s.to_lowercase().contains(&query))
                        })
            })
            .collect::<Vec<_>>();
        column()
            .w(px(296.))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(SURFACE))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                row()
                    .h(px(44.))
                    .px_3()
                    .gap_1()
                    .child(
                        icon_button("sidebar-collapse", "split-columns", "Hide session sidebar")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.sidebar_open = false;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        label("Zuse")
                            .text_size(px(12.))
                            .text_color(rgb(TEXT))
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button("search-sessions", "magnifer", "Search sessions (⌘K)")
                            .when(self.search_open, |d| d.bg(rgb(RAISED)))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.toggle_search(window, cx)),
                            ),
                    )
                    .child(
                        icon_button("new-session-icon", "plus", "New session")
                            .on_click(cx.listener(|this, _, _, cx| this.new_chat(cx))),
                    ),
            )
            .child(
                row().px_3().pt_2().pb_4().gap_2().child(
                    control("project-picker", "All projects")
                        .flex_1()
                        .min_w_0()
                        .text_color(rgb(TEXT))
                        .child(div().flex_1())
                        .child(icon("alt-arrow-down").size(px(12.)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.project_picker = !this.project_picker;
                            cx.notify();
                        })),
                ),
            )
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
                                cx.listener(|this, _, _, cx| {
                                    this.search_open = false;
                                    cx.notify();
                                }),
                            ),
                        ),
                )
            })
            .child(
                row()
                    .px_4()
                    .pb_2()
                    .gap_2()
                    .child(
                        label(if self.show_archive {
                            "Archived"
                        } else {
                            "Sessions"
                        })
                        .text_size(px(11.))
                        .text_color(rgb(FAINT)),
                    )
                    .child(
                        label(rows.len().to_string())
                            .text_size(px(10.))
                            .text_color(rgb(FAINT)),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button(
                            "archive-view",
                            "archive-minimalistic",
                            "Toggle archived sessions",
                        )
                        .size(px(22.))
                        .when(self.show_archive, |d| d.bg(rgb(RAISED)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_archive = !this.show_archive;
                            cx.notify();
                        })),
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
                    .when(rows.is_empty(), |d| {
                        d.child(
                            column()
                                .p_4()
                                .gap_2()
                                .child(
                                    label(if query.is_empty() {
                                        "No sessions yet"
                                    } else {
                                        "No matching sessions"
                                    })
                                    .text_color(rgb(MUTED)),
                                )
                                .child(
                                    label(if query.is_empty() {
                                        "Start a conversation with ⌘N"
                                    } else {
                                        "Try a title, project, or branch"
                                    })
                                    .text_size(px(11.))
                                    .text_color(rgb(FAINT)),
                                ),
                        )
                    })
                    .children(rows.into_iter().map(|chat| {
                        let selected = current.is_some_and(|id| id == &chat["id"]);
                        let session = self.sessions.iter().find(|s| s["chatId"] == chat["id"]);
                        let status = session
                            .and_then(|s| s["status"].as_str())
                            .unwrap_or_default();
                        let active = matches!(status, "working" | "awaitingInput");
                        let harness = chat["config"]["harness"].as_str().unwrap_or("zuse");
                        let age = chat["lastMessageAt"]
                            .as_str()
                            .or_else(|| chat["createdAt"].as_str())
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|at| {
                                let minutes = now.signed_duration_since(at).num_minutes().max(0);
                                if minutes < 1 {
                                    "now".to_owned()
                                } else if minutes < 60 {
                                    format!("{minutes}m")
                                } else if minutes < 1440 {
                                    format!("{}h", minutes / 60)
                                } else {
                                    format!("{}d", minutes / 1440)
                                }
                            });
                        let project = chat["cwd"]
                            .as_str()
                            .and_then(|s| s.trim_end_matches('/').rsplit('/').next())
                            .unwrap_or("No project");
                        let project = if chat["cwd"]
                            .as_str()
                            .is_some_and(|s| s.contains("/.zuse/scratch/"))
                        {
                            "Local workspace"
                        } else {
                            project
                        };
                        let chat = chat.clone();
                        column()
                            .id(SharedString::from(format!(
                                "chat-{}",
                                chat["id"].as_str().unwrap_or_default()
                            )))
                            .role(Role::Button)
                            .aria_label(SharedString::from(
                                chat["title"].as_str().unwrap_or("New session").to_owned(),
                            ))
                            .cursor_pointer()
                            .px_2()
                            .py_2()
                            .gap_1()
                            .rounded_lg()
                            .when(selected, |d| d.bg(rgb(0x2a2a2a)))
                            .hover(|d| d.bg(rgb(if selected { 0x303030 } else { 0x202020 })))
                            .child(
                                row()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .text_color(rgb(FAINT))
                                            .text_ellipsis()
                                            .child(format!("{project} @ This device")),
                                    )
                                    .when(!active, |d| {
                                        d.when_some(age, |d, age| {
                                            d.child(
                                                label(age)
                                                    .text_size(px(10.))
                                                    .text_color(rgb(FAINT)),
                                            )
                                        })
                                    })
                                    .when(active, |d| {
                                        d.child(
                                            row()
                                                .gap_1()
                                                .child(
                                                    div()
                                                        .size(px(4.))
                                                        .rounded_full()
                                                        .bg(rgb(0xd8a27d)),
                                                )
                                                .child(
                                                    label(if status == "awaitingInput" {
                                                        "Needs input"
                                                    } else {
                                                        "Working"
                                                    })
                                                    .text_size(px(10.))
                                                    .text_color(rgb(0xd8a27d)),
                                                ),
                                        )
                                    }),
                            )
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
                                            .text_size(px(13.))
                                            .text_ellipsis()
                                            .text_color(rgb(if selected { TEXT } else { 0xd0d0d0 }))
                                            .child(
                                                chat["title"]
                                                    .as_str()
                                                    .unwrap_or("New session")
                                                    .to_owned(),
                                            ),
                                    ),
                            )
                            .when(
                                chat["branch"].as_str().is_some_and(|s| !s.is_empty()),
                                |d| {
                                    d.child(
                                        row()
                                            .pl(px(22.))
                                            .gap_1()
                                            .child(
                                                icon("git-branch")
                                                    .size(px(11.))
                                                    .text_color(rgb(FAINT)),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(rgb(FAINT))
                                                    .text_ellipsis()
                                                    .child(
                                                        chat["branch"]
                                                            .as_str()
                                                            .unwrap_or_default()
                                                            .to_owned(),
                                                    ),
                                            ),
                                    )
                                },
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.select(chat.clone(), cx)),
                            )
                    })),
            )
            .child(
                row()
                    .p_4()
                    .gap_2()
                    .child(
                        div()
                            .size(px(28.))
                            .rounded_full()
                            .bg(rgb(0xe5e5e5))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.))
                            .text_color(rgb(BG))
                            .child("Z"),
                    )
                    .child(
                        column()
                            .flex_1()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Local workspace"),
                            )
                            .child(
                                row()
                                    .gap_1()
                                    .child(
                                        div().size(px(4.)).rounded_full().bg(rgb(
                                            if self.connected { 0x76b993 } else { 0xb79363 },
                                        )),
                                    )
                                    .child(
                                        label(if self.connected {
                                            "This device"
                                        } else {
                                            &self.status
                                        })
                                        .text_size(px(10.))
                                        .text_color(rgb(FAINT)),
                                    ),
                            ),
                    )
                    .child(
                        icon_button("settings", "settings-minimalistic", "Settings & usage")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings = !this.settings;
                                this.call(
                                    methods::LIST_AGENT_ACCOUNTS,
                                    json!({"forceUsage":false}),
                                );
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn composer_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let agents = self.agent_picker;
        column()
            .id("composer-picker")
            .absolute()
            .bottom_full()
            .mb_2()
            .right_0()
            .w(px(300.))
            .max_h(px(360.))
            .overflow_y_scroll()
            .rounded_xl()
            .border_1()
            .border_color(rgb(0x333333))
            .bg(rgb(0x1b1b1b))
            .shadow_lg()
            .p_2()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                row().px_2().py_2().child(
                    label(if agents {
                        "Choose agent"
                    } else {
                        "Choose model"
                    })
                    .text_size(px(11.))
                    .text_color(rgb(FAINT)),
                ),
            )
            .when(agents, |d| {
                d.children(self.providers.iter().enumerate().map(|(i, p)| {
                    let selected = self.provider_index == i;
                    row()
                        .id(("pick-agent", i))
                        .cursor_pointer()
                        .rounded_md()
                        .h(px(38.))
                        .px_2()
                        .gap_2()
                        .hover(|d| d.bg(rgb(0x292929)))
                        .child(
                            icon(provider_icon(p["id"].as_str().unwrap_or_default()))
                                .size(px(16.))
                                .text_color(rgb(0xd79a72)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(13.))
                                .child(p["name"].as_str().unwrap_or("Agent").to_owned()),
                        )
                        .when(selected, |d| {
                            d.child(icon("check").size(px(14.)).text_color(rgb(TEXT)))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.provider_index = i;
                            this.model = None;
                            this.models.clear();
                            this.agent_picker = false;
                            cx.notify();
                        }))
                }))
            })
            .when(!agents, |d| {
                d.child(
                    control("default-model", "Agent default")
                        .h(px(36.))
                        .text_color(rgb(TEXT))
                        .child(div().flex_1())
                        .when(self.model.is_none(), |d| {
                            d.child(icon("check").size(px(14.)))
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model = None;
                            this.model_picker = false;
                            cx.notify();
                        })),
                )
                .children(self.models.iter().enumerate().map(|(i, m)| {
                    let id = m["id"].as_str().unwrap_or_default().to_owned();
                    let selected = self.model.as_deref() == Some(&id);
                    control(
                        ("model", i),
                        SharedString::from(m["label"].as_str().unwrap_or(&id).to_owned()),
                    )
                    .h(px(36.))
                    .text_color(rgb(TEXT))
                    .child(div().flex_1())
                    .when(selected, |d| d.child(icon("check").size(px(14.))))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.model = Some(id.clone());
                        this.model_picker = false;
                        cx.notify();
                    }))
                }))
            })
            .into_any_element()
    }
}
