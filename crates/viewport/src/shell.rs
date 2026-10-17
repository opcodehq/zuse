//! Shared workspace presentation, following the native shell's density and surfaces.
use super::*;
use crate::assets::icon;
use zuse_markdown::{Block, InlineRun, parse_full};

const BG: u32 = 0x060606;
const SURFACE: u32 = 0x0d0d0d;
const RAISED: u32 = 0x181818;
const BORDER: u32 = 0x232323;
const TEXT: u32 = 0xebebeb;
const MUTED: u32 = 0xa3a3a3;
const FAINT: u32 = 0x686868;

pub(super) fn row() -> Div {
    div().flex().items_center()
}
pub(super) fn column() -> Div {
    div().flex().flex_col()
}
pub(super) fn label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .child(text.into())
}
pub(super) fn control(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Stateful<Div> {
    let name = name.into();
    row()
        .id(id)
        .role(Role::Button)
        .aria_label(name.clone())
        .cursor_pointer()
        .rounded_md()
        .h(px(30.))
        .px_2()
        .gap_2()
        .text_size(px(12.))
        .text_color(rgb(MUTED))
        .hover(|s| s.bg(rgb(0x202020)).text_color(rgb(TEXT)))
        .child(name)
}
fn icon_button(id: impl Into<ElementId>, glyph: &str, name: &'static str) -> Stateful<Div> {
    row()
        .id(id)
        .role(Role::Button)
        .aria_label(name)
        .cursor_pointer()
        .justify_center()
        .size(px(28.))
        .rounded_md()
        .text_color(rgb(MUTED))
        .hover(|s| s.bg(rgb(0x232323)).text_color(rgb(TEXT)))
        .child(icon(glyph))
}
fn provider_icon(id: &str) -> &'static str {
    match id {
        "claude-code" => "claude-mark",
        "codex" => "openai-mark",
        "cursor" => "cursor-mark",
        "opencode" => "opencode-mark",
        "pi" => "pi-mark",
        _ => "zuse-logo",
    }
}

