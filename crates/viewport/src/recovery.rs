use super::*;
use crate::localization::message as tr;
use crate::shell::{column, control, label};
use zuse_i18n::messages as msg;
use zuse_protocol::recovery::{RecoveryAction, ResolveRecovery};

impl WorkspaceView {
    pub(super) fn reset_recovery(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.recovery_request.take() {
            self.cancel(id);
        }
        self.recovery = None;
        self.recovery_selected = None;
        self.recovery_error = None;
        self.recovery_notice = None;
        self.recovery_output
            .update(cx, |input, cx| input.set_value(String::new(), cx));
    }
    pub(super) fn inspect_recovery(&mut self) {
        if self.recovery_request.is_some() {
            return;
        }
        let Some(chat) = &self.selected else {
            return;
        };
        self.recovery_error = None;
        self.recovery_notice = None;
        self.recovery_request = Some(self.call(
            methods::INSPECT_RECOVERY,
            json!({"chatId":chat["id"],"targetDeviceId":chat["deviceId"]}),
        ));
    }
    fn resolve_recovery(&mut self, decision: RecoveryAction) {
        if self.recovery_request.is_some() {
            return;
        }
        let (Some(chat), Some(snapshot)) = (&self.selected, &self.recovery) else {
            return;
        };
        let Some(call) = self.recovery_selected.and_then(|i| snapshot.calls.get(i)) else {
            return;
        };
        let mut params = serde_json::to_value(ResolveRecovery {
            chat_id: chat["id"].as_str().unwrap_or_default().into(),
            session_id: snapshot.session_id.clone(),
            agent_id: call.agent_id.clone(),
            call_id: call.call_id.clone(),
            revision: call.revision.clone(),
            decision,
        })
        .unwrap();
        params["targetDeviceId"] = chat["deviceId"].clone();
        self.recovery_error = None;
        self.recovery_notice = None;
        self.recovery_request = Some(self.call(methods::RESOLVE_RECOVERY, params));
    }
    pub(super) fn receive_recovery(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.recovery_request != Some(id) {
            return false;
        }
        if let Some(error) = frame["err"].as_str() {
            self.recovery_error = Some(error.into());
        }
        if let Some(value) = frame.get("ok") {
            match serde_json::from_value::<zuse_protocol::recovery::RecoverySnapshot>(value.clone())
            {
                Ok(snapshot) => {
                    let resolving = self
                        .pending
                        .get(&id)
                        .is_some_and(|method| method == methods::RESOLVE_RECOVERY);
                    let previous = self
                        .recovery
                        .as_ref()
                        .and_then(|old| {
                            self.recovery_selected
                                .and_then(|index| old.calls.get(index))
                        })
                        .map(|call| {
                            (
                                call.agent_id.clone(),
                                call.call_id.clone(),
                                call.revision.clone(),
                            )
                        });
                    self.recovery_selected = if resolving {
                        None
                    } else {
                        previous.and_then(|(agent, call, revision)| {
                            snapshot.calls.iter().position(|item| {
                                item.agent_id == agent
                                    && item.call_id == call
                                    && item.revision == revision
                            })
                        })
                    };
                    self.recovery = Some(snapshot);
                    if resolving {
                        self.recovery_output
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                        self.recovery_notice =
                            Some("Review saved. Continue in chat when ready.".into());
                    }
                }
                Err(_) => {
                    self.recovery_error =
                        Some("The engine returned an invalid recovery review.".into())
                }
            }
        }
        if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true {
            self.recovery_request = None;
            self.pending.remove(&id);
        }
        cx.notify();
        true
    }
    pub(super) fn recovery_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut view=column().id("recovery-panel").flex_1().min_h_0().overflow_y_scroll().p_3().gap_3()
            .child(label(tr(cx, msg::native::review_interrupted_tools)))
            .child(label(tr(cx, msg::native::inspect_the_tool_and_its_effects_before_authorizing_a_retry_saving_a_review_does_e9dcec1f)))
            .child(control("refresh-recovery",if self.recovery_request.is_some(){crate::localization::text("common-loading", cx)}else{crate::localization::text("chat-attempts_refresh", cx)})
                .on_click(cx.listener(|this,_,_,cx|{this.inspect_recovery();cx.notify();})));
        if let Some(error) = &self.recovery_error {
            view = view.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(0xf0b78a))
                    .child(SharedString::from(error.clone())),
            );
        }
        if let Some(notice) = &self.recovery_notice {
            view = view.child(label(notice.clone()));
        }
        if let Some(snapshot) = &self.recovery {
            if snapshot.calls.is_empty() {
                view = view.child(label(tr(
                    cx,
                    msg::native::no_uncertain_tool_outcomes_remain,
                )));
            }
            for (index, call) in snapshot.calls.iter().enumerate() {
                view = view.child(
                    control(
                        ("inspect-recovery-call", index),
                        SharedString::from(format!("{} · {}", call.agent_name, call.tool)),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.recovery_request.is_none() {
                            this.recovery_selected = Some(index);
                            this.recovery_output
                                .update(cx, |input, cx| input.set_value(String::new(), cx));
                            cx.notify();
                        }
                    })),
                );
            }
            if let Some(call) = self.recovery_selected.and_then(|i| snapshot.calls.get(i)) {
                let input = serde_json::to_string_pretty(&call.input).unwrap_or_default();
                view = view
                    .child(label(format!(
                        "{} · {}\n{}",
                        call.agent_name, call.tool, call.call_id
                    )))
                    .child(
                        column()
                            .id("recovery-call-input")
                            .max_h(px(320.))
                            .overflow_y_scroll()
                            .font_family("Geist Mono")
                            .text_size(px(12.))
                            .child(SharedString::from(input.clone())),
                    )
                    .child(
                        control("copy-recovery-input", tr(cx, msg::native::copy_full_input))
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(input.clone()))
                            }),
                    )
                    .child(
                        control(
                            "authorize-recovery-retry",
                            tr(cx, msg::native::authorize_retry_after_inspection),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.resolve_recovery(RecoveryAction::RetryAfterInspection);
                            cx.notify();
                        })),
                    )
                    .child(self.recovery_output.clone())
                    .child(
                        control(
                            "save-recovery-outcome",
                            tr(cx, msg::native::save_inspected_outcome),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            let output = this.recovery_output.read(cx).value(cx);
                            this.resolve_recovery(RecoveryAction::RecordOutcome { output });
                            cx.notify();
                        })),
                    );
            }
        }
        view.into_any_element()
    }
}
