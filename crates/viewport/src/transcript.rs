//! A failed read refreshes from the owning host; it never replays a command.
use super::*;
use std::time::Duration;
use web_time::Instant;

#[derive(Default)]
pub(super) struct TranscriptState {
    ready: bool,
    retries: usize,
    generation: uuid::Uuid,
    timer: Option<Task<()>>,
    deadline: Option<Task<()>>,
    healthy_since: Option<Instant>,
    pub notice: Option<String>,
}

impl WorkspaceView {
    pub(super) fn apply_archive_transcript(
        &mut self,
        update: TranscriptUpdate,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        apply_transcript_frame(&mut self.messages, update.frame).map_err(|e| e.to_string())?;
        self.transcript_state.ready = true;
        self.context_usage = update.context_usage;
        cx.notify();
        Ok(())
    }
    pub(super) fn install_cached_history(&mut self, head: &zuse_model::cloud::Head) {
        let mut chat = head.chat.clone();
        chat.archived = true;
        self.selected = serde_json::to_value(chat).ok();
        self.messages.clear();
        if apply_transcript_frame(&mut self.messages, head.transcript.frame.clone()).is_ok() {
            self.transcript_state.ready = true;
            self.context_usage = head.transcript.context_usage;
        }
        self.cached_only = true;
        self.error = None;
    }