impl WorkspaceView {
    fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.selected.as_ref().map(|c| c["id"].clone());
        column()
            .w(px(252.))
            .h_full()
            .flex_shrink_0()
            .bg(rgb(SURFACE))
            .border_r_1()
            .border_color(rgb(BORDER))
            .child(
                row()
                    .h(px(46.))
                    .px_4()
                    .gap_2()
                    .child(icon("zuse-logo").size(px(20.)).text_color(rgb(TEXT)))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_size(px(14.))
                            .child("Zuse"),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button("new-session-icon", "pen-new-square", "New session")
                            .on_click(cx.listener(|this, _, _, cx| this.new_chat(cx))),
                    ),
            )
            .child(
                column()
                    .px_2()
                    .gap_1()
                    .py_2()
                    .child(
                        control("new-session", "New session")
                            .child(div().flex_1())
                            .on_click(cx.listener(|this, _, _, cx| this.new_chat(cx))),
                    )
                    .child(
                        control("project-picker", "Open project")
                            .child(div().flex_1())
                            .child(icon("folder"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.project_picker = !this.project_picker;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                row()
                    .px_4()
                    .mt_4()
                    .mb_2()
                    .gap_2()
                    .child(icon("folder").size(px(14.)).text_color(rgb(FAINT)))
                    .child(label(self.project_name()))
                    .child(div().flex_1())
                    .child(
                        icon_button("archive-view", "archive-minimalistic", "Show archive")
                            .when(self.show_archive, |s| s.bg(rgb(RAISED)))
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
                    .px_2()
                    .overflow_y_scroll()
                    .gap_1()
                    .when(self.chats.is_empty(), |d| {
                        d.child(label("Your sessions will appear here").px_2().py_3())
                    })
                    .children(
                        self.chats
                            .iter()
                            .filter(|c| {
                                c["archived"].as_bool().unwrap_or(false) == self.show_archive
                            })
                            .map(|chat| {
                                let chat = chat.clone();
                                let selected = current.as_ref().is_some_and(|id| *id == chat["id"]);
                                let working = self
                                    .sessions
                                    .iter()
                                    .any(|s| s["chatId"] == chat["id"] && s["status"] == "working");
                                column()
                                    .id(SharedString::from(format!(
                                        "chat-{}",
                                        chat["id"].as_str().unwrap_or_default()
                                    )))
                                    .role(Role::Button)
                                    .aria_label(SharedString::from(
                                        chat["title"]
                                            .as_str()
                                            .unwrap_or("Untitled session")
                                            .to_owned(),
                                    ))
                                    .cursor_pointer()
                                    .px_3()
                                    .py_2()
                                    .gap_1()
                                    .rounded_lg()
                                    .when(selected, |s| s.bg(rgb(0x242424)))
                                    .hover(|s| s.bg(rgb(0x1b1b1b)))
                                    .child(
                                        row()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_size(px(12.))
                                                    .text_ellipsis()
                                                    .child(
                                                        chat["title"]
                                                            .as_str()
                                                            .unwrap_or("Untitled session")
                                                            .to_owned(),
                                                    ),
                                            )
                                            .when(working, |d| {
                                                d.child(
                                                    div()
                                                        .size(px(5.))
                                                        .rounded_full()
                                                        .bg(rgb(0xd79a72)),
                                                )
                                            }),
                                    )
                                    .child(
                                        row()
                                            .gap_1()
                                            .child(
                                                icon("git-branch")
                                                    .size(px(12.))
                                                    .text_color(rgb(FAINT)),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(rgb(FAINT))
                                                    .text_ellipsis()
                                                    .child(
                                                        chat["branch"]
                                                            .as_str()
                                                            .unwrap_or("Local checkout")
                                                            .to_owned(),
                                                    ),
                                            ),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select(chat.clone(), cx)
                                    }))
                            }),
                    ),
            )
            .child(
                column()
                    .border_t_1()
                    .border_color(rgb(BORDER))
                    .p_2()
                    .gap_1()
                    .child(
                        control("settings", "Settings & usage")
                            .child(div().flex_1())
                            .child(icon("settings-minimalistic"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.settings = !this.settings;
                                this.call(
                                    methods::LIST_AGENT_ACCOUNTS,
                                    json!({"forceUsage":false}),
                                );
                                cx.notify();
                            })),
                    )
                    .child(
                        row()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .child(div().size(px(5.)).rounded_full().bg(rgb(
                                if self.status == "Local engine connected" {
                                    0x769d7c
                                } else {
                                    0xb79363
                                },
                            )))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(rgb(FAINT))
                                    .child(self.status.clone()),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        row()
            .h(px(46.))
            .flex_shrink_0()
            .px_3()
            .border_b_1()
            .border_color(rgb(BORDER))
            .gap_2()
            .child(
                icon(provider_icon(
                    self.providers
                        .get(self.provider_index)
                        .and_then(|p| p["id"].as_str())
                        .unwrap_or_default(),
                ))
                .size(px(15.))
                .text_color(rgb(0xd38c6d)),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_ellipsis()
                    .child(
                        self.selected
                            .as_ref()
                            .and_then(|c| c["title"].as_str())
                            .unwrap_or("New session")
                            .to_owned(),
                    ),
            )
            .child(div().flex_1())
            .when(self.selected.is_some(), |d| {
                d.child(
                    icon_button("archive-session", "archive-minimalistic", "Archive session")
                        .on_click(cx.listener(|this, _, _, cx| this.archive(cx))),
                )
            })
            .child(
                icon_button(
                    "toggle-workspace",
                    "split-columns",
                    "Toggle workspace panel",
                )
                .when(self.right_open, |s| s.bg(rgb(RAISED)))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.right_open = !this.right_open;
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    fn composer(&self, cx: &mut Context<Self>) -> AnyElement {
        let provider = self.providers.get(self.provider_index);
        let agent = provider
            .and_then(|p| p["name"].as_str())
            .unwrap_or("Choose agent")
            .to_owned();
        let model = self.model.clone().unwrap_or_else(|| "Default model".into());
        let working = self.is_working();
        column()
            .px_5()
            .pb_2()
            .gap_2()
            .when(!self.queue.is_empty(), |d| d.child(self.queue_view(cx)))
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    row()
                        .px_3()
                        .py_2()
                        .gap_2()
                        .rounded_md()
                        .bg(rgb(0x251715))
                        .text_size(px(12.))
                        .text_color(rgb(0xe6a094))
                        .child(error)
                        .child(div().flex_1())
                        .child(
                            icon_button("dismiss-error", "close", "Dismiss error").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.error = None;
                                    cx.notify();
                                }),
                            ),
                        ),
                )
            })
            .when(working, |d| {
                d.child(
                    row()
                        .px_2()
                        .gap_2()
                        .child(div().size(px(5.)).rounded_full().bg(rgb(0xc59075)))
                        .child(label("Working…"))
                        .child(div().flex_1())
                        .child(label("New messages join the queue")),
                )
            })
            .child(
                column()
                    .rounded_xl()
                    .border_1()
                    .border_color(rgb(0x292929))
                    .bg(rgb(0x101010))
                    .p_3()
                    .gap_2()
                    .when(!self.attachments.is_empty() || self.uploading > 0, |d| {
                        d.child(
                            row()
                                .flex_wrap()
                                .gap_2()
                                .children(self.attachments.iter().enumerate().map(|(i, a)| {
                                    row()
                                        .id(("attachment", i))
                                        .px_2()
                                        .py_1()
                                        .gap_2()
                                        .rounded_md()
                                        .bg(rgb(RAISED))
                                        .child(
                                            icon(
                                                if a["mime"]
                                                    .as_str()
                                                    .is_some_and(|s| s.starts_with("image/"))
                                                {
                                                    "file-code"
                                                } else {
                                                    "paperclip"
                                                },
                                            )
                                            .size(px(12.)),
                                        )
                                        .child(label(
                                            a["name"].as_str().unwrap_or("Attachment").to_owned(),
                                        ))
                                        .child(
                                            icon_button(
                                                ("remove-attachment", i),
                                                "close",
                                                "Remove attachment",
                                            )
                                            .on_click(
                                                cx.listener(move |this, _, _, cx| {
                                                    this.attachments.remove(i);
                                                    cx.notify();
                                                }),
                                            ),
                                        )
                                }))
                                .when(self.uploading > 0, |d| {
                                    d.child(label(format!("Uploading {}…", self.uploading)))
                                }),
                        )
                    })
                    .child(div().w_full().min_h(px(36.)).child(self.input.clone()))
                    .child(
                        row()
                            .gap_1()
                            .child(
                                icon_button("attach", "paperclip", "Attach images or files")
                                    .on_click(cx.listener(|this, _, _, _| this.pick_attachments())),
                            )
                            .child(div().flex_1())
                            .child(control("agent-picker", agent).on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.settings = true;
                                    cx.notify();
                                },
                            )))
                            .child(
                                control("model-picker", SharedString::from(model))
                                    .child(icon("alt-arrow-down").size(px(12.)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.model_picker = !this.model_picker;
                                        if let Some(p) = this.providers.get(this.provider_index) {
                                            this.call(
                                                methods::LIST_MODELS,
                                                json!({"harness":p["id"]}),
                                            );
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(working, |d| {
                                d.child(
                                    icon_button("stop", "stop", "Stop agent")
                                        .bg(rgb(0xdddddd))
                                        .text_color(rgb(BG))
                                        .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx))),
                                )
                            })
                            .child(
                                icon_button(
                                    "send",
                                    if working { "queue-send" } else { "arrow-up" },
                                    if working {
                                        "Queue message"
                                    } else {
                                        "Send message"
                                    },
                                )
                                .rounded_full()
                                .size(px(30.))
                                .bg(rgb(TEXT))
                                .text_color(rgb(BG))
                                .on_click(cx.listener(
                                    |this, _, window, cx| this.submit(&Submit, window, cx),
                                )),
                            ),
                    ),
            )
            .when(self.model_picker, |d| {
                d.child(
                    column()
                        .max_h(px(220.))
                        .id("models-list")
                        .overflow_y_scroll()
                        .bg(rgb(RAISED))
                        .rounded_lg()
                        .p_2()
                        .gap_1()
                        .child(
                            control("default-model", "Default model").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.model = None;
                                    this.model_picker = false;
                                    cx.notify();
                                },
                            )),
                        )
                        .children(self.models.iter().enumerate().map(|(i, m)| {
                            let id = m["id"].as_str().unwrap_or_default().to_owned();
                            control(
                                ("model", i),
                                SharedString::from(m["label"].as_str().unwrap_or(&id).to_owned()),
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.model = Some(id.clone());
                                    this.model_picker = false;
                                    cx.notify();
                                },
                            ))
                        })),
                )
            })
            .child(
                row()
                    .h(px(24.))
                    .px_1()
                    .gap_2()
                    .child(icon("folder").size(px(13.)).text_color(rgb(FAINT)))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(FAINT))
                            .text_ellipsis()
                            .child(self.project_name()),
                    )
                    .child(div().flex_1())
                    .child(icon("git-branch").size(px(12.)).text_color(rgb(FAINT)))
                    .child(
                        div().text_size(px(10.)).text_color(rgb(FAINT)).child(
                            self.selected
                                .as_ref()
                                .and_then(|c| c["branch"].as_str())
                                .unwrap_or("Local checkout")
                                .to_owned(),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn queue_view(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .p_2()
            .gap_1()
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .child(
                row()
                    .px_1()
                    .gap_2()
                    .child(label(format!("Queued · {}", self.queue.len())))
                    .child(div().flex_1())
                    .child(label("After this turn")),
            )
            .children(self.queue.iter().enumerate().map(|(i, item)| {
                let item = item.clone();
                let remove = item.clone();
                let send = item.clone();
                let up = item.clone();
                row()
                    .gap_1()
                    .px_1()
                    .py_1()
                    .child(
                        icon("queue-drag-handle")
                            .size(px(12.))
                            .text_color(rgb(FAINT)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_ellipsis()
                            .child(item["text"].as_str().unwrap_or_default().to_owned()),
                    )
                    .when(i > 0, |d| {
                        d.child(
                            icon_button(("queue-up", i), "arrow-up", "Move earlier").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.queue_action(
                                        methods::MOVE_QUEUED_MESSAGE,
                                        &up,
                                        Some(i - 1),
                                        cx,
                                    )
                                }),
                            ),
                        )
                    })
                    .child(control(("steer", i), "Steer").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.queue_action(methods::STEER_QUEUED_MESSAGE_NOW, &item, None, cx)
                        },
                    )))
                    .child(
                        icon_button(("send-now", i), "queue-send", "Interrupt and send now")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.queue_action(methods::SEND_QUEUED_MESSAGE_NOW, &send, None, cx)
                            })),
                    )
                    .child(
                        icon_button(("remove-queued", i), "close", "Remove queued message")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.queue_action(methods::REMOVE_QUEUED_MESSAGE, &remove, None, cx)
                            })),
                    )
            }))
            .into_any_element()
    }

    fn transcript(&self) -> AnyElement {
        column()
            .id("transcript")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_6()
            .py_6()
            .gap_6()
            .when(self.messages.is_empty(), |d| {
                d.child(
                    column()
                        .flex_1()
                        .justify_center()
                        .items_center()
                        .gap_3()
                        .child(icon("zuse-logo").size(px(32.)).text_color(rgb(0xcccccc)))
                        .child(
                            div()
                                .text_size(px(21.))
                                .font_weight(FontWeight::MEDIUM)
                                .child("Where should we start?"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(rgb(FAINT))
                                .child("Choose a project, or start a local session."),
                        ),
                )
            })
            .children(self.messages.iter().enumerate().map(|(i, entry)| {
                let message = serde_json::to_value(entry).unwrap_or(Value::Null);
                let user = message["role"] == "user";
                column().id(("message", i)).w_full().gap_3().children(
                    message["parts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|part| {
                            let kind = part["kind"].as_str().unwrap_or_default();
                            match kind {
                                "text" => {
                                    let text = part["text"].as_str().unwrap_or_default();
                                    Some(if user {
                                        row()
                                            .justify_end()
                                            .child(
                                                div()
                                                    .max_w(relative(0.9))
                                                    .px_4()
                                                    .py_3()
                                                    .rounded_xl()
                                                    .bg(rgb(0x202020))
                                                    .text_size(px(13.))
                                                    .line_height(px(21.))
                                                    .child(text.to_owned()),
                                            )
                                            .into_any_element()
                                    } else {
                                        column()
                                            .text_size(px(13.))
                                            .line_height(px(22.))
                                            .gap_3()
                                            .children(
                                                parse_full(text)
                                                    .blocks
                                                    .iter()
                                                    .map(|b| markdown_block(&b.block)),
                                            )
                                            .into_any_element()
                                    })
                                }
                                "tool" => Some(
                                    row()
                                        .gap_2()
                                        .text_color(rgb(MUTED))
                                        .child(icon("terminal").size(px(13.)))
                                        .child(label(tool_label(part)))
                                        .into_any_element(),
                                ),
                                "reasoning" => Some(
                                    row()
                                        .gap_2()
                                        .text_color(rgb(FAINT))
                                        .child(icon("alt-arrow-right").size(px(12.)))
                                        .child(label("Thought process"))
                                        .into_any_element(),
                                ),
                                "image" => Some(
                                    row()
                                        .gap_2()
                                        .child(icon("paperclip"))
                                        .child(label(
                                            part["name"]
                                                .as_str()
                                                .unwrap_or("Image attachment")
                                                .to_owned(),
                                        ))
                                        .into_any_element(),
                                ),
                                _ => None,
                            }
                        }),
                )
            }))
            .into_any_element()
    }

    fn cloud_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let signed_in = self.cloud_account["user"].is_object();
        let tenant = self.plugins["tenantId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let busy = self.plugin_request.is_some();
        column().gap_3().child(label("Zuse account & plugins"))
            .child(row().gap_3()
                .child(column().flex_1().gap_1()
                    .child(div().text_size(px(13.)).child(self.cloud_account["user"]["email"].as_str().unwrap_or("Connect your Zuse account").to_owned()))
                    .child(label("Hosted connections are shared across your agents. Local agents work without signing in.")))
                .child(control("cloud-sign-in",if signed_in {"Sign out"} else {"Sign in"})

                    .on_click(cx.listener(move |this,_,_,cx| {this.call(if signed_in {methods::CLOUD_ACCOUNT_SIGN_OUT} else {methods::CLOUD_ACCOUNT_SIGN_IN},json!({}));cx.notify();}))))
            .when_some(self.cloud_account["error"].as_str().map(str::to_owned),|d,e|d.child(label(e)))
            .when_some(self.cloud_account["login"]["url"].as_str().filter(|u|u.starts_with("https://")).map(str::to_owned),|d,url| {
                d.child(row().gap_3().child(label(format!("Authorization code: {}",self.cloud_account["login"]["userCode"].as_str().unwrap_or_default())))
                    .child(control("authorize-account","Open sign-in").on_click(move |_,_,cx|cx.open_url(&url))))
            })
            .when(signed_in,|d|d
                .child(row().child(label("Connected plugins")).child(div().flex_1()).child(control("refresh-plugins",if busy {"Loading…"} else {"Refresh"}).on_click(cx.listener(|this,_,_,cx|{this.plugin_action(json!({"action":"list"}));cx.notify();}))))
                .children(self.plugins["tenants"].as_array().into_iter().flatten().enumerate().map(|(i,t)| {
                    let id=t["id"].clone();
                    control(("plugin-tenant",i),t["name"].as_str().unwrap_or("Workspace").to_owned()).on_click(cx.listener(move |this,_,_,cx|{this.plugin_action(json!({"action":"list","tenantId":id}));cx.notify();}))
                }))
                .children(self.plugins["connections"].as_array().into_iter().flatten().enumerate().map(|(i,c)| {
                    let id=c["id"].as_str().unwrap_or_default().to_owned();let tenant=tenant.clone();let enabled=c["enabled"]!=false;
                    row().gap_2().p_2().child(label(c["label"].as_str().unwrap_or("Plugin").to_owned())).child(label(c["state"].as_str().unwrap_or_default().to_owned())).child(div().flex_1())
                        .child(control(("toggle-plugin",i),if enabled {"Disable"} else {"Enable"}).on_click(cx.listener(move |this,_,_,cx|{this.plugin_action(json!({"action":"setEnabled","tenantId":tenant,"connectionId":id,"enabled":!enabled}));cx.notify();})))
                }))
                .children(self.plugins["catalog"].as_array().into_iter().flatten().enumerate().map(|(i,p)| {
                    let plugin=p["id"].as_str().unwrap_or_default().to_owned();let name=p["name"].as_str().unwrap_or("Plugin").to_owned();let tenant=tenant.clone();let title=name.clone();
                    row().gap_2().p_2().child(column().flex_1().gap_1().child(label(title)).child(label(p["description"].as_str().unwrap_or_default().to_owned())))
                        .child(control(("connect-plugin",i),"Connect").on_click(cx.listener(move |this,_,_,cx|{this.plugin_action(json!({"action":"connect","tenantId":tenant,"pluginId":plugin,"label":name,"requestId":uuid::Uuid::new_v4().to_string(),"returnTo":if this.browser_local {json!({"kind":"desktop","port":0})} else {json!({"kind":"web"})}}));cx.notify();})))
                })))
            .when_some(self.plugin_attempt["authorizationUrl"].as_str().filter(|u|u.starts_with("https://")).map(str::to_owned),|d,url|d.child(control("authorize-plugin","Authorize connection").on_click(move |_,_,cx|cx.open_url(&url))))
            .when(self.plugin_attempt["id"].is_string(),|d| {
                let attempt=self.plugin_attempt["id"].clone();let tenant=tenant.clone();
                d.child(row().gap_3().child(label(self.plugin_attempt["state"].as_str().unwrap_or("Connecting").to_owned())).child(control("poll-plugin","Check connection").on_click(cx.listener(move |this,_,_,cx|{this.plugin_action(json!({"action":"poll","tenantId":tenant,"attemptId":attempt}));cx.notify();}))))
            })
            .into_any_element()
    }

    fn settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .id("settings-body")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_6()
            .gap_5()
            .child(
                row()
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Settings"),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button("close-settings", "close", "Close settings").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.settings = false;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(self.cloud_settings(cx))
            .child(self.model_accounts_settings(cx))
            .child(label("Agents"))
            .children(self.providers.iter().enumerate().map(|(i, p)| {
                let installed = p["installed"] == true;
                row()
                    .id(("agent", i))
                    .cursor_pointer()
                    .p_3()
                    .gap_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .hover(|s| s.bg(rgb(RAISED)))
                    .child(
                        icon(provider_icon(p["id"].as_str().unwrap_or_default()))
                            .text_color(rgb(MUTED)),
                    )
                    .child(
                        column()
                            .flex_1()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .child(p["name"].as_str().unwrap_or("Agent").to_owned()),
                            )
                            .child(label(if installed {
                                "Installed on this machine"
                            } else {
                                "CLI installation required"
                            })),
                    )
                    .when(i == self.provider_index, |d| {
                        d.child(icon("check").text_color(rgb(TEXT)))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_provider(i, cx)))
            }))
            .child(
                row()
                    .mt_4()
                    .child(label("Accounts & usage"))
                    .child(div().flex_1())
                    .child(
                        icon_button("refresh-usage", "refresh", "Refresh usage").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.call(methods::LIST_AGENT_ACCOUNTS, json!({"forceUsage":true}));
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .children(
                self.accounts["accounts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|a| {
                        column()
                            .p_3()
                            .gap_3()
                            .rounded_lg()
                            .border_1()
                            .border_color(rgb(BORDER))
                            .child(
                                row()
                                    .gap_2()
                                    .child(icon(provider_icon(
                                        a["harness"].as_str().unwrap_or_default(),
                                    )))
                                    .child(label(
                                        a["email"]
                                            .as_str()
                                            .or_else(|| a["displayName"].as_str())
                                            .unwrap_or("CLI account")
                                            .to_owned(),
                                    ))
                                    .child(div().flex_1())
                                    .child(label(
                                        a["planLabel"].as_str().unwrap_or_default().to_owned(),
                                    )),
                            )
                            .children(a["usageWindows"].as_array().into_iter().flatten().map(|w| {
                                let fraction =
                                    w["usedFraction"].as_f64().unwrap_or(0.).clamp(0., 1.) as f32;
                                column()
                                    .gap_1()
                                    .child(
                                        row()
                                            .child(label(
                                                w["label"].as_str().unwrap_or("Usage").to_owned(),
                                            ))
                                            .child(div().flex_1())
                                            .child(label(format!("{:.0}%", fraction * 100.))),
                                    )
                                    .child(
                                        div().h(px(3.)).rounded_full().bg(rgb(BORDER)).child(
                                            div()
                                                .h_full()
                                                .w(relative(fraction))
                                                .rounded_full()
                                                .bg(rgb(0xb7a28c)),
                                        ),
                                    )
                            }))
                            .when_some(a["usageError"].as_str().map(str::to_owned), |d, e| {
                                d.child(label(e))
                            })
                    }),
            )
            .into_any_element()
    }

    fn project_picker_view(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .p_6()
            .gap_4()
            .flex_1()
            .child(
                row()
                    .child(div().text_size(px(20.)).child("Open a project"))
                    .child(div().flex_1())
                    .child(
                        icon_button("close-project-picker", "close", "Close project picker")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.project_picker = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(label(
                "Choose a known repository, or enter a path on the engine host.",
            ))
            .children(self.repos.iter().enumerate().map(|(i, p)| {
                let project = p.clone();
                column()
                    .id(("project", i))
                    .cursor_pointer()
                    .rounded_lg()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .p_3()
                    .gap_1()
                    .hover(|s| s.bg(rgb(RAISED)))
                    .child(
                        row()
                            .gap_2()
                            .child(icon("folder"))
                            .child(label(p["name"].as_str().unwrap_or("Project").to_owned())),
                    )
                    .child(label(p["path"].as_str().unwrap_or_default().to_owned()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.new_chat(cx);
                        this.project = Some(project.clone());
                        this.project_picker = false;
                        cx.notify();
                    }))
            }))
            .child(
                div()
                    .mt_3()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(rgb(BORDER))
                    .child(self.project_input.clone()),
            )
            .child(
                control("add-project", "Open folder")
                    .bg(rgb(RAISED))
                    .on_click(cx.listener(|this, _, _, cx| this.add_project(cx))),
            )
            .into_any_element()
    }

    fn workspace_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .w(px(if self.open_file.is_some() {
                480.
            } else if self.right_tab == "terminal" {
                360.
            } else {
                286.
            }))
            .flex_shrink_0()
            .h_full()
            .border_l_1()
            .border_color(rgb(BORDER))
            .bg(rgb(0x090909))
            .child(
                row()
                    .h(px(46.))
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .children(
                        [
                            ("files", "Files"),
                            ("changes", "Changes"),
                            ("terminal", "Terminal"),
                        ]
                        .into_iter()
                        .map(|(tab, name)| {
                            control(SharedString::from(format!("panel-{tab}")), name)
                                .when(self.right_tab == tab, |s| {
                                    s.text_color(rgb(TEXT)).bg(rgb(RAISED))
                                })
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.set_panel(tab, cx)),
                                )
                        }),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button("close-panel", "close", "Close workspace panel").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.right_open = false;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(match self.right_tab.as_str() {
                "terminal" => column()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("terminal-output")
                            .flex_1()
                            .overflow_y_scroll()
                            .p_3()
                            .text_size(px(11.))
                            .font_family("Geist Mono")
                            .line_height(px(17.))
                            .child(self.terminal_text.clone()),
                    )
                    .child(
                        row()
                            .p_2()
                            .gap_1()
                            .border_t_1()
                            .border_color(rgb(BORDER))
                            .child(div().flex_1().min_w_0().child(self.terminal_input.clone()))
                            .child(
                                icon_button("run-terminal", "arrow-up", "Run command").on_click(
                                    cx.listener(|this, _, _, cx| this.run_terminal_command(cx)),
                                ),
                            ),
                    )
                    .into_any_element(),
                "changes" => self.changes_view(),
                _ => self.files_view(cx),
            })
            .into_any_element()
    }

    fn files_view(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(file) = &self.open_file {
            return column()
                .flex_1()
                .min_h_0()
                .child(
                    row()
                        .px_2()
                        .h(px(36.))
                        .gap_2()
                        .border_b_1()
                        .border_color(rgb(BORDER))
                        .child(
                            icon_button("close-file", "arrow-left", "Back to files").on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.close_file(cx);
                                }),
                            ),
                        )
                        .child(label(file["path"].as_str().unwrap_or("File").to_owned())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .font_family("Geist Mono")
                        .child(self.file_editor.clone()),
                )
                .child(
                    row()
                        .h(px(36.))
                        .px_2()
                        .gap_2()
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .child(label(if self.file_dirty(cx) {
                            "Unsaved changes"
                        } else {
                            "Saved"
                        }))
                        .child(div().flex_1())
                        .when(self.file_dirty(cx), |d| {
                            d.child(control("discard-edits", "Discard").on_click(
                                cx.listener(|this, _, _, cx| this.discard_file_edits(cx)),
                            ))
                        })
                        .when(
                            !file["readOnlyReason"].is_string() && file["truncated"] != true,
                            |d| {
                                d.child(
                                    control(
                                        "save-file",
                                        if self.save_request.is_some() {
                                            "Saving…"
                                        } else {
                                            "Save"
                                        },
                                    )
                                    .bg(rgb(RAISED))
                                    .on_click(cx.listener(|this, _, _, cx| this.save_file(cx))),
                                )
                            },
                        ),
                )
                .into_any_element();
        }
        column()
            .flex_1()
            .min_h_0()
            .child(
                row()
                    .h(px(36.))
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(icon("folder").size(px(13.)).text_color(rgb(FAINT)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .text_ellipsis()
                            .child(if self.directory.is_empty() {
                                self.project_name()
                            } else {
                                self.directory.clone()
                            }),
                    )
                    .when(!self.directory.is_empty(), |d| {
                        d.child(
                            icon_button("directory-up", "arrow-left", "Parent folder").on_click(
                                cx.listener(|this, _, _, cx| {
                                    let parent = this
                                        .directory
                                        .rsplit_once('/')
                                        .map(|(p, _)| p)
                                        .unwrap_or("")
                                        .to_owned();
                                    this.load_directory(&parent);
                                    cx.notify();
                                }),
                            ),
                        )
                    })
                    .child(
                        icon_button("refresh-files", "refresh", "Refresh files").on_click(
                            cx.listener(|this, _, _, cx| {
                                let dir = this.directory.clone();
                                this.load_directory(&dir);
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .child(
                column()
                    .id("files-tree")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_2()
                    .gap_1()
                    .when(self.entries.is_empty(), |d| {
                        d.child(
                            column()
                                .p_4()
                                .gap_2()
                                .child(label(if self.selected.is_some() {
                                    "No files in this folder"
                                } else {
                                    "Open a session to browse its files"
                                })),
                        )
                    })
                    .children(self.entries.iter().enumerate().map(|(i, entry)| {
                        let entry = entry.clone();
                        row()
                            .id(("file-entry", i))
                            .h(px(28.))
                            .px_2()
                            .gap_2()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(RAISED)))
                            .child(
                                icon(if entry["kind"] == "directory" {
                                    "folder"
                                } else {
                                    "file-code"
                                })
                                .size(px(14.))
                                .text_color(rgb(FAINT)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .text_ellipsis()
                                    .child(entry["name"].as_str().unwrap_or_default().to_owned()),
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.open_entry(entry.clone(), cx)
                                }),
                            )
                    })),
            )
            .into_any_element()
    }

    fn changes_view(&self) -> AnyElement {
        let files = self.changes["files"].as_array();
        column()
            .id("changes-view")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .child(label("Working tree changes"))
            .when(files.is_none_or(Vec::is_empty), |d| {
                d.child(label("No changes to show").py_4())
            })
            .children(files.into_iter().flatten().map(|f| {
                column()
                    .py_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        row()
                            .gap_2()
                            .child(icon("file-code").size(px(13.)))
                            .child(label(
                                f["path"].as_str().unwrap_or("Changed file").to_owned(),
                            )),
                    )
                    .when_some(f["diff"].as_str().map(str::to_owned), |d, text| {
                        d.child(
                            div()
                                .font_family("Geist Mono")
                                .text_size(px(10.))
                                .text_color(rgb(MUTED))
                                .child(text),
                        )
                    })
            }))
            .into_any_element()
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Workspace")
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(|this, _: &SaveFile, _, cx| this.save_file(cx)))
            .size_full()
            .flex()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Geist")
            .text_size(px(13.))
            .child(self.sidebar(cx))
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(self.header(cx))
                    .child(if self.project_picker {
                        self.project_picker_view(cx)
                    } else if self.settings {
                        self.settings_view(cx)
                    } else {
                        column()
                            .flex_1()
                            .min_h_0()
                            .child(self.transcript())
                            .child(self.composer(cx))
                            .into_any_element()
                    }),
            )
            .when(
                self.right_open && !self.settings && !self.project_picker,
                |d| d.child(self.workspace_panel(cx)),
            )
    }
}

