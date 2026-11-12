//! A message refused because its computer's runtime is too old never becomes a
//! dead end. It stays in the chat while the computer updates:
//!
//! ```text
//! send ─▶ refused "updating" ─▶ Updating computer… (poll status every 1 s)
//!                                 ├─ committed / current ─▶ send it again
//!                                 └─ rolled-back / failed / unavailable / 3 min ─┐
//! send ─▶ refused "update unavailable" ─────────────────────────────────────────┤
//!                                                                               ▼
//!                                         Didn't send · [Update computer] [Discard]
//!   Update computer ─▶ recreate onto the latest base ─▶ Updating computer… ─▶ ready ─▶ send it again
//! ```
//!
//! "Send it again" is the same text, files and agent settings under a new
//! command id. Send is never disabled: a message written while the computer
//! updates joins the chat at once and waits with the others.
use super::*;
use crate::composer::tray::{TrayPill, TrayTone, tray_text_action};
use zuse_protocol::cloud_runtime_updates::{
    RUNTIME_UPDATE_UNAVAILABLE, RUNTIME_UPDATING, UpdateState,
};

const POLL: Duration = Duration::from_secs(1);
const UPDATE_LIMIT: Duration = Duration::from_secs(180);
const RECREATION_POLL: Duration = Duration::from_secs(2);
const RECREATION_LIMIT: Duration = Duration::from_secs(30 * 60);
pub(super) const UPDATING: &str = "Updating computer…";
const RESTART_NOTE: &str = "This restarts the computer; your files and chats are kept.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The computer is updating its runtime in place.
    Updating,
    /// It can't update in place; Update computer recreates it.
    Unavailable,
}
/// The runtime refusal a failed send carries, if any.
pub(super) fn refusal(code: &str) -> Option<Refusal> {
    match code.strip_prefix(zuse_protocol::cloud_task_commands::TASK_REJECTED_PREFIX)? {
        RUNTIME_UPDATING => Some(Refusal::Updating),
        RUNTIME_UPDATE_UNAVAILABLE => Some(Refusal::Unavailable),
        _ => None,
    }
}

/// A message waiting for its computer. Its echo stays in the chat.
#[derive(Clone)]
struct Held {
    request: Value,
    text: Option<String>,
    staged: Vec<StagedAttachment>,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::composer) enum NotSent {
    /// The in-place update rolled back, failed or took too long.
    UpdateFailed,
    /// The computer can't update in place.
    Unavailable,
    /// Update computer didn't finish.
    RecreationFailed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::composer) enum Phase {
    Updating {
        since: Instant,
    },
    Recreating {
        operation: Option<String>,
        since: Instant,
    },
    NotSent(NotSent),
}

#[derive(Default)]
pub(in crate::composer) struct State {
    held: Vec<Held>,
    /// Held messages being sent again, one after another.
    ready: std::collections::VecDeque<Held>,
    pub(in crate::composer) phase: Option<Phase>,
    work: Option<Task<()>>,
}

fn message_id(request: &Value) -> Option<&str> {
    request["action"]["messageId"].as_str()
}

/// The same message under a new identity. After recreation its files go to
/// the new computer again.
fn renewed(mut request: Value, recreated: bool, sleeping: bool) -> Value {
    let command = uuid::Uuid::new_v4().to_string();
    let message = uuid::Uuid::new_v4().to_string();
    let queue = request["action"]["type"] == "queue";
    request["commandId"] = json!(command);
    request["action"]["messageId"] = json!(message);
    if queue {
        request["action"]["queueId"] = json!(command);
    } else if request["action"]["payload"].is_object() {
        request["action"]["payload"]["messageId"] = json!(message);
    }
    request["wake"] = json!(wake::intent(queue, sleeping));
    if recreated {
        if queue {
            if let Some(input) = request["action"]["input"].as_object_mut() {
                input.remove("computerFiles");
            }
        } else if request["action"]["payload"]["request"].is_object() {
            request["action"]["payload"]["request"]["attachments"] = json!([]);
        }
    }
    request
}

