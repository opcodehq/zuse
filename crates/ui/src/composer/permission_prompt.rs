//! The same compact permission surface as the reference permission-card.
use super::*;
use crate::popover;
use zuse_protocol::permission::{ALLOW_CHAT, ALLOW_PROJECT, PermissionAction};

impl Composer {
    pub(super) fn render_permission_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let question = self
            .wizard
            .as_ref()
            .and_then(|wizard| wizard.current())
            .unwrap()
            .clone();
        let prompt = question.approval_prompt.as_ref().unwrap();
        let theme = Theme::of(cx).clone();
        let (headline, detail) = if matches!(prompt.action, PermissionAction::Other { .. }) {
            (question.header.clone(), question.question.clone())
        } else {
            (prompt.action.headline(), prompt.action.detail().to_owned())
        };
        if self.input.focus_handle(cx).is_focused(window) {
            window.focus(&self.permission_focus, cx);
        }
        let allow = prompt.allow_label.clone();
        let deny = prompt.deny_label.clone();
        let can_chat =
            prompt.persistent_allowed && question.options.iter().any(|label| label == ALLOW_CHAT);
        let can_project = prompt.persistent_allowed
            && question.options.iter().any(|label| label == ALLOW_PROJECT);
        let queue = self.wizard.as_ref().map_or(1, |wizard| {
            wizard.questions.len().saturating_sub(wizard.page)
        });
        let mut card = div()
            .id("tool-approval-card")
            .role(Role::Group)
            .aria_label(headline.clone())
            .when(self.permission_inspector.is_none(), |view| {
                view.track_focus(&self.permission_focus)
            })
            .w_full()
            .rounded(px(12.))
            .p(px(12.))
            .border_1()
            .border_color(theme.border.opacity(0.7))
            .bg(theme.surface_card)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let prompt = this
                    .wizard
                    .as_ref()
                    .and_then(|wizard| wizard.current())
                    .and_then(|question| question.approval_prompt.as_ref());
                let answer = match prompt {
                    Some(prompt) if event.keystroke.key == "escape" => {
                        Some(prompt.deny_label.clone())
                    }
                    Some(prompt)
                        if event.keystroke.key == "enter"
                            && (event.keystroke.modifiers.platform
                                || event.keystroke.modifiers.control) =>
                    {
                        Some(prompt.allow_label.clone())
                    }
                    _ => None,
                };
                if let Some(answer) = answer {
                    this.answer_permission(&answer, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text_strong)
                            .child(headline),
                    )
                    .when(queue > 1, |row| {
                        row.child(
                            div()
                                .rounded_full()
                                .px(px(8.))
                                .py(px(2.))
                                .text_size(px(10.))
                                .bg(theme.muted_fill())
                                .text_color(theme.text_muted)
                                .child(format!("+{} more", queue - 1)),
                        )
                    }),
            )
            .child(
                div()
                    .id("permission-request-detail")
                    .mt(px(8.))
                    .max_h(px(96.))
                    .overflow_y_scroll()
                    .rounded(px(6.))
                    .px(px(10.))
                    .py(px(6.))
                    .bg(theme.muted_fill().opacity(0.45))
                    .font_family(theme.font_mono.clone())
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(theme.text)
                    .child(detail),
            );
        if !can_chat && !can_project {
            card = card.child(
                div()
                    .mt(px(6.))
                    .text_size(px(10.))
                    .line_height(px(16.))
                    .text_color(theme.text_muted)
                    .child("Only Allow once is available for this request."),
            );
        }
        card.child(
            div()
                .mt(px(10.))
                .flex()
                .flex_wrap()
                .items_center()
                .justify_end()
                .gap(px(4.))
                .child(
                    popover::btn_ghost(&theme, "Deny", "permission-deny")
                        .id("permission-deny")
                        .role(Role::Button)
                        .aria_label("Deny this action")
                        .tab_index(0)
                        .h(px(28.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .text_size(px(11.))
                        .tooltip(crate::settings::widgets::text_tooltip("Escape".to_owned()))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.answer_permission(&deny, cx)),
                        ),
                )
                .child(
                    popover::btn_ghost(&theme, "Allow for this chat", "permission-allow-chat")
                        .id("permission-allow-chat")
                        .role(Role::Button)
                        .tab_index(0)
                        .h(px(28.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .text_size(px(11.))
                        .when(!can_chat, |view| view.opacity(0.4))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if can_chat {
                                this.answer_permission(ALLOW_CHAT, cx);
                            }
                        })),
                )
                .child(
                    popover::btn_ghost(&theme, "Always allow", "permission-allow-project")
                        .id("permission-allow-project")
                        .role(Role::Button)
                        .aria_label("Always allow in this project")
                        .tab_index(0)
                        .h(px(28.))
                        .px(px(8.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .text_size(px(11.))
                        .when(!can_project, |view| view.opacity(0.4))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if can_project {
                                this.answer_permission(ALLOW_PROJECT, cx);
                            }
                        })),
                )
                .child(
                    popover::btn_primary(&theme, "Allow once")
                        .id("permission-allow-once")
                        .role(Role::Button)
                        .tab_index(0)
                        .ml(px(4.))
                        .h(px(28.))
                        .px(px(10.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .text_size(px(11.))
                        .tooltip(crate::settings::widgets::text_tooltip(
                            "⌘ / Ctrl + Enter".to_owned(),
                        ))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.answer_permission(&allow, cx)),
                        ),
                ),
        )
        .into_any_element()
    }
    pub(super) fn answer_permission(&mut self, label: &str, cx: &mut Context<Self>) {
        let index = self
            .wizard
            .as_ref()
            .and_then(|wizard| wizard.current())
            .and_then(|question| question.options.iter().position(|option| option == label));
        let Some(index) = index else {
            return;
        };
        self.advance_task = None;
        self.input.update(cx, |input, cx| input.set_text("", cx));
        if let Some(wizard) = self.wizard.as_mut() {
            let _ = wizard.select(index);
        }
        self.wizard_advance(cx);
    }
}
