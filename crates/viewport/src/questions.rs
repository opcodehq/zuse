//! Approval and question controls use the engine's durable respond-input command.
use super::*;
use crate::shell::{column, control, label};
use zuse_protocol::UserInputQuestion;

impl WorkspaceView {
    pub(super) fn reset_inputs(&mut self, cx: &mut Context<Self>) {
        self.input_answers.clear();
        self.input_pending.clear();
        self.input_submitted.clear();
        self.input_editing = None;
        self.expanded_parts.clear();
        self.input_text
            .update(cx, |input, cx| input.set_value(String::new(), cx));
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
    pub(super) fn input_card(&self, part: &Value, cx: &mut Context<Self>) -> AnyElement {
        let request = part["requestId"].as_str().unwrap_or_default().to_owned();
        let resolved = part["resolved"] == true;
        let questions = serde_json::from_value::<Vec<UserInputQuestion>>(part["questions"].clone())
            .unwrap_or_default();
        let busy = self.input_submitted.contains(&request);
        let ready = self
            .input_answers
            .get(&request)
            .and_then(|answers| answers.complete(&questions))
            .is_some();
        let mut card = column()
            .gap_3()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(0x37302b))
            .bg(rgb(0x141210))
            .child(label(if resolved {
                "Answered"
            } else if busy {
                "Answer sent"
            } else {
                "Your answer is needed"
            }));
        for (index, question) in questions.iter().enumerate() {
            card = card
                .child(
                    div()
                        .text_size(px(12.))
                        .child(SharedString::from(question.header.clone())),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .line_height(px(21.))
                        .child(SharedString::from(question.question.clone())),
                );
            if resolved {
                continue;
            }
            for (option_index, option) in question.options.iter().enumerate() {
                let selected = self
                    .input_answers
                    .get(&request)
                    .is_some_and(|answers| answers.selected(&question.id).contains(option));
                let request_id = request.clone();
                let question = question.clone();
                let option = option.clone();
                card = card.child(
                    control(
                        SharedString::from(format!("question-{request}-{index}-{option_index}")),
                        SharedString::from(option.clone()),
                    )
                    .when(selected, |button| {
                        button.bg(rgb(0x35302a)).text_color(rgb(0xf0d2b8))
                    })
                    .when(busy, |button| button.opacity(0.5))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.input_submitted.contains(&request_id) {
                            this.input_answers
                                .entry(request_id.clone())
                                .or_default()
                                .choose(&question, &option);
                            cx.notify();
                        }
                    })),
                );
            }
            let request_id = request.clone();
            let question_id = question.id.clone();
            let prefill = question.prefill.clone().unwrap_or_default();
            card = card.child(
                control(
                    SharedString::from(format!("write-question-answer-{request}-{index}")),
                    "Write an answer",
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    if this.input_submitted.contains(&request_id) {
                        return;
                    }
                    let text = this
                        .input_answers
                        .get(&request_id)
                        .map(|answers| answers.selected(&question_id).join("\n"))
                        .filter(|text| !text.is_empty())
                        .unwrap_or_else(|| prefill.clone());
                    this.input_editing = Some((request_id.clone(), question_id.clone()));
                    this.input_text
                        .update(cx, |input, cx| input.set_value(text, cx));
                    window.focus(&this.input_text.read(cx).focus_handle(cx), cx);
                    cx.notify();
                })),
            );
            if self
                .input_editing
                .as_ref()
                .is_some_and(|(r, q)| r == &request && q == &question.id)
            {
                let request_id = request.clone();
                let question_id = question.id.clone();
                card = card.child(self.input_text.clone()).child(
                    control(
                        SharedString::from(format!("use-question-answer-{request}-{index}")),
                        "Use this answer",
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.input_submitted.contains(&request_id) {
                            let text = this.input_text.read(cx).value(cx);
                            this.input_answers
                                .entry(request_id.clone())
                                .or_default()
                                .set_text(&question_id, &text);
                            this.input_editing = None;
                            cx.notify();
                        }
                    })),
                );
            }
        }
        if !resolved {
            card = card.child(
                control(
                    SharedString::from(format!("submit-question-{request}")),
                    if busy { "Sending…" } else { "Submit answers" },
                )
                .when(!ready || busy, |button| button.opacity(0.4))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.submit_input(request.clone(), questions.clone(), cx)
                })),
            );
        }
        card.into_any_element()
    }
}