impl Composer {
    /// The computer is updating or being recreated: new sends wait with the
    /// held messages instead of being refused again.
    pub(super) fn cloud_runtime_update_holds(&self) -> bool {
        matches!(
            self.cloud.runtime.phase,
            Some(Phase::Updating { .. } | Phase::Recreating { .. })
        )
    }
    fn take_pending_as_held(&mut self) -> Option<Held> {
        let request = self.cloud.pending_native.take()?;
        Some(Held {
            request,
            text: self.cloud.pending_native_text.take(),
            staged: std::mem::take(&mut self.cloud.native_staged),
        })
    }
    /// A Send while the computer updates: it is already in the chat; it
    /// waits and goes out with the others.
    pub(super) fn hold_cloud_send(&mut self, cx: &mut Context<Self>) {
        if let Some(held) = self.take_pending_as_held() {
            self.cloud.runtime.held.push(held);
        }
        cx.notify();
    }
    /// The server refused the pending message for the computer's runtime.
    pub(super) fn hold_cloud_refusal(&mut self, refusal: Refusal, cx: &mut Context<Self>) {
        let Some(held) = self.take_pending_as_held() else {
            return;
        };
        let runtime = &mut self.cloud.runtime;
        runtime.held.push(held);
        // Messages queued to go out after it wait again too.
        runtime.held.extend(runtime.ready.drain(..));
        self.failure = None;
        match refusal {
            Refusal::Updating => {
                if !matches!(
                    self.cloud.runtime.phase,
                    Some(Phase::Updating { .. } | Phase::Recreating { .. })
                ) {
                    self.watch_cloud_runtime_update(cx);
                }
            }
            Refusal::Unavailable => {
                if !matches!(self.cloud.runtime.phase, Some(Phase::Recreating { .. })) {
                    self.cloud.runtime.work = None;
                    self.cloud.runtime.phase = Some(Phase::NotSent(NotSent::Unavailable));
                }
            }
        }
        cx.notify();
    }
    fn not_sent(&mut self, reason: NotSent, cx: &mut Context<Self>) {
        self.cloud.runtime.work = None;
        self.cloud.runtime.phase = Some(Phase::NotSent(reason));
        cx.notify();
    }
    /// Poll the in-place update until it finishes, then send again.
    fn watch_cloud_runtime_update(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.cloud.native.as_ref() else {
            return;
        };
        let target = task.target.clone();
        let since = cx.background_executor().now();
        self.cloud.runtime.phase = Some(Phase::Updating { since });
        self.cloud.runtime.work = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let result = target
                    .controller
                    .client()
                    .call_scoped(
                        methods::CLOUD_RUNTIME_UPDATE_STATUS,
                        json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,
                            "expectedOrganizationId":target.organization}),
                    )
                    .await;
                let state = match &result {
                    Ok(value)
                        if value["accountId"] == target.subject
                            && value["organizationId"].as_str()
                                == target.organization.as_deref()
                            && value["workspaceId"] == target.workspace =>
                    {
                        serde_json::from_value::<UpdateState>(value["status"]["state"].clone()).ok()
                    }
                    // A status read failing is not the update failing; ask again.
                    _ => None,
                };
                let expired = cx.background_executor().now().duration_since(since) >= UPDATE_LIMIT;
                let done = this.update(cx, |this, cx| match state {
                    Some(UpdateState::Committed | UpdateState::Current) => {
                        this.send_held_cloud_messages(false, cx);
                        true
                    }
                    Some(UpdateState::RolledBack | UpdateState::Failed) => {
                        this.not_sent(NotSent::UpdateFailed, cx);
                        true
                    }
                    Some(UpdateState::Unavailable) => {
                        this.not_sent(NotSent::Unavailable, cx);
                        true
                    }
                    _ if expired => {
                        this.not_sent(NotSent::UpdateFailed, cx);
                        true
                    }
                    _ => false,
                });
                if done.unwrap_or(true) {
                    return;
                }
            }
        }));
    }
    /// Update computer: recreate it on the newest base, follow the
    /// recreation, then send the held messages again.
    pub(in crate::composer) fn update_cloud_computer(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.cloud.runtime.phase, Some(Phase::NotSent(_)))
            || self.cloud.runtime.held.is_empty()
        {
            return;
        }
        let Some(task) = self.cloud.native.as_ref() else {
            return;
        };
        let target = task.target.clone();
        let since = cx.background_executor().now();
        self.failure = None;
        self.cloud.runtime.phase = Some(Phase::Recreating {
            operation: None,
            since,
        });
        let scope = json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,
            "expectedOrganizationId":target.organization});
        let mut start = scope.clone();
        start["action"] = json!("start");
        start["idempotencyKey"] = json!(uuid::Uuid::new_v4().to_string());
        start["targetRelease"] = json!("latest");
        self.cloud.runtime.work = Some(cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .call_scoped(methods::CLOUD_COMPUTER_RECREATION, start)
                .await;
            let operation = match result {
                Ok(value)
                    if value["accountId"] == target.subject
                        && value["organizationId"].as_str() == target.organization.as_deref()
                        && value["workspaceId"] == target.workspace
                        && value["computerRecreation"]["operationId"].is_string() =>
                {
                    let recreation = value["computerRecreation"].clone();
                    let operation = recreation["operationId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    let _ = this.update(cx, |this, cx| {
                        if let Some(Phase::Recreating {
                            operation: current, ..
                        }) = &mut this.cloud.runtime.phase
                        {
                            *current = Some(operation.clone());
                        }
                        cx.emit(ComposerEvent::CloudComputerRecreating { recreation });
                        cx.notify();
                    });
                    operation
                }
                Ok(_) => {
                    let _ = this.update(cx, |this, cx| {
                        this.not_sent(
                            NotSent::RecreationFailed("the server didn't confirm it.".into()),
                            cx,
                        )
                    });
                    return;
                }
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.not_sent(NotSent::RecreationFailed(error.to_string()), cx)
                    });
                    return;
                }
            };
            let mut status = scope;
            status["action"] = json!("status");
            status["operationId"] = json!(operation);
            loop {
                cx.background_executor().timer(RECREATION_POLL).await;
                let result = target
                    .controller
                    .client()
                    .call_scoped(methods::CLOUD_COMPUTER_RECREATION, status.clone())
                    .await;
                let state = match &result {
                    Ok(value)
                        if value["accountId"] == target.subject
                            && value["workspaceId"] == target.workspace =>
                    {
                        value["computerRecreation"]["state"]
                            .as_str()
                            .map(str::to_owned)
                    }
                    _ => None,
                };
                let expired =
                    cx.background_executor().now().duration_since(since) >= RECREATION_LIMIT;
                let done = this.update(cx, |this, cx| match state.as_deref() {
                    Some("ready") => {
                        this.send_held_cloud_messages(true, cx);
                        true
                    }
                    Some("attention-required" | "cancelled") => {
                        this.not_sent(
                            NotSent::RecreationFailed("the computer needs attention.".into()),
                            cx,
                        );
                        true
                    }
                    _ if expired => {
                        this.not_sent(
                            NotSent::RecreationFailed("it is taking too long.".into()),
                            cx,
                        );
                        true
                    }
                    _ => false,
                });
                if done.unwrap_or(true) {
                    return;
                }
            }
        }));
        cx.notify();
    }
    /// The computer is ready: send every held message again, in order.
    fn send_held_cloud_messages(&mut self, recreated: bool, cx: &mut Context<Self>) {
        let runtime = &mut self.cloud.runtime;
        runtime.phase = None;
        runtime.work = None;
        let sleeping = self.cloud.native.as_ref().is_some_and(|task| task.wake);
        let held = std::mem::take(&mut runtime.held);
        let session = self
            .cloud
            .native
            .as_ref()
            .map(|task| task.target.initial_session.clone());
        for mut message in held {
            let old = message_id(&message.request).map(str::to_owned);
            message.request = renewed(message.request, recreated, sleeping);
            if let (Some(session), Some(old), Some(new)) =
                (session.as_deref(), old, message_id(&message.request))
            {
                let new = new.to_owned();
                self.state.update(cx, |state, cx| {
                    if state.rekey_echo(session, &old, &new) {
                        cx.notify();
                    }
                });
            }
            self.cloud.runtime.ready.push_back(message);
        }
        self.send_next_ready_cloud_message(cx);
        cx.notify();
    }
    /// After a resent message is admitted, the next one goes.
    pub(super) fn send_next_ready_cloud_message(&mut self, cx: &mut Context<Self>) {
        if self.cloud.task.is_some() || self.cloud.pending_native.is_some() {
            return;
        }
        let Some(next) = self.cloud.runtime.ready.pop_front() else {
            return;
        };
        self.cloud.pending_native = Some(next.request);
        self.cloud.pending_native_text = next.text;
        self.cloud.native_staged = next.staged;
        self.dispatch_cloud_task(cx);
    }
    /// A resent message failed for another reason and went back to the draft
    /// with its own error. The ones after it return to the draft too, so
    /// nothing is lost.
    pub(super) fn release_ready_cloud_messages(&mut self, cx: &mut Context<Self>) {
        let ready: Vec<Held> = self.cloud.runtime.ready.drain(..).collect();
        self.return_held_to_draft(ready, cx);
    }
    /// Discard: the held messages leave the chat; their text and files return
    /// to the draft so nothing typed is lost.
    pub(in crate::composer) fn discard_held_cloud_messages(&mut self, cx: &mut Context<Self>) {
        let runtime = &mut self.cloud.runtime;
        runtime.work = None;
        runtime.phase = None;
        let mut held = std::mem::take(&mut runtime.held);
        held.extend(runtime.ready.drain(..));
        self.failure = None;
        self.return_held_to_draft(held, cx);
        cx.notify();
    }
    fn return_held_to_draft(&mut self, held: Vec<Held>, cx: &mut Context<Self>) {
        if held.is_empty() {
            return;
        }
        if let Some(session) = self
            .cloud
            .native
            .as_ref()
            .map(|task| task.target.initial_session.clone())
        {
            let ids: Vec<String> = held
                .iter()
                .filter_map(|held| message_id(&held.request).map(str::to_owned))
                .collect();
            self.state.update(cx, |state, cx| {
                for id in &ids {
                    state.remove_echo(&session, id);
                }
                cx.notify();
            });
        }
        // Whatever is in the box stays first; nothing typed is replaced.
        let draft = self.input.read(cx).text().to_owned();
        let texts: Vec<String> = std::iter::once(draft)
            .chain(held.iter().filter_map(|held| held.text.clone()))
            .filter(|text| !text.trim().is_empty())
            .collect();
        self.input
            .update(cx, |input, cx| input.set_text(texts.join("\n\n"), cx));
        let files = self
            .attachments
            .entry(self.current_key.clone())
            .or_default();
        for file in held.into_iter().flat_map(|held| held.staged) {
            if !files.iter().any(|existing| existing.id == file.id) {
                files.push(file);
            }
        }
        self.sync_attachment_chips(cx);
    }
    pub(in crate::composer) fn cloud_runtime_update_row(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<TrayPill> {
        let phase = self.cloud.runtime.phase.as_ref()?;
        if self.cloud.runtime.held.is_empty() {
            return None;
        }
        let spinner = |cx: &mut Context<Self>| -> gpui::AnyElement {
            crate::loaders::mini_mono_spinner(
                "cloud-runtime-update-spinner",
                2.,
                theme.text_muted,
                cx.entity_id(),
                cx,
            )
            .into_any_element()
        };
        Some(match phase {
            Phase::Updating { .. } => TrayPill::new("cloud-runtime-update", UPDATING)
                .subtitle(if self.cloud.runtime.held.len() == 1 {
                    "Your message sends as soon as it's done."
                } else {
                    "Your messages send as soon as it's done."
                })
                .icon_element(spinner(cx)),
            Phase::Recreating { .. } => TrayPill::new("cloud-runtime-update", UPDATING)
                .subtitle(RESTART_NOTE)
                .icon_element(spinner(cx)),
            Phase::NotSent(reason) => {
                let title = match self.cloud.runtime.held.len() {
                    1 => "Didn't send".to_owned(),
                    count => format!("{count} messages didn't send"),
                };
                let subtitle = match reason {
                    NotSent::UpdateFailed => format!(
                        "The computer couldn't update itself. Update computer moves it to the latest version. {RESTART_NOTE}"
                    ),
                    NotSent::Unavailable => {
                        format!("This computer needs an update first. {RESTART_NOTE}")
                    }
                    NotSent::RecreationFailed(error) => {
                        format!("The update didn't finish: {error} Your message is still here.")
                    }
                };
                TrayPill::new("cloud-runtime-update", title)
                    .subtitle(subtitle)
                    .tone(TrayTone::Warning)
                    .glyph(crate::icons::ALERT_CIRCLE)
                    .action(
                        tray_text_action("update-cloud-computer", "Update computer", None, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.update_cloud_computer(cx))),
                    )
                    .action(
                        tray_text_action("discard-cloud-held", "Discard", None, theme).on_click(
                            cx.listener(|this, _, _, cx| this.discard_held_cloud_messages(cx)),
                        ),
                    )
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_runtime_codes_are_runtime_refusals() {
        let prefix = zuse_protocol::cloud_task_commands::TASK_REJECTED_PREFIX;
        assert_eq!(
            refusal(&format!("{prefix}{RUNTIME_UPDATING}")),
            Some(Refusal::Updating)
        );
        assert_eq!(
            refusal(&format!("{prefix}{RUNTIME_UPDATE_UNAVAILABLE}")),
            Some(Refusal::Unavailable)
        );
        assert_eq!(refusal(RUNTIME_UPDATING), None);
        assert_eq!(refusal(&format!("{prefix}Pick another model.")), None);
    }

    #[test]
    fn a_resent_message_keeps_its_content_under_a_new_identity() {
        let original = json!({"commandId":"c1","sessionId":"s","wake":"explicit-resume",
            "action":{"type":"submit","messageId":"m1","payload":{"kind":"run","messageId":"m1",
            "request":{"prompt":"Fix it","attachments":["/home/zuse/uploads/a.png"],"model":"m"}}}});
        let same = renewed(original.clone(), false, false);
        assert_ne!(same["commandId"], "c1");
        assert_ne!(same["action"]["messageId"], "m1");
        assert_eq!(
            same["action"]["messageId"],
            same["action"]["payload"]["messageId"]
        );
        assert_eq!(
            same["action"]["payload"]["request"],
            original["action"]["payload"]["request"]
        );
        let recreated = renewed(original, true, false);
        assert_eq!(
            recreated["action"]["payload"]["request"]["attachments"],
            json!([])
        );
        assert_eq!(
            recreated["action"]["payload"]["request"]["prompt"],
            "Fix it"
        );
        let queued = renewed(
            json!({"commandId":"q1","action":{"type":"queue","queueId":"q1","messageId":"m","input":{"text":"x","computerFiles":["/f"]}}}),
            true,
            true,
        );
        assert_eq!(queued["action"]["queueId"], queued["commandId"]);
        assert!(queued["action"]["input"].get("computerFiles").is_none());
        assert_eq!(queued["wake"], "preserve-sleep");
    }

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
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async { tokio::task::yield_now().await });
        }
        cx.run_until_parked();
    }
    fn wait(cx: &mut gpui::TestAppContext, runtime: &tokio::runtime::Runtime, by: Duration) {
        cx.executor().advance_clock(by);
        settle(cx, runtime);
    }
    fn refuse(
        runtime: &tokio::runtime::Runtime,
        replies: &tokio::sync::mpsc::Sender<String>,
        request: &Value,
        code: &str,
    ) {
        let prefix = zuse_protocol::cloud_task_commands::TASK_REJECTED_PREFIX;
        reply(
            runtime,
            replies,
            json!({"id":request["id"],"err":format!("capability error: {prefix}{code}")}),
        );
    }
    fn admit(
        runtime: &tokio::runtime::Runtime,
        replies: &tokio::sync::mpsc::Sender<String>,
        request: &Value,
    ) {
        reply(
            runtime,
            replies,
            json!({"id":request["id"],"ok":{"accountId":"owner","organizationId":null,
                "data":{"commandId":request["params"]["request"]["commandId"],"durablyAdmitted":true}}}),
        );
    }
    fn echoed(state: &Entity<AppState>, cx: &mut gpui::TestAppContext) -> Vec<(String, String)> {
        state.read_with(cx, |state, _| {
            state
                .pending_echoes()
                .iter()
                .filter_map(|entry| match &entry.parts[..] {
                    [zuse_model::MessagePart::Text { text, .. }] => {
                        Some((entry.id.clone(), text.clone()))
                    }
                    _ => None,
                })
                .collect()
        })
    }
    /// Send `text`, then return the request the server sees.
    fn send(
        composer: &Entity<Composer>,
        cx: &mut gpui::TestAppContext,
        runtime: &tokio::runtime::Runtime,
        requests: &mut tokio::sync::mpsc::Receiver<String>,
        text: &str,
    ) -> Value {
        composer.update(cx, |c, cx| {
            c.input.update(cx, |input, cx| input.set_text(text, cx));
            assert!(c.send_cloud_task(text.into(), false, cx));
        });
        settle(cx, runtime);
        let request = next(requests).expect("the message is sent");
        assert_eq!(request["method"], methods::CLOUD_TASK_COMMAND);
        request
    }
    fn status(state: &str) -> Value {
        json!({"accountId":"owner","organizationId":null,"workspaceId":"task",
            "status":{"updateId":"update","state":state,"errorCode":null}})
    }

    #[gpui::test]
    fn an_updating_computer_keeps_the_message_then_sends_it_again(cx: &mut gpui::TestAppContext) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, state, mut requests, replies) = composer_with_requests(cx);
        let first = send(&composer, cx, &runtime, &mut requests, "Fix the build");
        refuse(&runtime, &replies, &first, RUNTIME_UPDATING);
        settle(cx, &runtime);
        composer.update(cx, |c, cx| {
            assert!(matches!(
                c.cloud.runtime.phase,
                Some(Phase::Updating { .. })
            ));
            assert_eq!(c.input.read(cx).text(), "", "it stays out of the draft");
            assert!(c.failure.is_none(), "no error while it updates");
            assert!(c.cloud.pending_native.is_none());
        });
        let original = first["params"]["request"]["action"]["messageId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            echoed(&state, cx),
            vec![(original.clone(), "Fix the build".to_owned())],
            "the message stays in the chat"
        );
        // Send still works: the new message joins the chat and waits.
        composer.update(cx, |c, cx| {
            c.input
                .update(cx, |input, cx| input.set_text("Then run the tests", cx));
            assert!(c.send_cloud_task("Then run the tests".into(), false, cx));
            assert_eq!(c.input.read(cx).text(), "");
        });
        settle(cx, &runtime);
        assert!(next(&mut requests).is_none(), "it waits for the update");
        assert_eq!(echoed(&state, cx).len(), 2);

        wait(cx, &runtime, POLL);
        let poll = next(&mut requests).expect("the update is polled");
        assert_eq!(poll["method"], methods::CLOUD_RUNTIME_UPDATE_STATUS);
        assert_eq!(poll["params"]["workspaceId"], "task");
        assert_eq!(poll["params"]["expectedAccountId"], "owner");
        reply(
            &runtime,
            &replies,
            json!({"id":poll["id"],"ok":status("swapping")}),
        );
        settle(cx, &runtime);
        assert!(next(&mut requests).is_none(), "nothing resends mid-swap");

        wait(cx, &runtime, POLL);
        let poll = next(&mut requests).expect("polled again a second later");
        reply(
            &runtime,
            &replies,
            json!({"id":poll["id"],"ok":status("committed")}),
        );
        settle(cx, &runtime);
        let resend = next(&mut requests).expect("the same message is sent again");
        let request = &resend["params"]["request"];
        assert_eq!(
            request["action"]["payload"]["request"]["prompt"],
            "Fix the build"
        );
        assert_eq!(
            request["action"]["payload"]["request"]["model"],
            first["params"]["request"]["action"]["payload"]["request"]["model"]
        );
        assert_ne!(
            request["commandId"],
            first["params"]["request"]["commandId"]
        );
        let renewed_id = request["action"]["messageId"].as_str().unwrap().to_owned();
        assert_ne!(renewed_id, original);
        let echoes = echoed(&state, cx);
        assert_eq!(
            echoes[0],
            (renewed_id, "Fix the build".to_owned()),
            "same spot in the chat"
        );
        admit(&runtime, &replies, &resend);
        settle(cx, &runtime);
        let second = next(&mut requests).expect("the waiting message goes next");
        assert_eq!(
            second["params"]["request"]["action"]["payload"]["request"]["prompt"],
            "Then run the tests"
        );
        admit(&runtime, &replies, &second);
        settle(cx, &runtime);
        composer.update(cx, |c, _| {
            assert!(c.cloud.runtime.phase.is_none());
            assert!(c.cloud.runtime.held.is_empty() && c.cloud.runtime.ready.is_empty());
            assert!(c.cloud.pending_native.is_none());
        });
    }

    #[gpui::test]
    fn update_computer_recreates_onto_the_latest_base_then_sends(cx: &mut gpui::TestAppContext) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, state, mut requests, replies) = composer_with_requests(cx);
        let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let _subscription = cx.update(|cx| {
            let events = events.clone();
            cx.subscribe(&composer, move |_, event: &ComposerEvent, _| {
                if let ComposerEvent::CloudComputerRecreating { recreation } = event {
                    events.borrow_mut().push(recreation.clone());
                }
            })
        });
        let first = send(&composer, cx, &runtime, &mut requests, "Fix the build");
        refuse(&runtime, &replies, &first, RUNTIME_UPDATE_UNAVAILABLE);
        settle(cx, &runtime);
        composer.update(cx, |c, cx| {
            assert_eq!(
                c.cloud.runtime.phase,
                Some(Phase::NotSent(NotSent::Unavailable))
            );
            assert_eq!(c.input.read(cx).text(), "");
            c.update_cloud_computer(cx);
            assert!(matches!(
                c.cloud.runtime.phase,
                Some(Phase::Recreating {
                    operation: None,
                    ..
                })
            ));
        });
        assert_eq!(echoed(&state, cx).len(), 1, "the message stays in the chat");
        settle(cx, &runtime);
        let start = next(&mut requests).expect("recreation is requested");
        assert_eq!(start["method"], methods::CLOUD_COMPUTER_RECREATION);
        assert_eq!(start["params"]["action"], "start");
        assert_eq!(start["params"]["targetRelease"], "latest");
        assert_eq!(start["params"]["workspaceId"], "task");
        assert!(uuid::Uuid::parse_str(start["params"]["idempotencyKey"].as_str().unwrap()).is_ok());
        reply(
            &runtime,
            &replies,
            json!({"id":start["id"],"ok":{"accountId":"owner","organizationId":null,"workspaceId":"task",
                "computerRecreation":{"operationId":"op","state":"quiescing"}}}),
        );
        settle(cx, &runtime);
        assert_eq!(
            events.borrow().len(),
            1,
            "the conversation follows the recreation"
        );
        composer.update(cx, |c, _| {
            assert!(matches!(
                &c.cloud.runtime.phase,
                Some(Phase::Recreating { operation: Some(op), .. }) if op == "op"
            ));
        });
        wait(cx, &runtime, RECREATION_POLL);
        let check = next(&mut requests).expect("recreation progress is followed");
        assert_eq!(check["params"]["action"], "status");
        assert_eq!(check["params"]["operationId"], "op");
        reply(
            &runtime,
            &replies,
            json!({"id":check["id"],"ok":{"accountId":"owner","organizationId":null,"workspaceId":"task",
                "computerRecreation":{"operationId":"op","state":"restoring"}}}),
        );
        settle(cx, &runtime);
        assert!(next(&mut requests).is_none());
        wait(cx, &runtime, RECREATION_POLL);
        let check = next(&mut requests).expect("followed until ready");
        reply(
            &runtime,
            &replies,
            json!({"id":check["id"],"ok":{"accountId":"owner","organizationId":null,"workspaceId":"task",
                "computerRecreation":{"operationId":"op","state":"ready"}}}),
        );
        settle(cx, &runtime);
        let resend = next(&mut requests).expect("the message is sent again");
        assert_eq!(resend["method"], methods::CLOUD_TASK_COMMAND);
        assert_eq!(
            resend["params"]["request"]["action"]["payload"]["request"]["prompt"],
            "Fix the build"
        );
        assert_ne!(
            resend["params"]["request"]["commandId"],
            first["params"]["request"]["commandId"]
        );
        admit(&runtime, &replies, &resend);
        settle(cx, &runtime);
        composer.update(cx, |c, _| assert!(c.cloud.runtime.phase.is_none()));
    }

    #[gpui::test]
    fn a_failed_update_shows_the_message_as_not_sent_with_update_and_discard(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, state, mut requests, replies) = composer_with_requests(cx);
        let first = send(&composer, cx, &runtime, &mut requests, "Fix the build");
        refuse(&runtime, &replies, &first, RUNTIME_UPDATING);
        settle(cx, &runtime);
        wait(cx, &runtime, POLL);
        let poll = next(&mut requests).expect("polled");
        reply(
            &runtime,
            &replies,
            json!({"id":poll["id"],"ok":status("rolled-back")}),
        );
        settle(cx, &runtime);
        composer.update(cx, |c, cx| {
            assert_eq!(
                c.cloud.runtime.phase,
                Some(Phase::NotSent(NotSent::UpdateFailed))
            );
            assert_eq!(c.input.read(cx).text(), "");
        });
        assert_eq!(
            echoed(&state, cx).len(),
            1,
            "shown as not sent, still in the chat"
        );
        wait(cx, &runtime, POLL * 5);
        assert!(next(&mut requests).is_none(), "polling stopped");
        composer.update(cx, |c, cx| {
            c.discard_held_cloud_messages(cx);
            assert!(c.cloud.runtime.phase.is_none());
            assert_eq!(c.input.read(cx).text(), "Fix the build", "its text returns");
        });
        assert!(echoed(&state, cx).is_empty(), "it leaves the chat");
        settle(cx, &runtime);
        assert!(next(&mut requests).is_none(), "Discard never sends");
    }

    #[gpui::test]
    fn an_update_that_takes_too_long_stops_waiting(cx: &mut gpui::TestAppContext) {
        let runtime = runtime();
        let _entered = runtime.enter();
        let (composer, _state, mut requests, replies) = composer_with_requests(cx);
        let first = send(&composer, cx, &runtime, &mut requests, "Fix the build");
        refuse(&runtime, &replies, &first, RUNTIME_UPDATING);
        settle(cx, &runtime);
        wait(cx, &runtime, POLL);
        let poll = next(&mut requests).expect("polled");
        // A failed status read is retried, not treated as a failed update.
        reply(
            &runtime,
            &replies,
            json!({"id":poll["id"],"err":"cloud unavailable"}),
        );
        settle(cx, &runtime);
        composer.update(cx, |c, _| {
            assert!(matches!(
                c.cloud.runtime.phase,
                Some(Phase::Updating { .. })
            ))
        });
        wait(cx, &runtime, UPDATE_LIMIT);
        let poll = next(&mut requests).expect("polled again");
        reply(
            &runtime,
            &replies,
            json!({"id":poll["id"],"ok":status("downloading")}),
        );
        settle(cx, &runtime);
        composer.update(cx, |c, _| {
            assert_eq!(
                c.cloud.runtime.phase,
                Some(Phase::NotSent(NotSent::UpdateFailed))
            );
        });
    }
}
