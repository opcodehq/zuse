//! A cloud message whose delivery the server couldn't confirm. Until it is
//! reviewed the server refuses new runs on the conversation, so instead of a
//! dead end the message shows "Didn't go through" with Retry and Discard.
//!
//! Both send `review-unknown` for the message's command. The server never
//! runs a reviewed unknown later (it is terminal and never redispatched), so
//! Retry then sends the same text as a new message. A message that already
//! has a reply may have run; it only offers Dismiss, never a resend.
use super::*;
use crate::composer::tray::{TrayPill, TrayTone, tray_text_action};
use zuse_protocol::cloud_task_commands::TASK_UNKNOWN_OUTCOME_PREFIX;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::composer) struct Unknown {
    pub(in crate::composer) command: String,
    pub(in crate::composer) message: Option<String>,
    pub(in crate::composer) text: Option<String>,
    attachments: Value,
}

#[derive(Default)]
pub(in crate::composer) struct State {
    pub(in crate::composer) current: Option<Unknown>,
    /// Retry or Discard in flight: its message, and whether it resends.
    review: Option<(Task<()>, Unknown, bool)>,
    /// Reviewed here; a history read from before the review can't bring it back.
    reviewed: HashSet<String>,
    /// Recent admitted runs by command, so Retry resends the same files and settings.
    sent: std::collections::VecDeque<(String, Value)>,
}

/// What a new Send says while an earlier message needs Retry or Discard.
const RESOLVE_FIRST: &str =
    "Your earlier message didn't go through. Retry or discard it first. Your draft is kept.";