fn tool_label(part: &Value) -> String {
    let call = &part["call"];
    let name = call["title"]
        .as_str()
        .or_else(|| call["name"].as_str())
        .or_else(|| call["kind"].as_str())
        .unwrap_or("Tool call");
    let status = if part["isError"] == true {
        "Failed"
    } else if part["resolved"] == true {
        "Completed"
    } else {
        "Running"
    };
    format!("{name} · {status}")
}

fn inline(runs: &[InlineRun]) -> AnyElement {
    let mut text = String::new();
    let mut highlights = vec![];
    for run in runs {
        let start = text.len();
        text.push_str(&run.text);
        let mut style = HighlightStyle::default();
        if run.style.bold {
            style.font_weight = Some(FontWeight::SEMIBOLD);
        }
        if run.style.italic {
            style.font_style = Some(FontStyle::Italic);
        }
        if run.style.code {
            style.color = Some(rgb(0xb7a2d4).into());
            style.background_color = Some(rgb(0x201b29).into());
        }
        if run.style.link.is_some() {
            style.color = Some(rgb(0x9eb8dd).into());
        }
        highlights.push((start..text.len(), style));
    }
    StyledText::new(text)
        .with_highlights(highlights)
        .into_any_element()
}

fn markdown_block(block: &Block) -> AnyElement {
    match block {
        Block::Paragraph { runs } => div().child(inline(runs)).into_any_element(),
        Block::Heading { level, runs } => div()
            .text_size(px(match level {
                1 => 21.,
                2 => 18.,
                _ => 15.,
            }))
            .font_weight(FontWeight::SEMIBOLD)
            .child(inline(runs))
            .into_any_element(),
        Block::CodeBlock { language, code } => column()
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .overflow_hidden()
            .when_some(language.clone(), |d, l| {
                d.child(label(l).px_3().py_1().bg(rgb(RAISED)))
            })
            .child(
                div()
                    .p_3()
                    .bg(rgb(0x101010))
                    .text_size(px(11.))
                    .font_family("Geist Mono")
                    .line_height(px(19.))
                    .child(code.clone()),
            )
            .into_any_element(),
        Block::BlockQuote { children } => column()
            .border_l_2()
            .border_color(rgb(0x444444))
            .pl_3()
            .text_color(rgb(MUTED))
            .children(children.iter().map(markdown_block))
            .into_any_element(),
        Block::List {
            ordered_start,
            items,
        } => column()
            .gap_2()
            .children(items.iter().enumerate().map(|(i, blocks)| {
                row()
                    .items_start()
                    .gap_2()
                    .child(
                        div().w(px(18.)).text_color(rgb(MUTED)).child(
                            ordered_start
                                .map(|n| format!("{}.", n + i as u64))
                                .unwrap_or_else(|| "•".into()),
                        ),
                    )
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .gap_1()
                            .children(blocks.iter().map(markdown_block)),
                    )
            }))
            .into_any_element(),
        Block::Table { header, rows, .. } => column()
            .border_1()
            .border_color(rgb(BORDER))
            .rounded_md()
            .child(row().bg(rgb(RAISED)).children(header.iter().map(|c| {
                div()
                    .flex_1()
                    .min_w_0()
                    .p_2()
                    .font_weight(FontWeight::MEDIUM)
                    .child(inline(c))
            })))
            .children(rows.iter().map(|r| {
                row().border_t_1().border_color(rgb(BORDER)).children(
                    r.iter()
                        .map(|c| div().flex_1().min_w_0().p_2().child(inline(c))),
                )
            }))
            .into_any_element(),
        Block::Rule => div().h(px(1.)).bg(rgb(BORDER)).my_2().into_any_element(),
    }
}