    pub(super) fn transcript_refresh_pending(&self) -> bool {
        self.transcript_watch.is_some() || self.transcript_state.timer.is_some()
    }
    pub(super) fn transcript_blocked(&self) -> bool {
        self.selected.is_some() && !self.transcript_state.ready
    }
    pub(super) fn stop_transcript(&mut self) {
        if let Some(id) = self.transcript_watch.take() {
            self.cancel(id);
        }
        self.transcript_state = TranscriptState {
            generation: uuid::Uuid::new_v4(),
            ..Default::default()
        };
        self.context_confirmed = false;
    }
    pub(super) fn begin_transcript(&mut self, cx: &mut Context<Self>) {
        self.stop_transcript();
        self.subscribe_transcript(cx);
    }
    fn subscribe_transcript(&mut self, cx: &mut Context<Self>) {
        if !self.connected {
            return;
        }
        if let Some(chat) = self.selected.clone() {
            self.transcript_watch = Some(self.call(
                methods::WATCH_DOC_MESSAGES,
                zuse_protocol::presentation::chat_request(&chat, json!({})),
            ));
            let id = self.transcript_watch.unwrap();
            let generation = self.transcript_state.generation;
            self.transcript_state.deadline = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_secs(15))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    if this.transcript_watch == Some(id)
                        && this.transcript_state.generation == generation
                        && !this.transcript_state.ready
                    {
                        this.recover_transcript(
                            "The host did not send its initial history snapshot".into(),
                            cx,
                        );
                    }
                });
            }));
        }
    }
    pub(super) fn retry_transcript(&mut self, cx: &mut Context<Self>) {
        if !self.connected {
            return;
        }
        self.begin_transcript(cx);
        self.transcript_state.notice = Some("Refreshing chat history…".into());
        cx.notify();
    }
    fn recover_transcript(&mut self, reason: String, cx: &mut Context<Self>) {
        eprintln!("Chat history refresh failed: {reason}");
        if let Some(id) = self.transcript_watch.take() {
            self.cancel(id);
        }
        if self
            .transcript_state
            .healthy_since
            .is_some_and(|since| since.elapsed() >= Duration::from_secs(10))
        {
            self.transcript_state.retries = 0;
        }
        self.transcript_state.deadline = None;
        self.transcript_state.ready = false;
        self.transcript_state.healthy_since = None;
        self.context_confirmed = false;
        let delays = [200, 600, 1800];
        let Some(delay) = delays.get(self.transcript_state.retries).copied() else {
            self.transcript_state.notice =
                Some("Chat history could not refresh. Your draft is preserved.".into());
            cx.notify();
            return;
        };
        self.transcript_state.retries += 1;
        self.transcript_state.notice =
            Some("Refreshing chat history. Your draft is preserved…".into());
        let generation = self.transcript_state.generation;
        self.transcript_state.timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.transcript_state.generation != generation || !this.connected {
                    return;
                }
                this.transcript_state.timer = None;
                this.subscribe_transcript(cx);
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub(super) fn receive_transcript(&mut self, id: u64, frame: &Value, cx: &mut Context<Self>) {
        if self.transcript_watch != Some(id) {
            return;
        }
        if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
            let update = match serde_json::from_value::<TranscriptUpdate>(value.clone()) {
                Ok(update) => update,
                Err(error) => {
                    self.recover_transcript(error.to_string(), cx);
                    return;
                }
            };
            let reset = matches!(&update.frame, zuse_model::TranscriptFrame::Reset { .. });
            if !self.transcript_state.ready && !reset {
                self.recover_transcript("The host omitted the initial history snapshot".into(), cx);
                return;
            }
            let follow = self.transcript_scroll.max_offset().y + self.transcript_scroll.offset().y
                <= px(24.);
            if let Err(error) = apply_transcript_frame(&mut self.messages, update.frame) {
                self.recover_transcript(error.to_string(), cx);
                return;
            }
            self.clear_closed_secret(cx);
            if reset {
                self.transcript_state.deadline = None;
                self.transcript_state.ready = true;
                self.transcript_state
                    .healthy_since
                    .get_or_insert_with(Instant::now);
            }
            self.transcript_state.notice = None;
            self.context_usage = update.context_usage;
            self.context_confirmed = true;
            if !self
                .context_usage
                .and_then(|usage| usage.window)
                .is_some_and(|window| window > 0)
            {
                self.context_menu = false;
            }
            if follow {
                self.transcript_scroll.scroll_to_bottom();
            }
            self.sync_previews(true, cx);
            self.confirm_deliveries();
        }
        if frame.get("err").is_some() || frame["done"] == true {
            self.recover_transcript(
                frame["err"]
                    .as_str()
                    .unwrap_or("The host closed the history stream")
                    .into(),
                cx,
            );
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::channel::mpsc;
    use std::prelude::v1::test;
    struct Fixture {
        window: WindowHandle<WorkspaceView>,
        _events: UnboundedSender<Value>,
        requests: UnboundedReceiver<Value>,
    }
    fn fixture(cx: &mut gpui::TestAppContext) -> Fixture {
        cx.update(initialize);
        let (requests, rx) = mpsc::unbounded();
        let (events, frames) = mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window
            .update(cx, |view, _, cx| {
                view.connected = true;
                view.workspace_synced = true;
                view.connection_epoch = 1;
                view.selected =
                    Some(json!({"id":"chat","deviceId":"host","config":{"harness":"codex"}}));
                view.begin_transcript(cx);
            })
            .unwrap();
        Fixture {
            window,
            _events: events,
            requests: rx,
        }
    }
    #[gpui::test]
    fn cached_history_cannot_dispatch_execution_even_with_a_connected_host(
        cx: &mut gpui::TestAppContext,
    ) {
        let mut f = fixture(cx);
        while f.requests.try_recv().is_ok() {}
        f.window
            .update(cx, |view, _, cx| {
                view.cached_only = true;
                view.connected = true;
                assert!(!view.require_connection(cx));
                assert_eq!(view.call(methods::MUTATE, json!({})), 0);
                assert_eq!(
                    view.call(methods::QUEUE_COMMAND, json!({"text":"do not execute"})),
                    0
                );
                assert_eq!(
                    view.call("CloudUnarchive", json!({"workspaceId":"other-workspace"})),
                    0
                );
            })
            .unwrap();
        assert!(f.requests.try_recv().is_err());
    }
    fn watch(f: &Fixture, cx: &mut gpui::TestAppContext) -> u64 {
        f.window
            .update(cx, |view, _, _| view.transcript_watch.unwrap())
            .unwrap()
    }
    fn tick(cx: &mut gpui::TestAppContext, millis: u64) {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(millis));
        cx.run_until_parked();
    }
    fn entry() -> Value {
        json!({"id":"outgoing","role":"user","createdAt":0,"deviceId":"host","parts":[{"id":"text","kind":"text","text":"confirmed"}]})
    }
    #[gpui::test]
    fn transcript_recovery_preserves_drafts_and_does_not_acknowledge_rejected_rows(
        cx: &mut gpui::TestAppContext,
    ) {
        let mut f = fixture(cx);
        let first = watch(&f, cx);
        f.window.update(cx, |view, _, cx| {
            view.receive(json!({"id":first,"item":{"reset":[],"contextUsage":{"tokens":12,"window":100}}}), cx);
            view.error = Some("Unrelated upload failure".into());
            view.input.update(cx, |input, cx| input.set_value("Keep my draft".into(), cx));
            view.uncertain_drafts.push(Draft { text: "confirmed".into(), attachments: vec![], key: "chat".into(), chat_id: Some("chat".into()), host_id: Some("host".into()), message_id: Some("outgoing".into()), not_sent: false });
            view.receive(json!({"id":first,"item":{"upsert":[{"after":null,"entry":entry()}],"count":99,"contextUsage":{"tokens":99,"window":100}}}), cx);
            assert!(view.messages.is_empty());
            assert_eq!(view.uncertain_drafts.len(), 1);
            assert_eq!(view.context_usage.unwrap().tokens, Some(12));
            assert!(view.transcript_blocked());
        }).unwrap();
        tick(cx, 200);
        let fresh = watch(&f, cx);
        assert_ne!(fresh, first);
        f.window.update(cx, |view, _, cx| {
            view.receive(json!({"id":first,"item":{"reset":[entry()]}}), cx);
            assert!(view.messages.is_empty());
            view.receive(json!({"id":fresh,"item":{"reset":[entry()],"contextUsage":{"tokens":2,"window":100}}}), cx);
            assert!(!view.transcript_blocked());
            assert!(view.transcript_state.notice.is_none());
            assert_eq!(view.uncertain_drafts.len(), 0);
            assert_eq!(view.error.as_deref(), Some("Unrelated upload failure"));
            assert_eq!(view.input.read(cx).value(cx), "Keep my draft");
        }).unwrap();
        while let Ok(request) = f.requests.try_recv() {
            assert!(!matches!(
                request["method"].as_str(),
                Some(methods::MUTATE | methods::QUEUE_COMMAND | methods::QUEUE_MESSAGE)
            ));
        }
    }
    #[gpui::test]
    fn transcript_recovery_is_bounded_and_manual_retry_requires_a_reset(
        cx: &mut gpui::TestAppContext,
    ) {
        let f = fixture(cx);
        for delay in [200, 600, 1800] {
            let id = watch(&f, cx);
            f.window
                .update(cx, |view, _, cx| {
                    view.receive(json!({"id":id,"item":{"count":0}}), cx)
                })
                .unwrap();
            tick(cx, delay);
        }
        let id = watch(&f, cx);
        f.window
            .update(cx, |view, _, cx| {
                view.receive(json!({"id":id,"item":{"reset":"invalid"}}), cx);
                assert!(view.transcript_watch.is_none());
                assert!(view.transcript_state.timer.is_none());
                assert!(
                    view.transcript_state
                        .notice
                        .as_ref()
                        .unwrap()
                        .contains("could not refresh")
                );
            })
            .unwrap();
        tick(cx, 10_000);
        f.window
            .update(cx, |view, _, cx| {
                assert!(view.transcript_watch.is_none());
                view.retry_transcript(cx);
                let id = view.transcript_watch.unwrap();
                view.receive(json!({"id":id,"item":{"reset":[]}}), cx);
                assert!(!view.transcript_blocked());
                assert!(view.transcript_state.notice.is_none());
            })
            .unwrap();
    }
    #[gpui::test]
    fn transcript_recovery_times_out_a_silent_initial_read_and_cancels_its_deadline(
        cx: &mut gpui::TestAppContext,
    ) {
        let f = fixture(cx);
        let old = watch(&f, cx);
        tick(cx, 15_000);
        f.window
            .update(cx, |view, _, _| {
                assert!(view.transcript_watch.is_none());
                assert!(view.transcript_blocked());
            })
            .unwrap();
        tick(cx, 200);
        let current = watch(&f, cx);
        assert_ne!(old, current);
        f.window
            .update(cx, |view, _, cx| {
                view.receive(json!({"id":old,"item":{"reset":[entry()]}}), cx);
                assert!(view.messages.is_empty());
                view.receive(json!({"id":current,"item":{"reset":[]}}), cx);
                assert!(!view.transcript_blocked());
                assert!(view.transcript_state.deadline.is_none());
            })
            .unwrap();
        tick(cx, 15_000);
        assert_eq!(watch(&f, cx), current);
    }

    #[gpui::test]
    fn transcript_recovery_cancels_old_timers_when_the_host_or_draft_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        let f = fixture(cx);
        let old = watch(&f, cx);
        f.window
            .update(cx, |view, _, cx| {
                view.receive(json!({"id":old,"done":true}), cx);
                view.selected.as_mut().unwrap()["deviceId"] = json!("new-host");
                view.begin_transcript(cx);
            })
            .unwrap();
        let current = watch(&f, cx);
        tick(cx, 250);
        assert_eq!(watch(&f, cx), current);
        f.window
            .update(cx, |view, _, cx| {
                view.receive(json!({"id":old,"item":{"reset":[entry()]}}), cx);
                assert!(view.messages.is_empty());
                view.receive(json!({"id":current,"err":"unavailable"}), cx);
                view.stop_transcript();
                view.selected = None;
            })
            .unwrap();
        tick(cx, 10_000);
        f.window
            .update(cx, |view, _, _| {
                assert!(view.transcript_watch.is_none());
                assert!(!view.transcript_blocked());
            })
            .unwrap();
    }
}
