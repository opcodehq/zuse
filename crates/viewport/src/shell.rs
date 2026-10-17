//! Shared workspace presentation, following the native shell's density and surfaces.
use super::*;
use crate::assets::icon;
use zuse_markdown::{Block, InlineRun, parse_full};

pub(super) const BG: u32 = 0x0b0b0b;
pub(super) const SURFACE: u32 = 0x171717;
pub(super) const RAISED: u32 = 0x181818;
pub(super) const BORDER: u32 = 0x242424;
pub(super) const TEXT: u32 = 0xebebeb;
pub(super) const MUTED: u32 = 0xa3a3a3;
pub(super) const FAINT: u32 = 0x777777;

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
pub(super) fn icon_button(
    id: impl Into<ElementId>,
    glyph: &str,
    name: &'static str,
) -> Stateful<Div> {
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
pub(super) fn provider_icon(id: &str) -> &'static str {
    match id {
        "claude-code" => "claude-mark",
        "codex" => "openai-mark",
        "cursor" => "cursor-mark",
        "opencode" => "opencode-mark",
        "pi" => "pi-mark",
        "devin" => "devin-mark",
        "grok" => "grok-mark",
        "hermes" => "hermes-mark",
        "antigravity" => "antigravity-mark",
        "gemini" | "kiro" => "globe",
        _ => "zuse-logo",
    }
}

