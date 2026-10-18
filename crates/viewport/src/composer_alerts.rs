//! Compact attached composer notices and explicit recovery actions.
use super::*;
use crate::{
    assets::icon,
    shell::{BORDER, FAINT, MUTED, TEXT, column, control, icon_button, label, row},
};

impl WorkspaceView {
    fn latest_provider_alert(&self) -> Option<(String, String)> {
        if self.is_working() {
            return None;
        }
        let entry = self
            .messages
            .iter()
            .rev()
            .find(|m| m.role == zuse_model::MessageRole::Assistant)?;
        entry.parts.iter().rev().find_map(|part| match part {
            zuse_model::MessagePart::Error { id, message } => {
                let key = format!("{}:{id}", entry.id);
                (!self.dismissed_alerts.contains(&key)).then(|| (key, message.clone()))
            }
            _ => None,
        })
    }
    fn restore_provider_message(
        &mut self,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.queue_edit.is_some()
            || !self.input.read(cx).value(cx).is_empty()
            || !self.attachments.is_empty()
        {
            self.error =
                Some("Keep or clear the current draft before restoring this message.".into());
            cx.notify();
            return;
        }
        let text = self
            .messages
            .iter()
            .rev()
            .find(|m| m.role == zuse_model::MessageRole::User)
            .map(|entry| {
                entry
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        zuse_model::MessagePart::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        if let Some(text) = text {
            let parsed = zuse_protocol::message_attachments::parse(&text);
            self.attachments = parsed
                .attachments
                .into_iter()
                .map(|a| json!({"path":a.path,"name":a.name}))
                .collect();
            self.input
                .update(cx, |input, cx| input.set_value(parsed.text, cx));
            self.dismissed_alerts.insert(key);
            self.sync_previews(true, cx);
            window.focus(&self.input.focus_handle(cx), cx);
            cx.notify();
        }
    }
    pub(super) fn has_composer_notices(&self) -> bool {
        !self.connected
            || self.transcript_state.notice.is_some()
            || self.error.is_some()
            || self.latest_provider_alert().is_some()
            || self.connection_notice.is_some()
            || !self.uncertain_drafts.is_empty()
            || !self.queue.is_empty()
            || self.is_working()
            || self.provider_update_notice()
    }
    pub(super) fn composer_notices(&self, cx: &mut Context<Self>) -> AnyElement {
        let alert = self
            .error
            .as_ref()
            .map(|error| (format!("local:{error}"), error.clone(), true))
            .or_else(|| {
                self.latest_provider_alert()
                    .map(|(id, text)| (id, text, false))
            });
        let mut view = column().w_full();
        if let Some(notice) = &self.transcript_state.notice {
            view = view.child(
                row()
                    .id("transcript-refresh-notice")
                    .role(Role::Status)
                    .min_h(px(32.))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(icon("refresh").size(px(14.)).text_color(rgb(MUTED)))
                    .child(label(notice.clone()).flex_1().min_w_0().text_size(px(11.)))
                    .child(
                        control("retry-transcript", "Retry")
                            .tab_index(0)
                            .h(px(24.))
                            .on_click(cx.listener(|this, _, _, cx| this.retry_transcript(cx))),
                    ),
            );
        }

        if self.provider_update_notice() {
            view = view.child(
                row()
                    .id("provider-updates-notice")
                    .role(Role::Status)
                    .min_h(px(32.))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(icon("refresh").size(px(14.)).text_color(rgb(MUTED)))
                    .child(
                        label("Agent provider updates")
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.)),
                    )
                    .child(
                        control("review-provider-updates", "Review")
                            .h(px(24.))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.settings = true;
                                this.select_settings_section(
                                    crate::settings_view::Section::Providers,
                                    cx,
                                );
                                window.focus(&this.settings_nav_focus, cx);
                            })),
                    ),
            );
        }
        if !self.connected {
            view = view.child(
                row()
                    .id("connection-alert")
                    .role(Role::Status)
                    .min_h(px(32.))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(icon("globe").size(px(14.)).text_color(rgb(0xe2b876)))
                    .child(
                        label(self.status.clone())
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11.)),
                    ),
            );
        }
        if let Some((id, message, local)) = alert {
            let lower = message.to_lowercase();
            let title = if lower.contains("auth")
                || lower.contains("sign in")
                || lower.contains("api key")
                || lower.contains("log in")
            {
                "Sign in to continue"
            } else if lower.contains("quota")
                || lower.contains("rate limit")
                || lower.contains("usage limit")
            {
                "Usage limit reached"
            } else if lower.contains("not installed") {
                "Agent unavailable"
            } else if lower.contains("file") || lower.contains("save") {
                "File needs attention"
            } else {
                "Something went wrong"
            };
            let expanded = self.expanded_alert.as_deref() == Some(&id);
            let restore = id.clone();
            let account_action = lower.contains("auth")
                || lower.contains("sign in")
                || lower.contains("api key")
                || lower.contains("log in")
                || lower.contains("not installed")
                || lower.contains("quota")
                || lower.contains("usage limit");
            let toggle = id.clone();
            let dismiss = id.clone();
            let copy = message.clone();
            view = view.child(
                column()
                    .id("composer-error")
                    .role(Role::Alert)
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(
                        row()
                            .min_h(px(32.))
                            .px_3()
                            .py_1()
                            .gap_2()
                            .child(
                                icon("danger-triangle")
                                    .size(px(14.))
                                    .text_color(rgb(0xe2b876)),
                            )
                            .child(
                                row()
                                    .id("expand-composer-error")
                                    .cursor_pointer()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(title),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(11.))
                                            .text_color(rgb(MUTED))
                                            .text_ellipsis()
                                            .child(message.clone()),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.expanded_alert =
                                            if this.expanded_alert.as_deref() == Some(&toggle) {
                                                None
                                            } else {
                                                Some(toggle.clone())
                                            };
                                        cx.notify();
                                    })),
                            )
                            .child(
                                icon_button("copy-composer-error", "copy", "Copy error details")
                                    .size(px(24.))
                                    .on_click(move |_, _, cx| {
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            copy.clone(),
                                        ))
                                    }),
                            )
                            .when(!local, |d| {
                                d.child(
                                    crate::styling::dither_action(
                                        "restore-provider-message",
                                        "Restore message",
                                        true,
                                        true,
                                    )
                                    .h(px(26.))
                                    .text_size(px(11.))
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.restore_provider_message(
                                                restore.clone(),
                                                window,
                                                cx,
                                            )
                                        },
                                    )),
                                )
                            })
                            .when(account_action, |d| {
                                d.child(
                                    control("composer-error-settings", "Settings")
                                        .h(px(26.))
                                        .text_size(px(11.))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.settings = true;
                                            cx.notify();
                                        })),
                                )
                            })
                            .child(
                                icon_button("dismiss-error", "close", "Dismiss error")
                                    .size(px(24.))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if local {
                                            this.error = None;
                                        } else {
                                            this.dismissed_alerts.insert(dismiss.clone());
                                        }
                                        this.expanded_alert = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(expanded, |d| {
                        d.child(
                            div()
                                .id("composer-error-details")
                                .max_h(px(160.))
                                .overflow_y_scroll()
                                .px_3()
                                .py_2()
                                .border_t_1()
                                .border_color(rgb(BORDER))
                                .text_size(px(11.))
                                .line_height(px(18.))
                                .text_color(rgb(MUTED))
                                .child(message),
                        )
                    }),
            );
        }
        if self.connection_notice.is_some() || !self.uncertain_drafts.is_empty() {
            view = view.child(self.connection_recovery_view(cx));
        }
        if !self.queue.is_empty() {
            view = view.child(self.queue_view(cx));
        }
        if self.is_working() {
            view = view.child(
                row()
                    .id("agent-working-tray")
                    .role(Role::Status)
                    .min_h(px(32.))
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(rgb(BORDER))
                    .child(div().size(px(5.)).rounded_full().bg(rgb(0x89ca21)))
                    .child(label("Working").text_color(rgb(TEXT)).text_size(px(11.)))
                    .child(label("New messages join the queue").text_size(px(10.)))
                    .child(div().flex_1()),
            );
        }
        view.into_any_element()
    }
    pub(super) fn access_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .id("access-menu")
            .role(Role::ListBox)
            .aria_label("Agent access")
            .occlude()
            .absolute()
            .bottom_full()
            .mb_2()
            .left(px(40.))
            .w(px((self.content_width - 80.).clamp(200., 256.)))
            .p_1()
            .gap(px(0.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(0x343434))
            .bg(rgb(0x20211e))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                label("AGENT ACCESS")
                    .text_size(px(10.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(MUTED))
                    .px_2()
                    .pt(px(6.))
                    .pb(px(4.)),
            )
            .children(
                zuse_protocol::presentation::ACCESS_LEVELS
                    .into_iter()
                    .enumerate()
                    .map(|(i, mode)| {
                        let details = zuse_protocol::presentation::access_details(mode);
                        row()
                            .id(("access-mode", i))
                            .role(Role::ListBoxOption)
                            .aria_label(details.label)
                            .aria_selected(self.sandbox == mode)
                            .cursor_pointer()
                            .px_2()
                            .py_2()
                            .gap_2()
                            .rounded(px(6.))
                            .when(self.access_active == i, |view| view.bg(rgb(0x2b2b28)))
                            .hover(|d| d.bg(rgb(0x2b2b28)))
                            .child(
                                column()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(px(4.))
                                    .child(
                                        row()
                                            .gap_2()
                                            .child(
                                                svg()
                                                    .path(details.icon)
                                                    .size(px(14.))
                                                    .flex_none()
                                                    .text_color(rgb(TEXT)),
                                            )
                                            .child(
                                                div()
                                                    .text_color(rgb(TEXT))
                                                    .text_size(px(12.))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(details.label),
                                            ),
                                    )
                                    .child(
                                        label(details.description)
                                            .text_size(px(11.))
                                            .line_height(px(16.)),
                                    ),
                            )
                            .when(self.sandbox == mode, |view| {
                                view.child(icon("check").size(px(14.)).text_color(rgb(TEXT)))
                            })
                            .on_mouse_move(cx.listener(move |this, _, _, cx| {
                                if this.access_active != i {
                                    this.access_active = i;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.choose_access(mode, cx);
                                window.focus(&this.input.focus_handle(cx), cx);
                                cx.notify();
                            }))
                    }),
            )
            .child(
                label(if self.access_save.is_some() {
                    "Saving permission mode…"
                } else if self.selected.is_none() {
                    "Used when you start this chat."
                } else {
                    "Saved for this chat and its next turns."
                })
                .px_2()
                .py_2()
                .text_size(px(10.))
                .text_color(rgb(FAINT)),
            )
            .into_any_element()
    }
    pub(super) fn composer_add_menu(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .id("composer-add-menu")
            .absolute()
            .bottom_full()
            .mb_2()
            .left_0()
            .w(px(220.))
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x343434))
            .bg(rgb(0x20211e))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                control("attach-files-menu", "Attach images or files")
                    .h(px(34.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_menu = false;
                        this.pick_attachments();
                        cx.notify();
                    })),
            )
            .child(
                control("composer-tools-settings", "Plugins & connections")
                    .h(px(34.))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_menu = false;
                        this.settings = true;
                        this.plugin_action(json!({"action":"list"}));
                        cx.notify();
                    })),
            )
            .into_any_element()
    }
}
