//! Shared workspace presentation, following the native shell's density and surfaces.
use super::*;
use crate::localization::message as tr;
use crate::{assets::icon, composer_style as composer};
use zuse_i18n::messages as msg;
use zuse_markdown::{Block, InlineRun, parse_full};

pub(super) const BG: u32 = 0x0b0c0b;
pub(super) const SURFACE: u32 = 0x0e0f0e;
pub(super) const RAISED: u32 = 0x191c1a;
pub(super) const BORDER: u32 = 0x1f211f;
pub(super) const TEXT: u32 = 0xe8e9e7;
pub(super) const MUTED: u32 = 0xa4a8a3;
pub(super) const FAINT: u32 = 0x6b716a;

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
    name: impl Into<SharedString>,
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
        "codex" => "codex-mark",
        "cursor" => "cursor-mark",
        "opencode" => "opencode-mark",
        "pi" => "pi-mark",
        "muse" => "muse-mark",
        "devin" => "devin-mark",
        "grok" => "grok-mark",
        "hermes" => "hermes-mark",
        "antigravity" => "antigravity-mark",
        "gemini" | "acp-gemini" => "gemini-mark",
        "kiro" => "kiro-mark",
        _ => "zuse-logo",
    }
}

impl WorkspaceView {
    pub(super) fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        row()
            .h(px(36.))
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
            .when(
                self.selected
                    .as_ref()
                    .is_some_and(|chat| chat["archived"] == true),
                |view| {
                    view.child(
                        icon_button(
                            "back-to-archives",
                            "alt-arrow-left",
                            crate::localization::text(
                                "chat-archived_chats_page_back_to_archived_chats",
                                cx,
                            ),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.settings = true;
                            this.select_settings_section(
                                crate::settings_view::Section::Archived,
                                cx,
                            );
                        })),
                    )
                },
            )
            .when(self.cloud.guest, |view| {
                view.child(
                    icon_button("leave-cloud", "alt-arrow-left", "Back to threads").on_click(
                        cx.listener(|this, _, _, _| {
                            let _ = this.requests.unbounded_send(json!({"ui":"leaveCloud"}));
                        }),
                    ),
                )
            })
            .child(icon("git-branch").size(px(14.)))
            .child(
                control(
                    "workspace-branch",
                    SharedString::from(
                        self.selected
                            .as_ref()
                            .and_then(|chat| chat["branch"].as_str())
                            .filter(|branch| !branch.is_empty())
                            .unwrap_or("New chat")
                            .to_owned(),
                    ),
                )
                .h(px(24.))
                .text_size(px(11.))
                .max_w(px(280.))
                .overflow_hidden()
                .child(icon("alt-arrow-down").size(px(10.)))
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.selected.is_some() {
                        this.right_open = true;
                        this.set_panel("changes", cx);
                        cx.notify();
                    }
                })),
            )
            .when_some(
                self.selected
                    .as_ref()
                    .and_then(|chat| chat["branch"].as_str())
                    .map(str::to_owned),
                |view, branch| {
                    view.child(
                        icon_button("copy-branch", "copy", "Copy branch")
                            .size(px(24.))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(branch.clone()))
                            }),
                    )
                },
            )
            .child(div().flex_1())
            .child(
                icon_button("voice-call", "microphone", "Open or hide voice call").on_click(
                    cx.listener(|this, _, _, cx| {
                        if this.browser_voice_live() {
                            this.toggle_browser_voice_stage(cx);
                        } else {
                            this.start_browser_voice(cx);
                        }
                    }),
                ),
            )
            .when(self.browser_voice_live(), |d| {
                d.child(
                    control("end-voice-compact", tr(cx, msg::native::end_voice))
                        .on_click(cx.listener(|this, _, _, cx| this.end_browser_voice(None, cx))),
                )
            })
            .when(self.selected.is_some(), |d| {
                d.child(
                    icon_button(
                        "archive-session",
                        "archive-minimalistic",
                        if self
                            .selected
                            .as_ref()
                            .is_some_and(|chat| chat["archived"] == true)
                        {
                            "Restore thread"
                        } else {
                            "Archive thread"
                        },
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.archive(cx))),
                )
            })
            .when_some(self.selected.clone(), |d, chat| {
                d.child(
                    icon_button("thread-actions", "more-horizontal", "Thread actions").on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.open_thread_actions(chat.clone(), window, cx);
                        }),
                    ),
                )
            })
            .when(self.cloud.guest, |view| {
                view.child(div().w(px(28.)).h(px(28.)))
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

    pub(super) fn composer(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.browser_voice_replaces_composer() {
            return self.browser_voice_stage(cx);
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|chat| chat["archived"] == true)
        {
            return self.archived_thread_footer(cx);
        }
        if let Some((request, questions)) = self.pending_composer_question() {
            return self.question_composer(&request, &questions, cx);
        }
        let provider = self.composer_provider();
        let working = self.is_working();
        let has_draft =
            !self.input.read(cx).value(cx).trim().is_empty() || !self.attachments.is_empty();
        let starting = self.cloud_startup_pending();
        column()
            .w_full()
            .max_w(px(if self.selected.is_none() && !starting {
                composer::LANDING_WIDTH.min(
                    (self.content_width
                        - 2. * if self.viewport_width <= 800. {
                            16.
                        } else {
                            24.
                        })
                    .max(0.),
                )
            } else {
                composer::CONVERSATION_WIDTH + 24.
            }))
            .min_w_0()
            .self_center()
            .flex_shrink_0()
            .px(px(12.))
            .pb(px(if self.selected.is_none() { 28. } else { 12. }))
            .pt_2()
            .when_some(self.voice.notice.clone(), |d, notice| {
                d.child(
                    row()
                        .gap_2()
                        .px_3()
                        .py_2()
                        .rounded_lg()
                        .bg(rgb(0x291719))
                        .child(label(notice).flex_1().min_w_0().text_color(rgb(0xe5a0a0)))
                        .child(
                            icon_button("dismiss-voice-notice", "close", "Dismiss voice message")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.voice.notice = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when(self.has_composer_notices(), |d| {
                d.child(
                    column()
                        .w(relative(14. / 15.))
                        .self_center()
                        .rounded_t(px(19.))
                        .bg(crate::palette::composer_toolbar())
                        .when(starting, |tray| {
                            tray.overflow_hidden()
                                .border_1()
                                .border_color(crate::palette::tone(
                                    gpui::white().opacity(0.06),
                                    crate::palette::composer_border(),
                                ))
                                .bg(crate::cloud_queue::background())
                        })
                        .child(self.composer_notices(cx)),
                )
            })
            .when(
                self.selected.is_none() && !self.cloud.guest && !starting,
                |view| view.child(self.cloud_toolbar(cx)),
            )
            .when(!self.queue.is_empty(), |view| {
                view.child(self.queue_view(cx))
            })
            .when(self.queue_edit.is_some(), |view| {
                view.child(self.queue_edit_bar(cx))
            })
            .child(
                column()
                    .w_full()
                    .min_w_0()
                    .relative()
                    .rounded(px(composer::RADIUS))
                    .border_1()
                    .border_color(crate::palette::composer_border())
                    .bg(crate::palette::composer_background())
                    .shadow_sm()
                    .when(self.model_picker, |d| {
                        d.child(self.composer_picker(cx, None))
                    })
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
                            .pt(px(composer::EDITOR_PAD_TOP))
                            .pb(px(composer::EDITOR_PAD_BOTTOM))
                            .min_h(px(composer::EDITOR_MIN
                                + composer::EDITOR_PAD_TOP
                                + composer::EDITOR_PAD_BOTTOM))
                            .child(self.input.clone()),
                    )
                    .child(
                        row()
                            .relative()
                            .px(px(10.))
                            .pt(px(4.))
                            .pb_2()
                            .gap_1()
                            .child(
                                icon_button("composer-add", "plus", "Add attachments or tools")
                                    .size(px(28.))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.context_menu = false;
                                        this.add_menu = !this.add_menu;
                                        this.access_picker = false;
                                        this.model_picker = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                row()
                                    .id("agent-access")
                                    .track_focus(&self.access_focus)
                                    .on_key_down(cx.listener(Self::on_access_key))
                                    .role(Role::ComboBox)
                                    .aria_label(tr(cx, msg::chat::chat_composer_agent_access))
                                    .aria_expanded(self.access_picker)
                                    .tab_index(0)
                                    .h(px(28.))
                                    .px(px(10.))
                                    .gap(px(6.))
                                    .rounded(px(6.))
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(
                                        if self.sandbox
                                            == zuse_protocol::SandboxLevel::DangerFullAccess
                                        {
                                            0xe2b876
                                        } else {
                                            MUTED
                                        },
                                    ))
                                    .cursor_pointer()
                                    .when(self.access_change_pending_for_selection(), |view| {
                                        view.opacity(0.6)
                                    })
                                    .when(self.access_picker, |view| view.bg(rgb(0x202020)))
                                    .hover(|view| view.bg(rgb(0x202020)))
                                    .child(
                                        svg()
                                            .path(
                                                zuse_protocol::presentation::access_details(
                                                    self.sandbox,
                                                )
                                                .icon,
                                            )
                                            .size(px(14.))
                                            .flex_none()
                                            .text_color(rgb(
                                                if self.sandbox
                                                    == zuse_protocol::SandboxLevel::DangerFullAccess
                                                {
                                                    0xe2b876
                                                } else {
                                                    MUTED
                                                },
                                            )),
                                    )
                                    .when(self.content_width >= 420., |view| {
                                        view.child(if self.access_change_pending_for_selection() {
                                            crate::localization::text(
                                                "settings-wallpaper_saving",
                                                cx,
                                            )
                                        } else {
                                            crate::localization::access_label(self.sandbox, cx)
                                        })
                                    })
                                    .child(icon("alt-arrow-down").size(px(12.)).opacity(0.6))
                                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                        cx.stop_propagation()
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_access(window, cx)
                                    })),
                            )
                            .child(div().flex_1().min_w_0())
                            .child(
                                row()
                                    .id("model-picker")
                                    .relative()
                                    .child({
                                        let bounds = self.picker_menu.trigger.clone();
                                        canvas(move |b, _, _| bounds.set(Some(b)), |_, _, _, _| {})
                                            .absolute()
                                            .left_0()
                                            .top_0()
                                            .size_full()
                                    })
                                    .role(Role::Button)
                                    .aria_label(tr(cx, msg::providers::model_picker_change_model))
                                    .cursor_pointer()
                                    .h(px(28.))
                                    .w(px((self.content_width * 0.4).clamp(104., 160.)))
                                    .min_w_0()
                                    .px(px(10.))
                                    .gap(px(6.))
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
                                        .size(px(12.))
                                        .text_color(rgb(MUTED)),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_ellipsis()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(TEXT))
                                            .child(
                                                if self.selected_model().is_none()
                                                    && self
                                                        .model
                                                        .as_deref()
                                                        .is_none_or(|m| m == "default")
                                                {
                                                    tr(cx, msg::native::agent_default)
                                                } else {
                                                    (self.picker_label()).to_string()
                                                },
                                            ),
                                    )
                                    .when_some(self.picker_effort_label(), |view, detail| {
                                        view.child(
                                            div()
                                                .flex_none()
                                                .text_size(px(11.))
                                                .text_color(rgb(MUTED))
                                                .child(detail),
                                        )
                                    })
                                    .child(icon("alt-arrow-down").size(px(12.)).opacity(0.6))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_model_picker(window, cx)
                                    })),
                            )
                            .when_some(self.context_control(cx), |view, control| {
                                view.child(control)
                            })
                            .when(working && self.queue_edit.is_none(), |d| {
                                d.child(
                                    icon_button(
                                        "stop",
                                        "stop",
                                        crate::localization::text(
                                            "projects-masters_stop_agent",
                                            cx,
                                        ),
                                    )
                                    .size(px(28.))
                                    .bg(rgb(0x33352d))
                                    .text_color(rgb(TEXT))
                                    .on_click(cx.listener(|this, _, _, cx| this.interrupt(cx))),
                                )
                            })
                            .when((!working || has_draft) && self.queue_edit.is_none(), |d| {
                                d.child(
                                    crate::styling::round_icon_action(
                                        "send",
                                        if working { "queue-send" } else { "arrow-up" },
                                        if working {
                                            "Queue message"
                                        } else {
                                            "Send message"
                                        },
                                        has_draft
                                            && (self.connected || self.cloud.cached)
                                            && !self.transcript_blocked()
                                            && !self.cloud_model_change_pending(),
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

    pub(super) fn transcript(&self, cx: &mut Context<Self>) -> AnyElement {
        let archived = self
            .selected
            .as_ref()
            .is_some_and(|chat| chat["archived"] == true);
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
            .when(self.messages.is_empty() && archived, |d| {
                d.child(column().flex_1().justify_center().items_center().gap_3()
                    .child(icon("archive-minimalistic").size(px(24.)).text_color(rgb(MUTED)))
                    .child(label(if self.transcript_blocked() { crate::localization::text("native-loading_saved_conversation", cx) } else { crate::localization::text("native-no_messages_archived", cx) }).text_color(rgb(MUTED))))
            })
            .when(self.messages.is_empty() && !archived && self.selected.is_none() && !self.cloud_startup_pending(), |d| {
                let title = self.creation_project_name().map(|name|tr(cx, |l10n| msg::native::what_build_in(l10n, name))).unwrap_or_else(||tr(cx,msg::native::what_build_today));
                d.px(px(if self.viewport_width <= 800. { 16. } else { 24. })).pt(px(32.)).child(column().flex_1().justify_center().items_center().w_full().max_w(px(768.)).self_center().gap_2()
                    .child(div().relative().top(px(3.)).text_size(px(24.)).line_height(px(32.)).font_weight(FontWeight::MEDIUM).child(title))
                    .when(self.project.is_some() || self.cloud.mode, |view| view.child(control("welcome-without-project", tr(cx, msg::chat::start_without_project))
                        .text_color(rgb(MUTED)).text_size(px(12.))
                        .on_click(cx.listener(|this, _, _, cx| { this.project = None; this.set_creation_destination(false, cx); cx.notify(); })))))
            })
            .when(self.transcript_state.has_more, |view| view.child(
                row().w_full().justify_center().child(control("load-earlier-messages", if self.transcript_state.loading_earlier { "Loading earlier messages…" } else { "Load earlier messages" })
                    .on_click(cx.listener(|this, _, _, cx| this.load_earlier_messages(cx))))
            ))
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
                    .children(entry.parts.iter().filter_map(|part| {
                        if let zuse_model::MessagePart::AgentContext { agent, previous_agent: Some(previous), .. } = part {
                            Some(crate::activity::agent_switch_notice(previous, agent, rgb(MUTED).into()))
                        } else { None }
                    }))
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
                                    "input" => {
                                        let in_composer = part["resolved"] != true && self.pending_composer_question().is_some_and(|(id,_)| part["requestId"] == id);
                                        if in_composer { None } else { Some(self.input_card(part, cx)) }
                                    },
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
                                    "visual" => Some(self.visual_part(part,cx)),
                                    "uiBlock" => Some(self.ui_block_part(part, i + 1 == self.messages.len(), cx)),
                                    "tool" | "reasoning" => Some(self.activity_part(&entry.id,part,cx)),
                                    "image" => Some(self.image_preview(
                                        part["path"].as_str().unwrap_or_default(),
                                        part["name"].as_str().unwrap_or("Image attachment"),384.,cx)),
                                    _ => None,
                                }
                            }),
                    )
                    .child(row().absolute().left_0().right_0().bottom(px(-25.)).h(px(22.)).when(user,|d|d.justify_end()).child(
                        icon_button(("copy-message",i),"copy",crate::localization::text("chat-assistant_message_actions_copy_message", cx))
                            .opacity(0.).group_hover("message",|d|d.opacity(1.))
                            .on_click({let text=entry.parts.iter().filter_map(|p|serde_json::to_value(p).ok()).filter_map(|p|p["text"].as_str().map(str::to_owned)).collect::<Vec<_>>().join("\n");move|_,_,cx|cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))})
                    ))
            }))
            .into_any_element()
    }

    pub(super) fn project_picker_view(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .p_6()
            .gap_4()
            .flex_1()
            .child(
                row()
                    .child(
                        div()
                            .text_size(px(20.))
                            .child(tr(cx, msg::native::open_a_project)),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button("close-project-picker", "close", "Close project picker")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.project_picker = false;
                                cx.notify();
                            })),
                    ),
            )
            .child(label(tr(
                cx,
                msg::native::choose_a_known_repository_or_enter_a_path_on_the_engine_host,
            )))
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
                        if !this.new_chat(cx) {
                            return;
                        }
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
                control("add-project", tr(cx, msg::native::open_folder))
                    .bg(rgb(RAISED))
                    .on_click(cx.listener(|this, _, _, cx| this.add_project(cx))),
            )
            .into_any_element()
    }

    fn workspace_panel(&self, width: f32, cx: &mut Context<Self>) -> AnyElement {
        column()
            .w(px(width))
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
                    .child(
                        row()
                            .id("workspace-panel-tabs")
                            .flex_1()
                            .min_w_0()
                            .overflow_x_scroll()
                            .children(
                                [
                                    ("browser", "Browser"),
                                    ("files", "Files"),
                                    ("changes", "Changes"),
                                    ("history", "History"),
                                    ("pullRequests", "PRs"),
                                    ("terminal", "Terminal"),
                                    ("actions", "Actions"),
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
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.set_panel(tab, cx)
                                        }))
                                }),
                            ),
                    )
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
                "browser" => self.browser_view(cx),
                "recovery" => self.recovery_view(cx),
                "actions" => self.project_actions_view(cx),
                "terminal" => column()
                    .child(self.terminal_tabs_view(cx))
                    .when(self.terminal_id.is_none(), |d| {
                        d.child(
                            control("open-browser-terminal", tr(cx, msg::native::open_terminal))
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
                "history" => self.history_view(cx),
                "pullRequests" => self.pull_requests_view(cx),
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
                            d.child(label(tr(cx, msg::common::loading)).text_size(px(10.)))
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
                        label(tr(
                            cx,
                            msg::native::partial_file_preview_editing_is_disabled,
                        ))
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
                            crate::localization::text("settings-wallpaper_saving", cx)
                        } else if self.file_dirty(cx) {
                            ("Unsaved changes").to_string()
                        } else {
                            crate::localization::text("chat-browser_pane_saved", cx)
                        }))
                        .child(div().flex_1())
                        .when(self.file_dirty(cx), |d| {
                            d.child(
                                control(
                                    "discard-edits",
                                    tr(cx, msg::shell::pending_chat_creation_discard),
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.discard_file_edits(cx)),
                                ),
                            )
                        })
                        .when(
                            !file["readOnlyReason"].is_string() && file["truncated"] != true,
                            |d| {
                                d.child(
                                    control(
                                        "save-file",
                                        if self.save_request.is_some() {
                                            crate::localization::text(
                                                "settings-wallpaper_saving",
                                                cx,
                                            )
                                        } else {
                                            crate::localization::text("common-save", cx)
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
                            icon_button(
                                "directory-up",
                                "arrow-left",
                                crate::localization::text(
                                    "chat-environment_path_browser_parent_folder",
                                    cx,
                                ),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                let parent = this
                                    .directory
                                    .rsplit_once('/')
                                    .map(|(p, _)| p)
                                    .unwrap_or("")
                                    .to_owned();
                                this.load_directory(&parent);
                                cx.notify();
                            })),
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
                        d.child(label(tr(cx, msg::native::loading_files)).px_3().py_2())
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
                                                .child(crate::assets::file_icon(
                                                    entry["path"].as_str().unwrap_or_default(),
                                                    entry["kind"] == "directory",
                                                ))
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
                                    crate::localization::text("common-loading", cx)
                                } else {
                                    ("Load more files").to_string()
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
        let light = match self.preferences.appearance {
            zuse_protocol::presentation::AppearanceMode::Light => true,
            zuse_protocol::presentation::AppearanceMode::Dark => false,
            zuse_protocol::presentation::AppearanceMode::System => matches!(
                window.appearance(),
                WindowAppearance::Light | WindowAppearance::VibrantLight
            ),
        };
        crate::palette::set_light(light);
        if self.cached_only {
            return column()
                .size_full()
                .min_h_0()
                .child(self.transcript(cx))
                .into_any_element();
        }
        if self.cloud.target.is_some() && !self.cloud_startup_pending() {
            return self.cloud_workspace(window, cx);
        }
        if self.connected
            && !self.transcript_blocked()
            && self.cloud.resume_draft.as_ref().is_some_and(|draft| {
                self.selected.as_ref().and_then(|chat| chat["id"].as_str())
                    == Some(draft.0.as_str())
            })
        {
            let (_, text, attachments, send) = self.cloud.resume_draft.take().unwrap();
            self.input.update(cx, |input, cx| input.set_value(text, cx));
            self.attachments = attachments;
            if send {
                self.submit(&Submit, window, cx);
            }
        }
        self.sync_motion(cx);
        self.sync_browser_visibility();
        self.sync_browser_voice_visibility(cx);
        let target = self
            .selected
            .as_ref()
            .filter(|_| !self.cloud.cached)
            .and_then(|chat| {
                Some(crate::git_controls::Target {
                    chat_id: chat["id"].as_str()?.into(),
                    cwd: chat["cwd"].as_str()?.into(),
                    device_id: chat["deviceId"].as_str()?.into(),
                    archived: chat["archived"] == true,
                })
            });

        let colors = crate::plugin_setup::SetupColors {
            text: rgb(0xe8e9e7).into(),
            muted: rgb(0xa4a8a3).into(),
            background: rgb(0x141414).into(),
            border: rgb(0x343434).into(),
            danger: rgb(0xe5a0a0).into(),
        };
        self.git_controls.update(cx, |panel, cx| {
            panel.colors = colors;
            panel.set_target(target, cx);
        });

        let panel_width = if !self.right_open || self.settings || self.project_picker {
            0.
        } else if self.right_tab == "browser" {
            (f32::from(window.viewport_size().width)
                - if self.sidebar_open {
                    self.sidebar_width()
                } else {
                    0.
                }
                - 380.)
                .clamp(300., 640.)
        } else if self.open_file.is_some()
            || matches!(self.right_tab.as_str(), "history" | "pullRequests")
        {
            400.
        } else if self.right_tab == "terminal" {
            360.
        } else {
            320.
        };
        self.viewport_width = f32::from(window.viewport_size().width);
        self.content_height = f32::from(window.viewport_size().height)
            - if self.settings {
                0.
            } else {
                2. * crate::workspace_style::GUTTER
            };
        self.content_width = f32::from(window.viewport_size().width)
            - if self.sidebar_open && !self.settings {
                self.sidebar_width()
            } else {
                0.
            }
            - panel_width
            - if self.settings {
                0.
            } else {
                2. * crate::workspace_style::GUTTER
            };
        div()
            .track_focus(&self.workspace_focus)
            .key_context("Workspace")
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.sidebar_resize.is_some() {
                    this.resize_sidebar(f32::from(event.position.x), cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_sidebar_resize(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_sidebar_resize(cx)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.model_picker
                        || this.agent_picker
                        || this.access_picker
                        || this.add_menu
                        || this.context_menu
                    {
                        this.model_picker = false;
                        this.agent_picker = false;
                        this.access_picker = false;
                        this.add_menu = false;
                        this.context_menu = false;
                        cx.notify();
                    }
                }),
            )
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(|this, _: &SearchSessions, window, cx| {
                if this.thread_dialog.is_none() {
                    this.toggle_search(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewSession, window, cx| {
                if this.thread_dialog.is_none() {
                    this.start_new_session(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ClosePreview, window, cx| {
                if this.usage_open && !this.settings {
                    if let Some(dashboard) = this.usage_dashboard.clone() {
                        dashboard.update(cx, |dashboard, cx| dashboard.escape(window, cx));
                    }
                    return;
                }
                if this.cancel_sidebar_resize(cx) {
                    return;
                }
                if this
                    .sidebar_controls
                    .update(cx, |controls, cx| controls.dismiss_on_escape(window, cx))
                {
                    return;
                }
                if this.cloud.picker.take().is_some() {
                    cx.notify();
                    return;
                }
                if this.dismiss_cloud_delete(window, cx) {
                    return;
                }
                if this.thread_dialog.is_some() {
                    this.close_thread_dialog(window, cx);
                    return;
                }
                if this.context_menu {
                    this.context_menu = false;
                    window.focus(&this.context_focus, cx);
                    cx.notify();
                    return;
                }
                if this.plugin_state.confirmation.is_some() {
                    if let Some(dialog) = this.plugin_detail_menu.removal.clone() {
                        dialog.update(cx, |dialog, cx| dialog.cancel(window, cx));
                    }
                    cx.notify();
                    return;
                }
                if this.project_archive.menu.take().is_some() {
                    cx.notify();
                    return;
                }
                if this.plugins_add_menu {
                    this.plugins_add_menu = false;
                    cx.notify();
                    return;
                }
                if let Some(form) = this.plugin_setup.clone() {
                    form.update(cx, |form, cx| form.cancel(window, cx));
                    cx.notify();
                    return;
                }
                if this.plugins_browse {
                    if this.plugin_detail_menu.account.is_some() {
                        this.plugin_detail_menu.close_account(true, window, cx);
                    } else if this.plugin_detail_menu.open {
                        this.plugin_detail_menu.close();
                    } else if this.plugin_detail.take().is_none() {
                        this.close_plugins_browse();
                        this.settings = true;
                    }
                    cx.notify();
                    return;
                }
                if this.dismiss_custom_acp(cx) {
                    return;
                }
                if this.settings
                    && this.settings_section == crate::settings_view::Section::Providers
                    && this.dismiss_provider_login(cx)
                {
                    return;
                }
                if this.project_archive.global {
                    this.project_archive_escape(window, cx);
                    return;
                }
                if this.remote_ui.dialog.is_some() {
                    this.close_remote_dialog(window, cx);
                    return;
                }
                if this.settings {
                    if this.model_picker && this.picker_in_settings() {
                        let target = this.picker_target;
                        this.model_picker = false;
                        this.picker_target = model_picker::PickerTarget::Composer;
                        this.return_model_picker_focus(target, window, cx);
                    } else if this.default_permission_menu {
                        this.default_permission_menu = false;
                        window.focus(&this.default_permission_focus, cx);
                    } else if this.completion_preset_menu {
                        this.completion_preset_menu = false;
                        window.focus(&this.completion_preset_focus, cx);
                    } else {
                        this.settings = false;
                        this.model_picker = false;
                        this.picker_target = model_picker::PickerTarget::Composer;
                        window.focus(&this.input.focus_handle(cx), cx);
                    }
                    cx.notify();
                    return;
                }
                if this.project_archive.project.is_some() {
                    this.project_archive_escape(window, cx);
                    return;
                }
                if this.queue_edit.is_some()
                    && this.input.focus_handle(cx).is_focused(window)
                    && !this.model_picker
                    && !this.agent_picker
                    && !this.access_picker
                    && !this.add_menu
                    && !this.search_open
                    && this.preview_open.is_none()
                {
                    this.cancel_queue_edit(cx);
                    return;
                }
                this.preview_open = None;
                this.model_picker = false;
                this.agent_picker = false;
                this.search_open = false;
                this.access_picker = false;
                this.add_menu = false;
                window.focus(&this.input.focus_handle(cx), cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &SaveFile, _, cx| {
                if this.usage_open {
                    return;
                }
                if this.thread_dialog.is_none() && this.project_archive.project.is_none() {
                    this.save_file(cx);
                }
            }))
            .size_full()
            .relative()
            .flex()
            .bg(rgb(if self.settings { BG } else { SURFACE }))
            .text_color(rgb(TEXT))
            .font(gpui::Font {
                family: "Geist".into(),
                fallbacks: Some(crate::language_fonts::fallbacks(
                    crate::localization::localizer(cx).locale(),
                )),
                ..Default::default()
            })
            .text_size(px(13.))
            .when(self.sidebar_open && !self.settings, |d| {
                d.child(
                    crate::workspace_style::column()
                        .w(px(self.sidebar_width()))
                        .flex_none()
                        .pl(px(crate::workspace_style::GUTTER))
                        .child(self.sidebar(cx)),
                )
                .child(self.sidebar_separator(cx))
            })
            .child(self.workspace_main(window, cx))
            .when(
                self.right_open
                    && !self.settings
                    && !self.project_picker
                    && self.project_archive.project.is_none()
                    && !self.usage_open,
                |d| d.child(self.workspace_panel(panel_width, cx)),
            )
            .when(self.preview_open.is_some() && !self.settings, |d| {
                d.child(self.preview_overlay(window, cx))
            })
            .when_some(self.project_archive_menu_view(cx), |d, menu| d.child(menu))
            .when(self.thread_dialog.is_some(), |d| {
                d.child(self.thread_dialog_view(cx))
            })
            .into_any_element()
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

pub(super) fn markdown_block(block: &Block) -> AnyElement {
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
        "archived" => "Archived thread · Unarchive to edit files",
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
