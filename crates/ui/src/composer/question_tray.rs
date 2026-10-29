//! Reference layout: question tray at the top of the composer, answer editor below.
use super::*;
impl Composer {
    pub(super) fn render_question_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(wizard) = self.wizard.clone() else {
            return gpui::Empty.into_any_element();
        };
        let Some(question) = wizard.current().cloned() else {
            return gpui::Empty.into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let secret = self.private_question();
        if secret {
            self.prepare_private_input(window, cx);
        }
        let editor = if secret {
            self.private_input
                .as_ref()
                .unwrap()
                .1
                .clone()
                .into_any_element()
        } else {
            let cap = TEXTAREA_MAX - TEXTAREA_PAD_V;
            self.input.update(cx, |input, _| {
                input.viewport_height = Some(cap);
                input.settled_viewport_height = Some(cap);
                input.resizing = false;
                input.overflow_top_padding = 0.;
            });
            self.input.clone().into_any_element()
        };
        let empty = if secret {
            self.private_input
                .as_ref()
                .unwrap()
                .1
                .read(cx)
                .value(cx)
                .is_empty()
        } else {
            self.input.read(cx).is_empty()
        };
        let can_submit = if secret {
            !empty
        } else {
            !empty || wizard.page_has_pick() || question.multiline
        };
        let mut header = div()
            .flex()
            .items_start()
            .gap(px(8.))
            .px(px(14.))
            .pt(px(12.))
            .pb(px(8.))
            .child(
                div()
                    .id("question-tray-description")
                    .flex_1()
                    .min_w_0()
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .text_size(px(13.))
                    .line_height(px(20.))
                    .text_color(theme.text_strong)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(question.question.clone()),
            );
        if wizard.questions.len() > 1 {
            header = header.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        crate::popover::btn_ghost(&theme, "‹", "question-previous")
                            .id("question-previous")
                            .when(wizard.page == 0, |d| d.opacity(0.3))
                            .on_click(cx.listener(|this, _, _, cx| this.wizard_back(cx))),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme.text_muted)
                            .child(wizard.counter()),
                    ),
            );
        }
        let mut tray = div()
            .flex()
            .flex_col()
            .w(gpui::relative(14. / 15.))
            .mx_auto()
            .rounded_t(px(COMPOSER_RADIUS))
            .border_1()
            .border_b_0()
            .border_color(theme.composer_glass_border())
            .child(header);
        if question.multi_select {
            tray = tray.child(
                div()
                    .px(px(14.))
                    .pb(px(4.))
                    .text_size(px(11.))
                    .text_color(theme.text_muted)
                    .child("Select one or more options."),
            );
        }
        for (index, option) in question.options.iter().enumerate() {
            let picked = wizard.is_picked(index);
            tray =
                tray.child(
                    div()
                        .id(("question-option", index))
                        .mx(px(6.))
                        .px(px(8.))
                        .py(px(7.))
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .cursor_pointer()
                        .bg(if picked {
                            crate::theme::ink(0.08)
                        } else {
                            gpui::transparent_black()
                        })
                        .hover(|d| d.bg(crate::theme::ink(0.05)))
                        .child(div().text_size(px(10.)).text_color(theme.text_muted).child(
                            if picked {
                                "✓".into()
                            } else {
                                (index + 1).to_string()
                            },
                        ))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(13.))
                                .text_color(theme.text)
                                .child(option.clone()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.wizard_select(index, cx))),
                );
        }
        let title = if secret {
            "Approve and use"
        } else if wizard.page + 1 == wizard.questions.len() {
            "Submit answer"
        } else {
            "Continue"
        };
        let composer = div()
            .flex()
            .flex_col()
            .rounded(px(COMPOSER_RADIUS))
            .border_1()
            .border_color(theme.composer_glass_border())
            .px(px(14.))
            .pt(px(14.))
            .pb(px(10.))
            .gap(px(12.))
            .child(div().min_h(px(48.)).child(editor))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(if secret {
                                "Private · used once · kept out of chat"
                            } else {
                                "Type an answer or choose an option above"
                            }),
                    )
                    .when(secret, |footer| {
                        footer.child(
                            crate::popover::btn_ghost(&theme, "Decline", "question-decline")
                                .id("question-decline")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.submit_private_secret(true, cx)
                                })),
                        )
                    })
                    .child(
                        crate::popover::btn_primary(&theme, title)
                            .id("question-send")
                            .when(!can_submit, |d| d.opacity(0.4))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !can_submit {
                                    return;
                                }
                                if secret {
                                    this.submit_private_secret(false, cx);
                                } else {
                                    this.wizard_advance(cx);
                                }
                            })),
                    ),
            );
        let panel = div()
            .track_focus(&self.wizard_focus)
            .on_key_down(
                cx.listener(|this, event, window, cx| this.on_wizard_key(event, window, cx)),
            )
            .flex()
            .flex_col()
            .w_full()
            .child(crate::frost::floating_glass(COMPOSER_RADIUS, 20., tray))
            .child(crate::frost::floating_glass(COMPOSER_RADIUS, 20., composer));
        panel.into_any_element()
    }
}