impl WorkspaceView {
    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        row()
            .h(px(44.))
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .child(
                icon_button("toggle-sidebar", "split-columns", "Toggle session sidebar").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.sidebar_open = !this.sidebar_open;
                        cx.notify();
                    }),
                ),
            )
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
                    .text_size(px(13.))
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
            .when(self.content_width > 660. && self.selected.is_some(), |d| {
                d.child(
                    label(format!("{} · This device", self.project_name()))
                        .text_size(px(11.))
                        .text_color(rgb(FAINT))
                        .text_ellipsis(),
                )
            })
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
        let working = self.is_working();
        let has_draft =
            !self.input.read(cx).value(cx).trim().is_empty() || !self.attachments.is_empty();
        column()
            .w_full()
            .max_w(px(896.))
            .self_center()
            .flex_shrink_0()
            .px(px(if self.content_width < 500. { 12. } else { 24. }))
            .pb_3()
            .pt_2()
            .when(self.has_composer_notices(), |d| {
                d.child(
                    column()
                        .w(relative(14. / 15.))
                        .self_center()
                        .rounded_t(px(19.))
                        .border_1()
                        .border_b_0()
                        .border_color(rgb(0x2b2d26))
                        .bg(rgb(0x1c1d19))
                        .child(self.composer_notices(cx)),
                )
            })
            .when(self.selected.is_some() || self.project.is_some(), |d| {
                d.child(
                    row()
                        .w(relative(14. / 15.))
                        .self_center()
                        .min_h(px(34.))
                        .px_2()
                        .gap_1()
                        .bg(rgb(0x191a17))
                        .border_1()
                        .border_b_0()
                        .border_color(rgb(0x2b2d26))
                        .when(!self.has_composer_notices(), |d| d.rounded_t(px(19.)))
                        .child(
                            control("composer-project", SharedString::from(self.project_name()))
                                .h(px(28.))
                                .text_size(px(11.))
                                .child(icon("alt-arrow-down").size(px(10.)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.project_picker = true;
                                    cx.notify();
                                })),
                        )
                        .when_some(
                            self.selected
                                .as_ref()
                                .and_then(|c| c["branch"].as_str())
                                .map(str::to_owned),
                            |d, branch| {
                                d.child(
                                    control("composer-branch", SharedString::from(branch))
                                        .h(px(28.))
                                        .text_size(px(11.))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.right_open = true;
                                            this.set_panel("changes", cx);
                                        })),
                                )
                            },
                        ),
                )
            })
            .child(
                column()
                    .relative()
                    .rounded(px(19.))
                    .border_1()
                    .border_color(rgb(0x30312b))
                    .bg(rgb(0x181916))
                    .shadow_sm()
                    .when(self.model_picker, |d| d.child(self.composer_picker(cx)))
                    .when(self.access_picker, |d| d.child(self.access_menu(cx)))
                    .when(self.add_menu, |d| d.child(self.composer_add_menu(cx)))
                    .when(!self.attachments.is_empty() || self.uploading > 0, |d| {
                        d.child(
                            row()
                                .flex_wrap()
                                .px_3()
                                .pt_3()
                                .gap_2()
                                .children(self.attachments.iter().enumerate().map(|(i, a)| {
                                    column()
                                        .gap_1()
                                        .when(
                                            a["path"].as_str().is_some_and(
                                                zuse_protocol::attachment_mentions::is_image_path,
                                            ),
                                            |d| {
                                                d.child(self.image_preview(
                                                    a["path"].as_str().unwrap_or_default(),
                                                    "",
                                                    112.,
                                                    cx,
                                                ))
                                            },
                                        )
                                        .child(
                                            row()
                                                .px_2()
                                                .py_1()
                                                .gap_2()
                                                .rounded_md()
                                                .bg(rgb(RAISED))
                                                .child(icon("paperclip").size(px(12.)))
                                                .child(
                                                    label(
                                                        a["name"]
                                                            .as_str()
                                                            .unwrap_or("Attachment")
                                                            .to_owned(),
                                                    )
                                                    .text_size(px(11.)),
                                                )
                                                .child(
                                                    icon_button(
                                                        ("remove-attachment", i),
                                                        "close",
                                                        "Remove attachment",
                                                    )
                                                    .size(px(22.))
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.attachments.remove(i);
                                                        cx.notify();
                                                    })),
                                                ),
                                        )
                                }))
                                .when(self.uploading > 0, |d| {
                                    d.child(
                                        label(format!("Uploading {}…", self.uploading))
                                            .text_size(px(11.)),
                                    )
                                }),
                        )
                    })
                    .child(
                        div()
                            .w_full()
                            .px_3()
                            .pt_3()
                            .pb_2()
                            .min_h(px(58.))
                            .child(self.input.clone()),
                    )
                    .child(
                        row()
                            .relative()
                            .px_2()
                            .pb_2()
                            .gap_1()
                            .child(
                                icon_button("composer-add", "plus", "Add attachments or tools")
                                    .size(px(28.))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.add_menu = !this.add_menu;
                                        this.access_picker = false;
                                        this.model_picker = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                control(
                                    "agent-access",
                                    if self.content_width < 420. {
                                        "Access"
                                    } else if self.sandbox
                                        == zuse_protocol::SandboxLevel::DangerFullAccess
                                    {
                                        "Full access"
                                    } else {
                                        "Ask for approval"
                                    },
                                )
                                .h(px(28.))
                                .px_2()
                                .text_size(px(11.))
                                .when(
                                    self.sandbox == zuse_protocol::SandboxLevel::DangerFullAccess,
                                    |d| d.text_color(rgb(0xe2b876)),
                                )
                                .child(icon("alt-arrow-down").size(px(10.)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.access_picker = !this.access_picker;
                                        this.add_menu = false;
                                        this.model_picker = false;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(div().flex_1().min_w_0())
                            .child(
                                row()
                                    .id("model-picker")
                                    .role(Role::Button)
                                    .aria_label("Change model")
                                    .cursor_pointer()
                                    .h(px(28.))
                                    .w(px(if self.content_width < 500. {
                                        148.
                                    } else {
                                        176.
                                    }))
                                    .min_w_0()
                                    .px_2()
                                    .gap_2()
                                    .rounded_full()
                                    .bg(rgb(0x2b2d26))
                                    .hover(|d| d.bg(rgb(0x34372c)))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .child(
                                        icon(provider_icon(
                                            provider
                                                .and_then(|p| p["id"].as_str())
                                                .unwrap_or_default(),
                                        ))
                                        .size(px(13.))
                                        .text_color(rgb(MUTED)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_ellipsis()
                                            .text_size(px(11.))
                                            .text_color(rgb(TEXT))
                                            .child(self.picker_label()),
                                    )
                                    .child(
                                        icon("alt-arrow-down").size(px(10.)).text_color(rgb(FAINT)),
                                    )
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_model_picker(window, cx)
                                    })),
                            )
                            .when(working, |d| {
                                d.child(
                                    icon_button("stop", "stop", "Stop agent")
                                        .size(px(28.))
                                        .bg(rgb(0x33352d))
                                        .text_color(rgb(TEXT))
                                        .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx))),
                                )
                            })
                            .when(!working || has_draft, |d| {
                                d.child(
                                    crate::styling::dither_icon_action(
                                        "send",
                                        if working { "queue-send" } else { "arrow-up" },
                                        if working {
                                            "Queue message"
                                        } else {
                                            "Send message"
                                        },
                                        has_draft && self.connected,
                                        true,
                                    )
                                    .on_click(cx.listener(
                                        |this, _, window, cx| this.submit(&Submit, window, cx),
                                    )),
                                )
                            }),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn queue_view(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .px_2()
            .py_1()
            .gap_1()
            .border_b_1()
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

    fn transcript(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.messages.is_empty()
            && self.transcript_scroll.max_offset().y + self.transcript_scroll.offset().y <= px(24.)
        {
            self.transcript_scroll.scroll_to_bottom();
        }
        column()
            .id("transcript")
            .track_scroll(&self.transcript_scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_8()
            .pt_6()
            .pb_8()
            .gap_8()
            .when(self.messages.is_empty(), |d| {
                d.child(column().flex_1().justify_center().w_full().max_w(px(680.)).self_center().pb_8().gap_4()
                    .child(icon("zuse-logo").size(px(32.)).text_color(rgb(MUTED)))
                    .child(div().text_size(px(30.)).line_height(px(38.)).font_weight(FontWeight::MEDIUM).child("What would you like to build?"))
                    .child(div().text_size(px(13.)).line_height(px(22.)).text_color(rgb(MUTED))
                        .child("A fresh space for your next idea. Choose a project or start a conversation."))
                    .child(row().mt_4().gap_3()
                        .child(column().id("welcome-project").flex_1().min_w_0().p_4().gap_3().rounded_xl()
                            .border_1().border_color(rgb(BORDER)).cursor_pointer().hover(|d|d.bg(rgb(RAISED)))
                            .child(icon("folder").size(px(19.)).text_color(rgb(MUTED)))
                            .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child("Work on a project"))
                            .child(label("Open a folder on this machine").text_size(px(11.)))
                            .on_click(cx.listener(|this, _, _, cx|{this.project_picker = true; cx.notify();})))
                        .child(column().id("welcome-agents").flex_1().min_w_0().p_4().gap_3().rounded_xl()
                            .border_1().border_color(rgb(BORDER)).cursor_pointer().hover(|d|d.bg(rgb(RAISED)))
                            .child(icon("settings-minimalistic").size(px(19.)).text_color(rgb(MUTED)))
                            .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child("Connect your agents"))
                            .child(label("Models, accounts, and tools").text_size(px(11.)))
                            .on_click(cx.listener(|this, _, _, cx|{this.settings = true; cx.notify();}))))
                    .child(label("Your agents run on this machine.").mt_2().text_size(px(11.)).text_color(rgb(FAINT))))
            })
            .children(self.messages.iter().enumerate().map(|(i, entry)| {
                let message = serde_json::to_value(entry).unwrap_or(Value::Null);
                let user = message["role"] == "user";
                column()
                    .id(("message", i))
                    .group("message")
                    .relative()
                    .w_full().max_w(px(796.)).self_center()
                    .flex_shrink_0()
                    .gap_3()
                    .children(
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
                                            let parsed = zuse_protocol::message_attachments::parse(text);
                                            column().items_end().gap_3()
                                                .when(!parsed.text.is_empty(), |d|d.child(
                                                    div().max_w(relative(0.85)).px_4().py_3().rounded_xl()
                                                        .bg(rgb(0x1d1d1d)).text_size(px(14.)).line_height(px(21.))
                                                        .child(parsed.text)))
                                                .children(parsed.attachments.iter().map(|attachment| {
                                                    if zuse_protocol::attachment_mentions::is_image_path(&attachment.path) {
                                                        self.image_preview(&attachment.path,&attachment.name,384.,cx)
                                                    } else {
                                                        row().gap_2().px_3().py_2().rounded_lg().bg(rgb(RAISED))
                                                            .child(icon("paperclip").size(px(14.)))
                                                            .child(label(attachment.name.clone())).into_any_element()
                                                    }
                                                }))
                                                .into_any_element()
                                        } else {
                                            column()
                                                .text_size(px(14.))
                                                .line_height(px(24.))
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
                                    "error" => Some(
                                        column()
                                            .p_3()
                                            .rounded_lg()
                                            .bg(rgb(0x251715))
                                            .text_color(rgb(0xe6a094))
                                            .text_size(px(12.))
                                            .line_height(px(20.))
                                            .child(
                                                part["message"]
                                                    .as_str()
                                                    .unwrap_or("The agent reported an error.")
                                                    .to_owned(),
                                            )
                                            .into_any_element(),
                                    ),
                                    "input" => Some(self.input_card(part, cx)),
                                    "fork" => Some(
                                        row()
                                            .gap_2()
                                            .text_color(rgb(FAINT))
                                            .child(icon("split-columns").size(px(12.)))
                                            .child(label(format!(
                                                "Branched from {}",
                                                part["sourceTitle"]
                                                    .as_str()
                                                    .unwrap_or("conversation")
                                            )))
                                            .into_any_element(),
                                    ),
                                    "visual" => Some(
                                        row()
                                            .gap_2()
                                            .child(icon("file-code").size(px(13.)))
                                            .child(label(format!(
                                                "Interactive visual: {}",
                                                part["title"].as_str().unwrap_or("Visual")
                                            )))
                                            .into_any_element(),
                                    ),
                                    "tool" | "reasoning" => Some(self.activity_part(&entry.id,part,cx)),
                                    "image" => Some(self.image_preview(
                                        part["path"].as_str().unwrap_or_default(),
                                        part["name"].as_str().unwrap_or("Image attachment"),384.,cx)),
                                    _ => None,
                                }
                            }),
                    )
                    .child(row().absolute().left_0().right_0().bottom(px(-25.)).h(px(22.)).when(user,|d|d.justify_end()).child(
                        icon_button(("copy-message",i),"copy","Copy message")
                            .opacity(0.).group_hover("message",|d|d.opacity(1.))
                            .on_click({let text=entry.parts.iter().filter_map(|p|serde_json::to_value(p).ok()).filter_map(|p|p["text"].as_str().map(str::to_owned)).collect::<Vec<_>>().join("\n");move|_,_,cx|cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))})
                    ))
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
            .children(self.known_projects().iter().enumerate().map(|(i, p)| {
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
                400.
            } else if self.right_tab == "terminal" {
                360.
            } else {
                320.
            }))
            .flex_shrink_0()
            .h_full()
            .border_l_1()
            .border_color(rgb(BORDER))
            .bg(rgb(0x111111))
            .child(
                row()
                    .h(px(44.))
                    .px_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .children(
                        [
                            ("files", "Files"),
                            ("changes", "Changes"),
                            ("terminal", "Terminal"),
                            ("recovery", "Recovery"),
                        ]
                        .into_iter()
                        .map(|(tab, name)| {
                            control(SharedString::from(format!("panel-{tab}")), name)
                                .px_1()
                                .gap_1()
                                .text_size(px(11.))
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
                "recovery" => self.recovery_view(cx),
                "terminal" => column()
                    .when(self.terminal_id.is_none(), |d| {
                        d.child(
                            control("open-browser-terminal", "Open terminal")
                                .m_3()
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.set_panel("terminal", cx)),
                                ),
                        )
                    })
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
                "changes" => self.workspace_changes_view(cx),
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
                                cx.listener(|this, _, window, cx| {
                                    this.close_file(cx);
                                    if this.open_file.is_none() {
                                        window.focus(&this.input.read(cx).focus_handle(cx), cx);
                                    }
                                }),
                            ),
                        )
                        .child(
                            label(file["path"].as_str().unwrap_or("File").to_owned())
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis(),
                        )
                        .when(self.file_request.is_some(), |d| {
                            d.child(label("Loading…").text_size(px(10.)))
                        }),
                )
                .when_some(file["readOnlyReason"].as_str(), |d, reason| {
                    d.child(
                        label(read_only_message(reason))
                            .px_3()
                            .py_2()
                            .text_size(px(11.)),
                    )
                })
                .when(file["truncated"] == true, |d| {
                    d.child(
                        label("Partial file preview · Editing is disabled")
                            .px_3()
                            .py_2(),
                    )
                })
                .child(if file["text"].is_string() {
                    div()
                        .flex_1()
                        .min_h_0()
                        .font_family("Geist Mono")
                        .child(self.file_editor.clone())
                        .into_any_element()
                } else {
                    column()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .gap_3()
                        .child(icon("file-code").size(px(28.)))
                        .child(label(if file["encoding"] == "binary" {
                            "Binary file"
                        } else {
                            "Text preview unavailable"
                        }))
                        .child(
                            label(format!("{} bytes", file["size"].as_u64().unwrap_or(0)))
                                .text_size(px(11.)),
                        )
                        .into_any_element()
                })
                .child(
                    row()
                        .h(px(36.))
                        .px_2()
                        .gap_2()
                        .border_t_1()
                        .border_color(rgb(BORDER))
                        .child(label(if self.save_request.is_some() {
                            "Saving…"
                        } else if self.file_dirty(cx) {
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
                                    .when(
                                        !self.file_dirty(cx) || self.save_request.is_some(),
                                        |d| d.opacity(0.4),
                                    )
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
                    .flex_1()
                    .min_h_0()
                    .when(self.directory_request.is_some(), |d| {
                        d.child(label("Loading files…").px_3().py_2())
                    })
                    .when(
                        self.entries.is_empty() && self.directory_request.is_none(),
                        |d| {
                            d.child(
                                label(if self.selected.is_some() {
                                    "No files in this folder"
                                } else {
                                    "Open a session to browse its files"
                                })
                                .p_4(),
                            )
                        },
                    )
                    .child(
                        uniform_list(
                            "files-tree",
                            self.entries.len(),
                            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                                range
                                    .filter_map(|index| {
                                        this.entries.get(index).cloned().map(|entry| {
                                            row()
                                                .id(("file-entry", index))
                                                .h(px(28.))
                                                .px_2()
                                                .gap_2()
                                                .rounded_md()
                                                .cursor_pointer()
                                                .hover(|d| d.bg(rgb(RAISED)))
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
                                                        .child(
                                                            entry["name"]
                                                                .as_str()
                                                                .unwrap_or_default()
                                                                .to_owned(),
                                                        ),
                                                )
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.open_entry(entry.clone(), cx)
                                                }))
                                        })
                                    })
                                    .collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.files_scroll)
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .py_2(),
                    )
                    .when(self.directory_cursor.is_some(), |d| {
                        d.child(
                            control(
                                "load-more-files",
                                if self.directory_request.is_some() {
                                    "Loading…"
                                } else {
                                    "Load more files"
                                },
                            )
                            .m_2()
                            .on_click(cx.listener(|this, _, _, cx| this.load_more_directory(cx))),
                        )
                    }),
            )
            .into_any_element()
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let panel_width = if !self.right_open || self.settings || self.project_picker {
            0.
        } else if self.open_file.is_some() {
            400.
        } else if self.right_tab == "terminal" {
            360.
        } else {
            320.
        };
        self.content_height = f32::from(window.viewport_size().height);
        self.content_width = f32::from(window.viewport_size().width)
            - if self.sidebar_open { 296. } else { 0. }
            - panel_width;
        div()
            .track_focus(&self.workspace_focus)
            .key_context("Workspace")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.model_picker || this.agent_picker || this.access_picker || this.add_menu
                    {
                        this.model_picker = false;
                        this.agent_picker = false;
                        this.access_picker = false;
                        this.add_menu = false;
                        cx.notify();
                    }
                }),
            )
            .on_action(cx.listener(Self::submit))
            .on_action(
                cx.listener(|this, _: &SearchSessions, window, cx| this.toggle_search(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NewSession, window, cx| this.start_new_session(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ClosePreview, window, cx| {
                this.preview_open = None;
                this.model_picker = false;
                this.agent_picker = false;
                this.search_open = false;
                this.access_picker = false;
                this.add_menu = false;
                window.focus(&this.input.focus_handle(cx), cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SaveFile, _, cx| this.save_file(cx)))
            .size_full()
            .relative()
            .flex()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Geist")
            .text_size(px(13.))
            .when(self.sidebar_open, |d| d.child(self.sidebar(cx)))
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
                            .child(self.transcript(cx))
                            .child(self.composer(cx))
                            .into_any_element()
                    }),
            )
            .when(
                self.right_open && !self.settings && !self.project_picker,
                |d| d.child(self.workspace_panel(cx)),
            )
            .when(self.preview_open.is_some(), |d| {
                d.child(self.preview_overlay(window, cx))
            })
    }
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

fn read_only_message(reason: &str) -> &'static str {
    match reason {
        "binary" => "Read only · Binary file",
        "unsupportedEncoding" => "Read only · Unsupported text encoding",
        "mixedLineEndings" => "Read only · Mixed line endings",
        "symlink" => "Read only · Symbolic link",
        "tooLarge" => "Read only · File exceeds the editing limit",
        "permissionDenied" => "Read only · Permission denied",
        "outsideWorkspace" => "Read only · File is outside this workspace",
        _ => "Read only",
    }
}
