//! Approval and question controls use the engine's durable respond-input command.
use super::*;
use crate::localization::message as tr;
use crate::shell::{column, control, label};
use zuse_i18n::messages as msg;
use zuse_protocol::UserInputQuestion;

impl WorkspaceView {
    pub(super) fn reset_inputs(&mut self, cx: &mut Context<Self>) {
        self.secret_text
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.secret_editing = None;
        self.input_answers.clear();
        self.input_pending.clear();
        self.input_submitted.clear();
        self.input_editing = None;
        self.expanded_parts.clear();
        self.input_text
            .update(cx, |input, cx| input.set_value(String::new(), cx));
    }
    pub(super) fn clear_closed_secret(&mut self, cx: &mut Context<Self>) {
        let pending = self.pending_composer_question().is_some_and(|(id, qs)| {
            self.secret_editing.as_deref() == Some(id.as_str())
                && qs[0].id.starts_with("private-secret:")
        });
        if self.secret_editing.is_some() && !pending {
            self.secret_text
                .update(cx, |input, cx| input.set_value(String::new(), cx));
            self.secret_editing = None;
        }
        if let Some((request, questions)) = self.pending_composer_question() {
            if !questions[0].id.starts_with("private-secret:")
                && self
                    .input_editing
                    .as_ref()
                    .is_none_or(|(r, _)| r != &request)
            {
                self.input_text.update(cx, |i, cx| {
                    i.set_value(questions[0].prefill.clone().unwrap_or_default(), cx)
                });
                self.input_editing = Some((request.clone(), questions[0].id.clone()));
            }
            if questions[0].id.starts_with("private-secret:")
                && self.secret_editing.as_deref() != Some(request.as_str())
                && !self.input_submitted.contains(&request)
            {
                self.secret_text
                    .update(cx, |i, cx| i.set_value(String::new(), cx));
                self.secret_editing = Some(request);
            }
        }
    }
    pub(super) fn pending_composer_question(&self) -> Option<(String, Vec<UserInputQuestion>)> {
        self.messages
            .iter()
            .rev()
            .find(|m| m.role == zuse_model::MessageRole::Assistant)
            .and_then(|m| {
                m.parts.iter().find_map(|p| match p {
                    zuse_model::MessagePart::Input {
                        request_id,
                        questions,
                        resolved: false,
                        ..
                    } if !questions.is_empty() => Some((request_id.clone(), questions.clone())),
                    _ => None,
                })
            })
    }
    pub(super) fn send_question_answer(&mut self, cx: &mut Context<Self>) {
        let Some((request, questions)) = self.pending_composer_question() else {
            return;
        };
        if self.transcript_blocked() || self.input_submitted.contains(&request) {
            return;
        }
        if let Some(id) = questions[0].id.strip_prefix("private-secret:") {
            self.submit_secret_composer(&request, id, false, cx);
            return;
        }
        let index = self
            .input_editing
            .as_ref()
            .filter(|(r, _)| r == &request)
            .and_then(|(_, id)| questions.iter().position(|q| &q.id == id))
            .unwrap_or(0);
        self.stash_question_text(&request, &questions[index], cx);
        let answered = self
            .input_answers
            .get(&request)
            .is_some_and(|a| !a.selected(&questions[index].id).is_empty());
        if !answered {
            return;
        }
        if index + 1 < questions.len() {
            let next = &questions[index + 1];
            let text = self
                .input_answers
                .get(&request)
                .map(|a| {
                    a.selected(&next.id)
                        .iter()
                        .filter(|v| !next.options.contains(v))
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|s| !s.is_empty())
                .or_else(|| next.prefill.clone())
                .unwrap_or_default();
            self.input_editing = Some((request, next.id.clone()));
            self.input_text
                .update(cx, |input, cx| input.set_value(text, cx));
        } else {
            self.submit_input(request, questions, cx);
        }
        cx.notify();
    }
    fn stash_question_text(
        &mut self,
        request: &str,
        q: &UserInputQuestion,
        cx: &mut Context<Self>,
    ) {
        let text = self.input_text.read(cx).value(cx);
        let answers = self.input_answers.entry(request.into()).or_default();
        if !text.trim().is_empty()
            || answers
                .selected(&q.id)
                .iter()
                .any(|v| !q.options.contains(v))
        {
            answers.set_text(&q.id, &text);
        }
    }
    fn submit_secret_composer(
        &mut self,
        request: &str,
        id: &str,
        decline: bool,
        cx: &mut Context<Self>,
    ) {
        if self.transcript_blocked() || self.input_submitted.contains(request) {
            return;
        }
        let Some(chat) = &self.selected else {
            return;
        };
        let value = if decline {
            String::new()
        } else {
            self.secret_text.read(cx).value(cx)
        };
        if !decline && value.is_empty() {
            return;
        }
        let params = json!({"chatId":chat["id"],"targetDeviceId":chat["deviceId"],"requestId":request,"secretId":id,"decline":decline,"value":value});
        let call = self.call(methods::SUBMIT_PRIVATE_SECRET, params);
        self.secret_text
            .update(cx, |i, cx| i.set_value(String::new(), cx));
        self.secret_editing = None;
        self.input_pending.insert(call, request.into());
        self.input_submitted.insert(request.into());
        cx.notify();
    }
    pub(super) fn question_composer(
        &self,
        request: &str,
        questions: &[UserInputQuestion],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::shell::row;
        let index = self
            .input_editing
            .as_ref()
            .filter(|(r, _)| r == request)
            .and_then(|(_, id)| questions.iter().position(|q| &q.id == id))
            .unwrap_or(0);
        let q = &questions[index];
        let secret = q.id.starts_with("private-secret:");
        let busy = self.input_submitted.contains(request);
        let mut tray = column()
            .gap_1()
            .px_3()
            .pt_3()
            .pb_2()
            .w(gpui::relative(14. / 15.))
            .self_center()
            .rounded_t(px(crate::composer_style::RADIUS))
            .border_1()
            .border_b_0()
            .border_color(crate::palette::composer_border())
            .bg(crate::palette::composer_background())
            .child(
                row()
                    .gap_2()
                    .child(
                        div()
                            .id("question-composer-prompt")
                            .flex_1()
                            .max_h(px(220.))
                            .overflow_y_scroll()
                            .text_size(px(13.))
                            .line_height(px(20.))
                            .child(q.question.clone()),
                    )
                    .when(questions.len() > 1, |d| {
                        d.child(label(format!("{}/{}", index + 1, questions.len())))
                    }),
            );
        if q.multi_select {
            tray = tray.child(label(tr(cx, msg::chat::question_card_multiple_choice_hint)));
        }
        for (i, option) in q.options.iter().enumerate() {
            let request = request.to_owned();
            let question = q.clone();
            let option = option.clone();
            let picked = self
                .input_answers
                .get(&request)
                .is_some_and(|a| a.selected(&q.id).contains(&option));
            tray = tray.child(
                control(
                    SharedString::from(format!("question-pick-{i}")),
                    SharedString::from(format!(
                        "{}  {}",
                        if picked {
                            "✓".into()
                        } else {
                            (i + 1).to_string()
                        },
                        option
                    )),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !busy {
                        this.input_answers
                            .entry(request.clone())
                            .or_default()
                            .choose(&question, &option);
                        cx.notify();
                    }
                })),
            );
        }
        let mut footer = row().gap_2().child(
            label(if secret {
                "Private · used once · kept out of chat"
            } else {
                "Type an answer or choose an option above"
            })
            .flex_1(),
        );
        if secret {
            let request = request.to_owned();
            let id = q.id.strip_prefix("private-secret:").unwrap().to_owned();
            footer = footer.child(
                control(
                    "decline-composer-secret",
                    tr(cx, msg::projects::masters_decline),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.submit_secret_composer(&request, &id, true, cx)
                })),
            );
        }
        if index > 0 {
            let request = request.to_owned();
            let id = questions[index - 1].id.clone();
            footer = footer.child(
                control(
                    "question-composer-back",
                    tr(cx, msg::chat::checkpoints_back),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some((_, qs)) = this.pending_composer_question() {
                        let active = this
                            .input_editing
                            .as_ref()
                            .and_then(|(_, qid)| qs.iter().find(|q| &q.id == qid))
                            .unwrap_or(&qs[0])
                            .clone();
                        this.stash_question_text(&request, &active, cx);
                    }
                    this.input_editing = Some((request.clone(), id.clone()));
                    let question = this
                        .pending_composer_question()
                        .and_then(|(_, qs)| qs.into_iter().find(|q| q.id == id));
                    let text = question
                        .as_ref()
                        .map(|q| {
                            this.input_answers
                                .get(&request)
                                .map(|a| {
                                    a.selected(&id)
                                        .iter()
                                        .filter(|v| !q.options.contains(v))
                                        .cloned()
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                })
                                .unwrap_or_default()
                        })
                        .unwrap_or_default();
                    this.input_text.update(cx, |i, cx| i.set_value(text, cx));
                    cx.notify();
                })),
            );
        }
        footer = footer.child(
            control(
                "question-composer-send",
                if busy {
                    ("Sending…").to_string()
                } else if secret {
                    ("Approve and use").to_string()
                } else if index + 1 == questions.len() {
                    crate::localization::text("chat-question_card_submit_answer", cx)
                } else {
                    crate::localization::text("chat-question_card_continue", cx)
                },
            )
            .on_click(cx.listener(|this, _, _, cx| this.send_question_answer(cx))),
        );
        column()
            .w_full()
            .max_w(px(crate::composer_style::CONVERSATION_WIDTH))
            .self_center()
            .mx_3()
            .mb_3()
            .child(tray)
            .child(
                column()
                    .rounded(px(crate::composer_style::RADIUS))
                    .border_1()
                    .border_color(crate::palette::composer_border())
                    .bg(crate::palette::composer_background())
                    .p_3()
                    .gap_3()
                    .child(if secret {
                        self.secret_text.clone()
                    } else {
                        self.input_text.clone()
                    })
                    .child(footer),
            )
            .into_any_element()
    }
    pub(super) fn receive_input(&mut self, id: u64, frame: &Value) {
        if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true {
            if let Some(request) = self.input_pending.remove(&id) {
                if frame.get("err").is_some() {
                    self.input_submitted.remove(&request);
                }
            }
        }
    }
    fn submit_input(
        &mut self,
        request: String,
        questions: Vec<UserInputQuestion>,
        cx: &mut Context<Self>,
    ) {
        if self.transcript_blocked() {
            return;
        }
        if self.input_submitted.contains(&request) {
            return;
        }
        let Some(chat) = &self.selected else {
            return;
        };
        let Some(answers) = self
            .input_answers
            .get(&request)
            .and_then(|answers| answers.complete(&questions))
        else {
            return;
        };
        let id=self.call(methods::QUEUE_COMMAND,json!({"chatId":chat["id"],"targetDeviceId":chat["deviceId"],"command":{"kind":"respondInput","requestId":request,"answers":answers}}));
        self.input_pending.insert(id, request.clone());
        self.input_submitted.insert(request);
        cx.notify();
    }
    pub(super) fn input_card(&self, part: &Value, _cx: &mut Context<Self>) -> AnyElement {
        let questions = serde_json::from_value::<Vec<UserInputQuestion>>(part["questions"].clone())
            .unwrap_or_default();
        column()
            .gap_2()
            .p_3()
            .child(label(if part["resolved"] == true {
                "Answered"
            } else {
                "Question"
            }))
            .children(
                questions
                    .iter()
                    .map(|q| div().text_size(px(13.)).child(q.question.clone())),
            )
            .into_any_element()
    }
}
