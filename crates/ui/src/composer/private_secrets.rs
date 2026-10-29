//! Secret entry never borrows the composer, its drafts, or durable question answers.
use super::*;
impl Composer {
    pub(super) fn private_question(&self) -> bool {
        self.wizard
            .as_ref()
            .and_then(|w| w.current())
            .is_some_and(|q| q.id.starts_with("private-secret:"))
    }
    pub(super) fn clear_private_input(&mut self, cx: &mut Context<Self>) {
        if let Some((_, input)) = self.private_input.take() {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
    }
    pub(super) fn prepare_private_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.wizard.as_ref().map(|w| w.request_id.clone()) else {
            return;
        };
        if self
            .private_input
            .as_ref()
            .is_none_or(|(id, _)| id != &request)
        {
            self.clear_private_input(cx);
            let input =
                cx.new(|cx| zuse_viewport::input::TextInput::password("Enter secret…", window, cx));
            cx.subscribe_in(
                &input,
                window,
                |this, _, _: &zuse_viewport::input::Submitted, _, cx| {
                    this.submit_private_secret(false, cx)
                },
            )
            .detach();
            cx.subscribe(&input, |_, _, _: &zuse_viewport::input::Changed, cx| {
                cx.notify()
            })
            .detach();
            self.private_input = Some((request, input));
        }
    }
    pub(super) fn submit_private_secret(&mut self, decline: bool, cx: &mut Context<Self>) {
        let Some(wizard) = self.wizard.clone() else {
            return;
        };
        let Some(question) = wizard.current() else {
            return;
        };
        let Some(id) = question.id.strip_prefix("private-secret:") else {
            return;
        };
        let value = self
            .private_input
            .as_ref()
            .filter(|(r, _)| r == &wizard.request_id)
            .map(|(_, input)| input.read(cx).value(cx))
            .unwrap_or_default();
        if !decline && value.is_empty() {
            return;
        }
        let state = self.state.read(cx);
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let Some(chat) = state.selected_chat_row() else {
            return;
        };
        let chat_id = chat.id.clone();
        let params = serde_json::json!({"chatId":chat_id,"targetDeviceId":chat.device_id,"requestId":wizard.request_id,"secretId":id,"decline":decline,"value":if decline { "" } else { value.as_str() }});
        self.clear_private_input(cx);
        self.answered_requests.insert(wizard.request_id.clone());
        self.wizard = None;
        let draft = self
            .drafts
            .get(&self.current_key)
            .cloned()
            .unwrap_or_default();
        self.input.update(cx, |input, cx| {
            input.set_text(draft, cx);
            input.set_placeholder("Do anything…", cx);
            input.set_key_context(MESSAGE_COMPOSER_CONTEXT, cx);
        });
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call_scoped(methods::SUBMIT_PRIVATE_SECRET, params)
                .await;
            if result.is_err() {
                this.update(cx, |this, cx| {
                    this.failure = Some(
                        "Private secret submission failed or expired. Re-enter the token to retry."
                            .into(),
                    );
                    this.failure_key = Some(chat_id.clone());
                    this.answered_requests.remove(&wizard.request_id);
                    if this.state.read(cx).selected_chat.as_deref() == Some(chat_id.as_str()) {
                        this.wizard = Some(wizard);
                    }
                    cx.notify();
                })
                .ok();
            }
        }));
        cx.notify();
    }
}

#[cfg(feature = "appshots-fixture")]
impl Composer {
    pub fn fixture_question_answer(&mut self, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| {
            input.set_text(
                "Generate release notes from merged pull requests, grouped by feature.",
                cx,
            )
        });
    }

    /// Populate the real password editor with a synthetic value for visual QA.
    pub fn fixture_private_secret_token(&mut self, cx: &mut Context<Self>) {
        if let Some((_, input)) = &self.private_input {
            input.update(cx, |input, cx| {
                input.set_value("synthetic-demo-token-not-a-credential".into(), cx)
            });
        }
    }
}