impl Composer {
    /// Remember an admitted run so a later Retry can resend it as it was.
    pub(super) fn remember_cloud_run(&mut self, request: &Value) {
        let Some(command) = request["commandId"].as_str() else {
            return;
        };
        let sent = &mut self.cloud.unknown.sent;
        sent.push_back((command.to_owned(), request.clone()));
        while sent.len() > 16 {
            sent.pop_front();
        }
    }
    /// The newest message the server still holds as outcome-unknown.
    pub(super) fn observe_cloud_unknown(&mut self, admissions: &Value, cx: &mut Context<Self>) {
        let reviewed = &self.cloud.unknown.reviewed;
        let found = admissions
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| {
                row["executionState"] == "outcome-unknown"
                    && row["unknownReviewed"] != true
                    && row["commandId"]
                        .as_str()
                        .is_some_and(|command| !command.is_empty() && !reviewed.contains(command))
            })
            .max_by_key(|row| row["createdAt"].as_i64().unwrap_or_default())
            .map(|row| Unknown {
                command: row["commandId"].as_str().unwrap_or_default().to_owned(),
                message: row["messageId"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned),
                text: row["text"].as_str().map(str::to_owned),
                attachments: row.get("attachments").cloned().unwrap_or(json!([])),
            });
        if self.cloud.unknown.current != found {
            self.cloud.unknown.current = found;
            cx.notify();
        }
    }
    /// True when a new Send must wait for an earlier message's Retry or
    /// Discard; says so and keeps the draft.
    pub(super) fn cloud_unknown_blocks_send(&mut self, cx: &mut Context<Self>) -> bool {
        if self.cloud.unknown.review.is_some() {
            // Retry or Discard is a moment away; the draft stays put.
            return true;
        }
        if self.cloud.unknown.current.is_none() {
            return false;
        }
        self.failure = Some(RESOLVE_FIRST.into());
        self.failure_key = None;
        cx.notify();
        true
    }
    pub(super) fn cloud_unknown_retrying(&self) -> bool {
        self.cloud
            .unknown
            .review
            .as_ref()
            .is_some_and(|(_, _, retry)| *retry)
    }
    /// The message got a reply, so it may have run: never resend it.
    fn cloud_unknown_ran(&self, unknown: &Unknown, cx: &App) -> bool {
        unknown.message.as_deref().is_some_and(|message| {
            zuse_viewport::cloud_response_started(&self.state.read(cx).transcript, message)
        })
    }
    fn cloud_unknown_can_retry(&self, unknown: &Unknown, cx: &App) -> bool {
        unknown.message.is_some() && unknown.text.is_some() && !self.cloud_unknown_ran(unknown, cx)
    }
    pub(in crate::composer) fn cloud_unknown_row(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<TrayPill> {
        if self.cloud.unknown.review.is_some() {
            return None;
        }
        let unknown = self.cloud.unknown.current.as_ref()?;
        let retry = self.cloud_unknown_can_retry(unknown, cx);
        let pill = if retry {
            let text = unknown.text.as_deref().unwrap_or_default().trim();
            TrayPill::new("cloud-unknown-outcome", "Didn't go through")
                .subtitle(format!("“{text}”"))
                .action(
                    tray_text_action("retry-cloud-unknown", "Retry", None, theme)
                        .on_click(cx.listener(|this, _, _, cx| this.retry_cloud_unknown(cx))),
                )
        } else if unknown.message.is_some() {
            TrayPill::new("cloud-unknown-outcome", "Couldn't confirm this message")
                .subtitle("It may have run. Check its reply.")
        } else {
            TrayPill::new("cloud-unknown-outcome", "Couldn't confirm the last action")
        };
        Some(
            pill.tone(TrayTone::Warning)
                .glyph(crate::icons::ALERT_CIRCLE)
                .action(
                    tray_text_action(
                        "discard-cloud-unknown",
                        if retry { "Discard" } else { "Dismiss" },
                        None,
                        theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.discard_cloud_unknown(cx))),
                ),
        )
    }
    /// Review the unknown message, then send its text again as a new message.
    /// It joins the conversation at once, like any Send.
    pub(in crate::composer) fn retry_cloud_unknown(&mut self, cx: &mut Context<Self>) {
        let Some(unknown) = self.cloud.unknown.current.clone() else {
            return;
        };
        if self.cloud.unknown.review.is_some()
            || self.cloud.task.is_some()
            || !self.cloud_unknown_can_retry(&unknown, cx)
        {
            return;
        }
        if self.cloud.pending_native.is_some() {
            // A newer send's delivery is still unconfirmed; its own retry
            // keeps that identity, so it goes first.
            self.failure = Some(
                "Send your draft again first; its delivery isn't confirmed yet. Then Retry.".into(),
            );
            self.failure_key = None;
            cx.notify();
            return;
        }
        let Some(task) = self.cloud.native.as_ref() else {
            return;
        };
        let session = task.target.initial_session.clone();
        let text = unknown.text.clone().unwrap_or_default();
        let command = uuid::Uuid::new_v4().to_string();
        let message = uuid::Uuid::new_v4().to_string();
        let original = self
            .cloud
            .unknown
            .sent
            .iter()
            .find(|(sent, _)| *sent == unknown.command)
            .map(|(_, request)| request.clone());
        let request = if let Some(mut request) = original {
            // The same prompt, files and settings under a new identity.
            request["commandId"] = json!(command);
            request["action"]["messageId"] = json!(message);
            request["action"]["payload"]["messageId"] = json!(message);
            request["wake"] = json!(wake::intent(false, task.wake));
            request
        } else {
            let Some(resolved) = self.pickers.read(cx).cloud_task_choice() else {
                self.failure = Some(
                    "This task's saved agent settings are not available yet. Try again in a moment."
                        .into(),
                );
                cx.notify();
                return;
            };
            let (Some(harness), Some(model)) = (resolved.harness, resolved.model.clone()) else {
                self.failure =
                    Some("Choose an available agent and model for this task, then Retry.".into());
                cx.notify();
                return;
            };
            let sandbox = resolved.sandbox;
            json!({"commandId":command,"sessionId":session,"wake":wake::intent(false, task.wake),
            "action":{"type":"submit","messageId":message,"attachments":unknown.attachments,
                "payload":{"kind":"run","messageId":message,"request":{
                    "prompt":text,"harness":harness,"model":model,"reasoning":resolved.reasoning,
                    "modelOptions":resolved.model_options,"cwd":".","sandbox":sandbox,
                    "autoApprove":sandbox==SandboxLevel::DangerFullAccess,"resume":null,"attachments":[]
                }}}})
        };
        self.cloud.pending_native = Some(request);
        self.cloud.pending_native_text = Some(text.clone());
        self.cloud.native_staged.clear();
        self.failure = None;
        self.state.update(cx, |state, cx| {
            if let Some(old) = unknown.message.as_deref() {
                state.remove_admission_message(&session, old);
            }
            cx.notify();
        });
        self.push_cloud_echo(&session, &message, &text, cx);
        self.review_cloud_unknown(unknown, true, cx);
    }
    /// Review the unknown message and drop it; its text returns to an empty
    /// draft so nothing typed is lost.
    pub(in crate::composer) fn discard_cloud_unknown(&mut self, cx: &mut Context<Self>) {
        let Some(unknown) = self.cloud.unknown.current.clone() else {
            return;
        };
        if self.cloud.unknown.review.is_some() {
            return;
        }
        let restore = self.cloud_unknown_can_retry(&unknown, cx);
        if let Some(text) = unknown.text.as_deref().filter(|_| restore)
            && self.input.read(cx).text().is_empty()
        {
            self.input
                .update(cx, |input, cx| input.set_text(text.to_owned(), cx));
        }
        if let (Some(message), Some(task)) = (unknown.message.as_deref(), &self.cloud.native) {
            let session = task.target.initial_session.clone();
            self.state.update(cx, |state, cx| {
                state.remove_admission_message(&session, message);
                cx.notify();
            });
        }
        self.failure = None;
        self.review_cloud_unknown(unknown, false, cx);
    }
    fn review_cloud_unknown(&mut self, unknown: Unknown, retry: bool, cx: &mut Context<Self>) {
        let Some(task) = self.cloud.native.as_ref() else {
            return;
        };
        let target = task.target.clone();
        let request = json!({"commandId":uuid::Uuid::new_v4().to_string(),"sessionId":target.initial_session,
            "action":{"type":"review-unknown","interruptedCommandId":unknown.command}});
        let reviewing = unknown.clone();
        let work = cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .call_scoped(
                    methods::CLOUD_TASK_COMMAND,
                    json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,
                        "expectedOrganizationId":target.organization,"request":request}),
                )
                .await;
            let reviewed = match &result {
                Ok(value) => {
                    value["accountId"] == target.subject
                        && value["organizationId"].as_str() == target.organization.as_deref()
                        && value["data"]["reviewed"] == true
                }
                // Someone else already reviewed it: it won't run either way.
                Err(zuse_rpc::RpcError::Capability(code)) => {
                    code.strip_prefix(TASK_UNKNOWN_OUTCOME_PREFIX) == Some("not-found")
                }
                Err(_) => false,
            };
            let _ = this.update(cx, |this, cx| {
                this.cloud.unknown.review = None;
                if reviewed {
                    this.cloud.unknown.reviewed.insert(unknown.command.clone());
                    if this
                        .cloud
                        .unknown
                        .current
                        .as_ref()
                        .is_some_and(|current| current.command == unknown.command)
                    {
                        this.cloud.unknown.current = None;
                    }
                    if let Some(message) = unknown.message.as_deref() {
                        let session = target.initial_session.clone();
                        this.state.update(cx, |state, cx| {
                            state.remove_admission_message(&session, message);
                            cx.notify();
                        });
                    }
                    if retry {
                        this.dispatch_cloud_task(cx);
                    } else {
                        cx.emit(ComposerEvent::CloudLaunchChanged);
                    }
                } else {
                    let error = match result {
                        Err(error) => error.to_string(),
                        Ok(_) => "the server didn't confirm it.".into(),
                    };
                    if retry {
                        // Nothing was sent: the new copy leaves, the message
                        // keeps its Retry and Discard.
                        if let Some(message) = this
                            .cloud
                            .pending_native
                            .as_ref()
                            .and_then(|request| request["action"]["messageId"].as_str())
                            .map(str::to_owned)
                        {
                            let session = target.initial_session.clone();
                            this.state
                                .update(cx, |state, _| state.remove_echo(&session, &message));
                        }
                        this.cloud.pending_native = None;
                        this.cloud.pending_native_text = None;
                    } else if unknown
                        .text
                        .as_deref()
                        .is_some_and(|text| this.input.read(cx).text() == text)
                    {
                        this.input.update(cx, |input, cx| input.set_text("", cx));
                    }
                    this.failure = Some(
                        format!(
                            "Couldn't {} the message: {error}",
                            if retry { "retry" } else { "discard" }
                        )
                        .into(),
                    );
                    this.failure_key = None;
                    // A history read brings the message back if it is still there.
                    cx.emit(ComposerEvent::CloudLaunchChanged);
                }
                cx.notify();
            });
        });
        self.cloud.unknown.review = Some((work, reviewing, retry));
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cloud task composer whose RPC requests land in the returned channel.
    fn composer_with_requests(
        cx: &mut gpui::TestAppContext,
    ) -> (
        Entity<Composer>,
        Entity<AppState>,
        tokio::sync::mpsc::Receiver<String>,
        tokio::sync::mpsc::Sender<String>,
    ) {
        let (send, receive) = tokio::sync::mpsc::channel(16);
        let (replies_tx, replies) = tokio::sync::mpsc::channel(16);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            state.selected_chat = Some("session".into());
            state
        });
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        composer.update(cx, |c, cx| {
            let target = crate::state::CloudTarget {
                controller,
                subject: "owner".into(),
                organization: None,
                workspace: "task".into(),
                initial_session: "session".into(),
            };
            c.set_cloud_task(
                target,
                json!({"agent":"codex","model":"gpt-6.1-sol","runtimeMode":"full-access"}),
                cx,
            );
        });
        (composer, state, receive, replies_tx)
    }
    fn unknown_rows() -> Value {
        json!([{"sessionId":"session","commandId":"lost","messageId":"lost-message","text":"Fix the build",
            "attachments":[],"createdAt":1,"executionState":"outcome-unknown","unknownReviewed":false}])
    }
    /// The next request the composer sent, if any.
    fn next(requests: &mut tokio::sync::mpsc::Receiver<String>) -> Option<Value> {
        requests
            .try_recv()
            .ok()
            .map(|line| serde_json::from_str(line.trim()).expect("request frame"))
    }
    fn reply(
        runtime: &tokio::runtime::Runtime,
        replies: &tokio::sync::mpsc::Sender<String>,
        frame: Value,
    ) {
        runtime
            .block_on(replies.send(format!("{frame}\n")))
            .unwrap();
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }
    fn settle(cx: &mut gpui::TestAppContext, runtime: &tokio::runtime::Runtime) {
        // The RPC client routes replies on this thread's runtime; drive both
        // executors so no other thread wakes the deterministic scheduler.
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async { tokio::task::yield_now().await });
        }
        cx.run_until_parked();
    }

    #[gpui::test]
    fn retry_reviews_the_unknown_message_then_sends_it_again(cx: &mut gpui::TestAppContext) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, state, mut requests, replies) = composer_with_requests(cx);
        composer.update(cx, |c, cx| {
            c.observe_cloud_unknown(&unknown_rows(), cx);
            assert_eq!(
                c.cloud.unknown.current.as_ref().map(|u| u.command.as_str()),
                Some("lost")
            );
            c.retry_cloud_unknown(cx);
            assert_eq!(
                c.cloud_activity(),
                Some(("Sending message…", true)),
                "Retry shows progress at once"
            );
            let echoed = state.read(cx).pending_echoes().iter().any(|entry| {
                matches!(&entry.parts[..], [zuse_model::MessagePart::Text { text, .. }] if text == "Fix the build")
            });
            assert!(echoed, "the retried message joins the chat at once");
        });
        settle(cx, &runtime);
        let review = next(&mut requests).expect("review request sent");
        assert_eq!(review["method"], methods::CLOUD_TASK_COMMAND);
        assert_eq!(
            review["params"]["request"]["action"],
            json!({"type":"review-unknown","interruptedCommandId":"lost"})
        );
        assert!(
            next(&mut requests).is_none(),
            "nothing resends before the review"
        );
        reply(
            &runtime,
            &replies,
            json!({"id":review["id"],"ok":{"accountId":"owner","organizationId":null,
                "data":{"reviewed":true,"interruptedCommandId":"lost","commandId":review["params"]["request"]["commandId"],"durablyAdmitted":true}}}),
        );
        settle(cx, &runtime);
        let resend = next(&mut requests).expect("the message is sent again");
        let action = &resend["params"]["request"]["action"];
        assert_eq!(action["type"], "submit");
        assert_eq!(action["payload"]["request"]["prompt"], "Fix the build");
        assert_ne!(action["messageId"], "lost-message");
        assert_ne!(resend["params"]["request"]["commandId"], "lost");
        composer.update(cx, |c, cx| {
            assert!(c.cloud.unknown.current.is_none());
            // A history read from before the review can't bring it back.
            c.observe_cloud_unknown(&unknown_rows(), cx);
            assert!(c.cloud.unknown.current.is_none());
            c.cloud.task = None;
        });
    }

    #[gpui::test]
    fn discard_reviews_the_unknown_message_and_restores_its_text(cx: &mut gpui::TestAppContext) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, _state, mut requests, replies) = composer_with_requests(cx);
        composer.update(cx, |c, cx| {
            c.observe_cloud_unknown(&unknown_rows(), cx);
            c.discard_cloud_unknown(cx);
            assert_eq!(
                c.input.read(cx).text(),
                "Fix the build",
                "text returns at once"
            );
        });
        settle(cx, &runtime);
        let review = next(&mut requests).expect("review request sent");
        assert_eq!(
            review["params"]["request"]["action"],
            json!({"type":"review-unknown","interruptedCommandId":"lost"})
        );
        // Someone else reviewed it first: the message is resolved all the same.
        reply(
            &runtime,
            &replies,
            json!({"id":review["id"],"err":format!("capability error: {TASK_UNKNOWN_OUTCOME_PREFIX}not-found")}),
        );
        settle(cx, &runtime);
        assert!(next(&mut requests).is_none(), "Discard never resends");
        composer.update(cx, |c, cx| {
            assert!(c.cloud.unknown.current.is_none());
            assert!(c.failure.is_none());
            assert_eq!(c.input.read(cx).text(), "Fix the build");
            // New sends work again.
            assert!(c.send_cloud_task("Fix the build".into(), false, cx));
            assert!(c.cloud.pending_native.is_some());
            c.cloud.task = None;
        });
    }

    #[gpui::test]
    fn a_new_send_while_a_message_is_unknown_offers_retry_instead_of_busy(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, _state, mut requests, _replies) = composer_with_requests(cx);
        composer.update(cx, |c, cx| {
            c.observe_cloud_unknown(&unknown_rows(), cx);
            c.input
                .update(cx, |input, cx| input.set_text("Something new", cx));
            assert!(c.send_cloud_task("Something new".into(), false, cx));
            assert!(
                c.cloud.pending_native.is_none(),
                "nothing is sent to be refused"
            );
            assert_eq!(
                c.input.read(cx).text(),
                "Something new",
                "the draft is kept"
            );
            let failure = c.failure.as_deref().unwrap_or_default().to_owned();
            assert!(failure.contains("Retry or discard"), "{failure}");
            assert!(!failure.contains("busy"));
        });
        settle(cx, &runtime);
        assert!(next(&mut requests).is_none());
    }

    #[gpui::test]
    fn a_message_that_already_has_a_reply_is_never_resent(cx: &mut gpui::TestAppContext) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, state, mut requests, _replies) = composer_with_requests(cx);
        state.update(cx, |state, _| {
            state.apply_transcript(
                serde_json::from_value(json!([
                    {"id":"lost-message","role":"user","parts":[{"kind":"text","id":"t","text":"Fix the build"}],"createdAt":1,"deviceId":"runtime"},
                    {"id":"reply","role":"assistant","parts":[{"kind":"text","id":"r","text":"Fixed it."}],"createdAt":2,"deviceId":"runtime"}
                ]))
                .unwrap(),
            )
        });
        composer.update(cx, |c, cx| {
            c.observe_cloud_unknown(&unknown_rows(), cx);
            c.retry_cloud_unknown(cx);
            assert!(
                c.cloud.pending_native.is_none(),
                "it may have run: no resend"
            );
            c.discard_cloud_unknown(cx);
            assert_eq!(
                c.input.read(cx).text(),
                "",
                "a message that ran isn't restored"
            );
        });
        settle(cx, &runtime);
        let review = next(&mut requests).expect("Dismiss still reviews it");
        assert_eq!(
            review["params"]["request"]["action"]["type"],
            "review-unknown"
        );
        assert!(next(&mut requests).is_none());
        composer.update(cx, |c, _| c.cloud.unknown.review = None);
    }
}
