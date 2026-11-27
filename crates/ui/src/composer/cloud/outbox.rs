//! A cloud message the server hasn't taken yet never reads as a starting turn.
//!
//! ```text
//! send ─▶ Sending message… ─▶ accepted ─▶ the agent's turn (server record)
//!            │
//!            └─ server unreachable ─▶ Waiting for connection (retry 1 s, 2 s … 15 s,
//!                  │                   and at once when the server answers again)
//!                  ├─ accepted ─▶ the next waiting message goes, in order
//!                  └─ still unreachable after OFFLINE_LIMIT ─▶ Not sent · Retry
//! ```
//!
//! Every attempt carries the message's original command and message ids, so
//! the server takes it at most once however often it is sent again. A message
//! written while an earlier one is on its way joins the chat at once and waits
//! behind it in `later`.
use super::*;
use std::collections::VecDeque;

/// How long a message keeps being retried before it shows as not sent.
pub(super) const OFFLINE_LIMIT: Duration = Duration::from_secs(120);
/// An attempt the server hasn't answered by then counts as unreachable.
pub(super) const ATTEMPT_LIMIT: Duration = Duration::from_secs(60);
/// The longest wait between automatic attempts.
const MAX_BACKOFF: Duration = Duration::from_secs(15);
pub(super) const WAITING: &str = "Waiting for connection";

#[derive(Default)]
pub(in crate::composer) struct State {
    /// Messages sent while an earlier one is still on its way, oldest first.
    /// Each is already in the chat (or the queue).
    pub(super) later: VecDeque<runtime_update::Held>,
    /// When the message being delivered first failed to reach the server.
    since: Option<Instant>,
    attempts: u32,
    /// The next automatic attempt.
    retry: Option<Task<()>>,
    /// Retried for [`OFFLINE_LIMIT`] without reaching the server: the
    /// message shows as not sent with Retry.
    given_up: bool,
}

impl State {
    /// The message reached the server (or was answered for good), or the
    /// user sent it again: its delivery clock starts afresh.
    pub(super) fn settle(&mut self) {
        self.since = None;
        self.attempts = 0;
        self.retry = None;
        self.given_up = false;
    }
}

/// Whether a failed send never reached the server, so the same request may
/// be sent again as is.
pub(super) fn unreachable(error: &zuse_rpc::RpcError) -> bool {
    match error {
        zuse_rpc::RpcError::Transport(_) | zuse_rpc::RpcError::Closed => true,
        zuse_rpc::RpcError::Capability(code) => {
            code == zuse_protocol::cloud_task_commands::TASK_UNREACHABLE
        }
        _ => false,
    }
}

/// The wait before automatic attempt `attempt` (1-based): 1 s, 2 s, 4 s, 8 s,
/// then 15 s.
pub(super) fn backoff(attempt: u32) -> Duration {
    Duration::from_secs(1u64 << attempt.saturating_sub(1).min(4)).min(MAX_BACKOFF)
}

impl Composer {
    /// The message being sent couldn't reach the server: it waits for the
    /// connection and goes again on its own, until [`OFFLINE_LIMIT`].
    pub(super) fn cloud_send_unreachable(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let outbox = &mut self.cloud.outbox;
        let since = *outbox.since.get_or_insert(now);
        if now.duration_since(since) >= OFFLINE_LIMIT {
            outbox.retry = None;
            outbox.given_up = true;
            // Shown inline under the messages; no extra error in the tray.
            self.keep_unsent_native_send(cx);
            return;
        }
        outbox.attempts += 1;
        let wait = backoff(outbox.attempts);
        outbox.retry = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |this, cx| this.retry_cloud_delivery(cx));
        }));
        cx.notify();
    }
    /// Send the waiting message again, as is.
    fn retry_cloud_delivery(&mut self, cx: &mut Context<Self>) {
        self.cloud.outbox.retry = None;
        if self.cloud_waiting_for_connection() && self.cloud.task.is_none() {
            self.dispatch_cloud_task(cx);
        }
    }
    /// The server answered again (a history read went through): a message
    /// waiting for the connection goes now instead of at its next attempt.
    pub(crate) fn cloud_connection_restored(&mut self, cx: &mut Context<Self>) {
        if self.cloud_waiting_for_connection() && self.cloud.task.is_none() {
            self.retry_cloud_delivery(cx);
        }
    }
    /// A message is waiting for the connection and is retried on its own.
    pub(crate) fn cloud_waiting_for_connection(&self) -> bool {
        self.cloud.outbox.since.is_some()
            && !self.cloud.outbox.given_up
            && self.cloud.pending_native.is_some()
    }
    /// A message waiting for the connection (or, `given_up`, shown not
    /// sent), for a conversation's tests.
    #[cfg(test)]
    pub(crate) fn hold_cloud_send_offline_for_test(&mut self, given_up: bool) {
        self.cloud.pending_native = Some(json!({"commandId":"offline","sessionId":"session",
            "action":{"type":"submit","messageId":"offline-message","payload":{"kind":"run"}}}));
        self.cloud.pending_native_text = Some("you are stuck?".into());
        self.cloud.outbox.since = Some(Instant::now());
        self.cloud.outbox.given_up = given_up;
        self.cloud.unsent = given_up;
    }
    /// Messages in the chat the server never took: shown as not sent, with
    /// Retry, instead of any turn progress.
    pub(crate) fn cloud_messages_not_sent(&self) -> bool {
        self.cloud.outbox.given_up
            && self
                .cloud
                .pending_native
                .as_ref()
                .is_some_and(|request| request["action"]["messageId"].is_string())
    }
    /// Retry from the not-sent line: the first waiting message goes again
    /// under its same identity; the rest follow it in order.
    pub(crate) fn retry_cloud_messages_not_sent(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.cloud.pending_native.clone() else {
            return;
        };
        if request["action"]["type"] == "queue" {
            if let Some(id) = request["commandId"].as_str() {
                self.retry_unsent_cloud_queued(id, cx);
            }
        } else {
            self.retry_unsent_cloud_send(cx);
        }
    }
    /// The message being sent is settled: the next waiting one goes.
    pub(super) fn send_next_waiting_cloud_message(&mut self, cx: &mut Context<Self>) {
        if self.cloud.task.is_some() || self.cloud.pending_native.is_some() {
            return;
        }
        let Some(next) = self.cloud.outbox.later.pop_front() else {
            return;
        };
        self.cloud.pending_native = Some(next.request);
        self.cloud.pending_native_text = next.text;
        self.cloud.native_staged = next.staged;
        self.dispatch_cloud_task(cx);
    }
    /// The waiting messages, oldest first, leave the outbox (to go back to
    /// the draft with a refused one).
    pub(super) fn take_waiting_cloud_messages(&mut self) -> Vec<runtime_update::Held> {
        self.cloud.outbox.later.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_back_off_to_a_ceiling() {
        let waits: Vec<u64> = (1..=7).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 15, 15, 15]);
    }

    #[test]
    fn only_a_send_that_never_reached_the_server_is_retried_as_is() {
        use zuse_rpc::RpcError;
        assert!(unreachable(&RpcError::Transport(
            "Cloud service is unavailable. Try again.".into()
        )));
        assert!(unreachable(&RpcError::Closed));
        assert!(unreachable(&RpcError::Capability(
            zuse_protocol::cloud_task_commands::TASK_UNREACHABLE.into()
        )));
        assert!(!unreachable(&RpcError::Failed("refused".into())));
        assert!(!unreachable(&RpcError::Capability(
            "cloud_task_rejected:model".into()
        )));
    }

    /// A server that is unreachable until `online`, then admits each command
    /// once: a replay of an admitted command id returns the same admission.
    #[derive(Default)]
    struct Server {
        online: std::sync::atomic::AtomicBool,
        /// Every command id an attempt carried, in order.
        attempts: std::sync::Mutex<Vec<String>>,
        /// Command ids admitted, in order, each once.
        admitted: std::sync::Mutex<Vec<String>>,
    }
    struct Service(std::sync::Arc<Server>);
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Service {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            if method == "ClaimComputerPrimary" {
                return Ok(zuse_rpc::RpcReply::Value(json!({"deviceId":null})));
            }
            assert_eq!(method, methods::CLOUD_TASK_COMMAND);
            let id = params["request"]["commandId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            self.0.attempts.lock().unwrap().push(id.clone());
            if !self.0.online.load(std::sync::atomic::Ordering::SeqCst) {
                // As the engine reports an unreachable cloud service.
                return Err(zuse_rpc::RpcError::Capability(
                    zuse_protocol::cloud_task_commands::TASK_UNREACHABLE.into(),
                ));
            }
            let mut admitted = self.0.admitted.lock().unwrap();
            if !admitted.contains(&id) {
                admitted.push(id.clone());
            }
            Ok(zuse_rpc::RpcReply::Value(
                json!({"accountId":"owner","organizationId":null,
                "data":{"commandId":id,"durablyAdmitted":true}}),
            ))
        }
    }

    struct Fixture {
        runtime: tokio::runtime::Runtime,
        server: std::sync::Arc<Server>,
        state: Entity<crate::state::AppState>,
        composer: Entity<Composer>,
    }
    impl Fixture {
        fn new(cx: &mut gpui::TestAppContext) -> Self {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let server = std::sync::Arc::new(Server::default());
            let controller = {
                let _entered = runtime.enter();
                crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
                    std::sync::Arc::new(Service(server.clone())),
                ))
            };
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
                    json!({"agent":"zuse","model":"test-model","runtimeMode":"full-access","queueRunSelectionVersion":1}),
                    cx,
                );
            });
            Self {
                runtime,
                server,
                state,
                composer,
            }
        }
        /// Let every pending reply and task finish.
        fn settle(&self, cx: &mut gpui::TestAppContext) {
            let _entered = self.runtime.enter();
            for _ in 0..12 {
                cx.run_until_parked();
                self.runtime.block_on(async {
                    tokio::task::yield_now().await;
                });
            }
            cx.run_until_parked();
        }
        fn send(&self, text: &str, cx: &mut gpui::TestAppContext) {
            let _entered = self.runtime.enter();
            self.composer.update(cx, |c, cx| {
                c.input.update(cx, |input, cx| input.set_text(text, cx));
                assert!(c.send_cloud_task(text.into(), false, cx));
                assert_eq!(c.input.read(cx).text(), "", "it leaves the draft");
            });
        }
        fn online(&self, online: bool) {
            self.server
                .online
                .store(online, std::sync::atomic::Ordering::SeqCst);
        }
        /// The chat's messages not yet in the server's transcript, in order.
        fn shown(&self, cx: &mut gpui::TestAppContext) -> Vec<String> {
            self.state.read_with(cx, |state, _| {
                state
                    .pending_echoes()
                    .iter()
                    .filter_map(|entry| match &entry.parts[..] {
                        [zuse_model::MessagePart::Text { text, .. }] => Some(text.clone()),
                        _ => None,
                    })
                    .collect()
            })
        }
        fn activity(&self, cx: &mut gpui::TestAppContext) -> Option<(String, bool)> {
            self.composer.read_with(cx, |c, _| {
                c.cloud_activity()
                    .map(|(label, busy)| (label.to_owned(), busy))
            })
        }
        fn admitted(&self) -> Vec<String> {
            self.server.admitted.lock().unwrap().clone()
        }
    }

    #[gpui::test]
    fn offline_sends_wait_for_the_connection_then_go_once_in_order(cx: &mut gpui::TestAppContext) {
        let f = Fixture::new(cx);
        f.send("what was the issue?", cx);
        let first = f.composer.read_with(cx, |c, _| {
            c.cloud.pending_native.as_ref().unwrap()["commandId"]
                .as_str()
                .unwrap()
                .to_owned()
        });
        f.settle(cx);
        // Not with the server: a quiet waiting line, no error, no turn.
        assert_eq!(f.activity(cx), Some((WAITING.to_owned(), true)));
        f.composer.read_with(cx, |c, _| {
            assert!(c.failure.is_none(), "no error while it retries");
            assert!(!c.cloud_messages_not_sent());
            assert!(!c.cloud_send_unsent());
        });
        // Retried on its own, under the same id.
        cx.executor().advance_clock(Duration::from_secs(1));
        f.settle(cx);
        // Two more messages join the chat at once and wait behind it.
        f.send("what?", cx);
        f.send("you are stuck?", cx);
        assert_eq!(
            f.shown(cx),
            ["what was the issue?", "what?", "you are stuck?"]
        );
        assert!(f.admitted().is_empty());
        assert!(
            f.server
                .attempts
                .lock()
                .unwrap()
                .iter()
                .all(|id| *id == first),
            "only the first message is tried while it can't get through"
        );
        // The connection returns: they go, each once, in the order sent.
        f.online(true);
        {
            let _entered = f.runtime.enter();
            f.composer
                .update(cx, |c, cx| c.cloud_connection_restored(cx));
        }
        f.settle(cx);
        cx.executor().advance_clock(Duration::from_secs(20));
        f.settle(cx);
        let admitted = f.admitted();
        assert_eq!(admitted.len(), 3, "all three admitted: {admitted:?}");
        assert_eq!(admitted[0], first);
        let attempts = f.server.attempts.lock().unwrap().clone();
        let online_attempts: Vec<_> = attempts.iter().skip_while(|id| **id == first).collect();
        assert_eq!(
            online_attempts,
            [&admitted[1], &admitted[2]],
            "never sent twice once taken"
        );
        f.composer.read_with(cx, |c, _| {
            assert!(c.cloud.pending_native.is_none());
            assert!(c.cloud.outbox.later.is_empty());
            assert!(!c.cloud_waiting_for_connection());
        });
        // Accepted: the normal path takes over until the agent answers.
        assert_eq!(
            f.activity(cx),
            Some(("Waiting for agent…".to_owned(), true))
        );
        assert_eq!(
            f.shown(cx),
            ["what was the issue?", "what?", "you are stuck?"]
        );
    }

    #[gpui::test]
    fn a_long_outage_shows_not_sent_and_retry_sends_it_once(cx: &mut gpui::TestAppContext) {
        let f = Fixture::new(cx);
        f.send("what was the issue?", cx);
        f.send("what?", cx);
        f.settle(cx);
        // Retried quietly until the limit, then shown as not sent.
        for _ in 0..12 {
            cx.executor().advance_clock(MAX_BACKOFF);
            f.settle(cx);
        }
        f.composer.read_with(cx, |c, _| {
            assert!(c.cloud_messages_not_sent());
            assert!(!c.cloud_waiting_for_connection());
            assert!(c.cloud_activity().is_none(), "no turn and no waiting line");
            assert!(c.failure.is_none(), "shown inline, not as a tray error");
        });
        assert_eq!(f.shown(cx), ["what was the issue?", "what?"]);
        // No more automatic attempts once it shows as not sent.
        let tried = f.server.attempts.lock().unwrap().len();
        cx.executor().advance_clock(Duration::from_secs(60));
        f.settle(cx);
        assert_eq!(f.server.attempts.lock().unwrap().len(), tried);
        // Retry: the same message, shown once, sent once; the next follows.
        f.online(true);
        {
            let _entered = f.runtime.enter();
            f.composer
                .update(cx, |c, cx| c.retry_cloud_messages_not_sent(cx));
        }
        f.settle(cx);
        cx.executor().advance_clock(Duration::from_secs(1));
        f.settle(cx);
        assert_eq!(f.shown(cx), ["what was the issue?", "what?"]);
        let admitted = f.admitted();
        assert_eq!(admitted.len(), 2, "{admitted:?}");
        f.composer.read_with(cx, |c, _| {
            assert!(!c.cloud_messages_not_sent());
            assert!(c.cloud.pending_native.is_none());
        });
        // A second Retry has nothing left to send.
        {
            let _entered = f.runtime.enter();
            f.composer
                .update(cx, |c, cx| c.retry_cloud_messages_not_sent(cx));
        }
        f.settle(cx);
        assert_eq!(f.admitted(), admitted);
    }
}
