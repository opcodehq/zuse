//! Native cloud launch uses the existing attached tray and retains the accepted request on failure.
use super::*;
use crate::localization::message as tr;
use serde_json::{Value, json};
use zuse_i18n::messages as msg;
mod launch_upload;
mod outbox;
mod reupload;
mod runtime_update;
mod send_upload;
mod unknown;
mod waiting;
mod wake;
/// Shown when a message is sent before the server has recorded its new
/// conversation; cleared once it has.
const STILL_OPENING: &str =
    "This chat is still opening on its computer. Your draft is preserved; send again in a moment.";
#[derive(Default)]
pub(super) struct Launch {
    pub task: Option<Task<()>>,
    pub message: Option<String>,
    cached: Option<bool>,
    owner: Option<String>,
    organization: Option<String>,
    pub(super) request: Option<Value>,
    accepted: Option<Value>,
    issue: Option<StagedAttachment>,
    pub(super) staged: Vec<StagedAttachment>,
    pub pause_task: Option<Task<()>>,
    pub pause_error: Option<String>,
    native: Option<NativeTask>,
    pending_native: Option<Value>,
    pending_native_text: Option<String>,
    // Retain receipt identity through the gap before execution/history arrives.
    admitted_native: Option<Value>,
    native_staged: Vec<StagedAttachment>,
    pub(super) rejected_drafts: Vec<(String, Vec<StagedAttachment>)>,
    /// The retained message is in the chat but wasn't sent: the tray offers
    /// Retry and Edit.
    unsent: bool,
    pause_command: Option<String>,
    /// An earlier message whose delivery outcome is unknown.
    pub(super) unknown: unknown::State,
    /// Messages waiting while their computer updates its runtime.
    pub(super) runtime: runtime_update::State,
    /// Messages the server hasn't taken yet: retried while it can't be
    /// reached, then shown as not sent (see [`outbox`]).
    pub(super) outbox: outbox::State,
    /// Admitted messages' files, kept until their computer takes them.
    pub(super) reupload: reupload::State,
    /// The cloud computer's stage, set by the conversation that owns this
    /// composer and shown in the tray's single status line.
    status: Option<(String, bool)>,
    /// The conversation's computer needs attention: its status line offers
    /// Retry and Delete.
    status_actions: bool,
    /// The conversation that owns this composer has its agent mid-turn (see
    /// `CloudConversation::turn_running`): the composer offers Stop and
    /// queues a message instead of starting a new turn.
    turn_running: bool,
    /// Why a parked launch failed, shown when its tab is in front again.
    pub(super) failure: Option<SharedString>,
    /// When the new chat's first message was sent.
    started_at: Option<std::time::Instant>,
    /// The new chat's first-message files uploading while its computer
    /// starts (see [`launch_upload`]).
    upload: Option<launch_upload::Progress>,
    /// A waiting message's files being copied to server storage.
    waiting_copy: Option<waiting::WaitingCopy>,
    /// The files of the message being sent, while they upload (see
    /// [`send_upload`]).
    send_upload: Option<send_upload::Upload>,
    /// A conversation just added beside the computer's others is still being
    /// recorded by the server (a few tens of milliseconds, no computer work):
    /// a send waits for it, so it never reaches the server unregistered.
    registering: bool,
}
impl Launch {
    pub(super) fn admitted(&self) -> bool {
        self.accepted.is_some()
    }
    /// A new chat is being created (or failed to be and waits for Retry or
    /// Delete).
    pub(super) fn pending(&self) -> bool {
        self.request.is_some() || self.task.is_some()
    }
    /// Move the new-chat launch out, leaving the rest (a bound cloud chat's
    /// state) in place.
    pub(super) fn take_launch(&mut self) -> Launch {
        Launch {
            task: self.task.take(),
            message: self.message.take(),
            request: self.request.take(),
            accepted: self.accepted.take(),
            issue: self.issue.take(),
            staged: std::mem::take(&mut self.staged),
            failure: self.failure.take(),
            started_at: self.started_at.take(),
            upload: self.upload.take(),
            owner: self.owner.clone(),
            organization: self.organization.clone(),
            ..Default::default()
        }
    }
    pub(super) fn put_launch(&mut self, launch: Launch) {
        self.task = launch.task;
        self.message = launch.message;
        self.request = launch.request;
        self.accepted = launch.accepted;
        self.issue = launch.issue;
        self.staged = launch.staged;
        self.failure = launch.failure;
        self.started_at = launch.started_at;
        self.upload = launch.upload;
    }
}
/// A new cloud chat being created in some tab: what its sidebar row and tab
/// show before the server confirms it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CloudLaunchSummary {
    pub key: String,
    pub project_id: String,
    pub first_message: String,
    pub harness: Option<HarnessId>,
    pub workspace: Option<String>,
    pub failed: bool,
    pub started_at: Option<std::time::Instant>,
}
/// A computer stage as the short status line shows it.
fn short_stage(message: &str) -> &str {
    match message {
        "Starting computer" | "Starting computer…" => "Starting computer…",
        "Preparing cloud computer…" => "Preparing computer…",
        other => other,
    }
}
/// Tries to archive a computer whose new chat was deleted or removed.
const ARCHIVE_ATTEMPTS: u32 = 5;
/// How long the server has to confirm a new cloud chat before its tab shows
/// it as failed with Retry and Delete.
pub(crate) const CLOUD_CREATE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(90);
pub(crate) const CLOUD_CREATE_TIMEOUT: &str =
    "Cloud didn't confirm this chat in time. Retry checks the same request.";
/// How long an accepted computer has to become ready before its tab shows a
/// failed state instead of waiting forever.
pub(crate) const CLOUD_READY_DEADLINE: std::time::Duration =
    std::time::Duration::from_secs(20 * 60);
pub(crate) const CLOUD_READY_TIMEOUT: &str =
    "The computer didn't start in time. Retry to check it again.";
struct NativeTask {
    target: crate::state::CloudTarget,
    wake: bool,
    queue_selection_version: u32,
}
impl Composer {
    pub(crate) fn set_cloud_task(
        &mut self,
        target: crate::state::CloudTarget,
        config: Value,
        cx: &mut Context<Self>,
    ) {
        let wake = self.cloud.native.as_ref().is_some_and(|previous| {
            previous.target.subject == target.subject
                && previous.target.organization == target.organization
                && previous.target.workspace == target.workspace
                && previous.target.initial_session == target.initial_session
                && previous.wake
        });
        let seed = serde_json::from_value::<HarnessId>(config["agent"].clone())
            .ok()
            .map(|harness| zuse_protocol::ChatConfig {
                harness,
                plan: false,
                model: config["model"].as_str().map(str::to_owned),
                reasoning: serde_json::from_value(config["reasoning"].clone())
                    .ok()
                    .flatten(),
                model_options: serde_json::from_value(
                    config
                        .get("modelOptions")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                )
                .unwrap_or_default(),
                sandbox: serde_json::from_value(config["sandbox"].clone()).unwrap_or_else(|_| {
                    match config["runtimeMode"].as_str() {
                        Some("full-access") => SandboxLevel::DangerFullAccess,
                        Some("auto") => SandboxLevel::Auto,
                        _ => SandboxLevel::WorkspaceWrite,
                    }
                }),
            });
        self.pickers.update(cx, |pickers, cx| {
            pickers.bind_cloud_task(
                target.clone(),
                config["providerId"].as_str().unwrap_or("boxd").to_owned(),
                seed,
                cx,
            )
        });
        self.cloud.native = Some(NativeTask {
            target,
            wake,
            queue_selection_version: config["queueRunSelectionVersion"].as_u64().unwrap_or(0)
                as u32,
        });
        self.cloud.cached = None;
        cx.notify();
    }
    pub(crate) fn clear_cloud_task(&mut self, cx: &mut Context<Self>) {
        self.cloud.task = None;
        self.cloud.unknown = Default::default();
        self.cloud.runtime = Default::default();
        self.cloud.reupload = Default::default();
        self.cloud.outbox = Default::default();
        self.cloud.native = None;
        self.cloud.pending_native = None;
        self.cloud.pending_native_text = None;
        self.cloud.admitted_native = None;
        self.cloud.turn_running = false;
        self.cloud.native_staged.clear();
        self.pickers
            .update(cx, |pickers, cx| pickers.clear_cloud_task_binding(cx));
        self.input.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }
    /// Whether this composer belongs to a cloud chat, whose files go
    /// straight to its computer.
    pub(crate) fn is_native_cloud(&self) -> bool {
        self.cloud.native.is_some()
    }
    /// The cloud chat this composer is bound to, if any.
    pub(super) fn native_cloud_target(&self) -> Option<&crate::state::CloudTarget> {
        self.cloud.native.as_ref().map(|task| &task.target)
    }
    /// Hold sends while the server records this new conversation.
    pub(crate) fn set_cloud_registering(&mut self, registering: bool, cx: &mut Context<Self>) {
        self.cloud.registering = registering;
        if !registering {
            // Its notices (held send, retried attempt) are resolved now.
            self.failure = None;
        }
        cx.notify();
    }
    pub(crate) fn set_cloud_task_sleeping(&mut self, sleeping: bool) {
        if let Some(task) = &mut self.cloud.native {
            task.wake = sleeping;
        }
    }
    pub(super) fn send_cloud_task(
        &mut self,
        text: String,
        queue: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.send_cloud_task_as(text, queue, false, cx)
    }
    /// Send the message still being delivered again, under its same identity.
    fn retry_cloud_task(&mut self, text: String, queue: bool, cx: &mut Context<Self>) -> bool {
        self.send_cloud_task_inner(text, queue, false, true, cx)
    }

    /// Send, queue or steer a cloud chat's message, as a chat on this device
    /// does: while the agent runs, a steer goes into the running turn (when
    /// its agent reads messages mid-turn) and anything else is queued. Every
    /// message shows at once: a sent or steered one in the chat, a queued one
    /// in the queue.
    pub(super) fn send_cloud_task_as(
        &mut self,
        text: String,
        queue: bool,
        steer: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        self.send_cloud_task_inner(text, queue, steer, false, cx)
    }
    fn send_cloud_task_inner(
        &mut self,
        text: String,
        queue: bool,
        steer: bool,
        retry: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.cloud.native.is_none() {
            return false;
        }
        // A new message written while an earlier one is still on its way
        // waits behind it, in order (see [`outbox`]); a Retry of the earlier
        // one goes on under its own identity.
        let earlier = self.cloud.pending_native.is_some() || self.cloud.task.is_some();
        let retry = retry && self.cloud.pending_native.is_some();
        if self.cloud.task.is_some() && retry {
            return true;
        }
        if self.cloud.registering {
            // A shown reason (a failed attempt being retried) stays visible.
            if self.failure.is_none() {
                self.failure = Some(STILL_OPENING.into());
            }
            cx.notify();
            return true;
        }
        if self.cloud_unknown_blocks_send(cx) {
            return true;
        }
        let Some(task) = &self.cloud.native else {
            return false;
        };
        if queue
            && task.queue_selection_version
                != zuse_protocol::cloud_task_commands::QUEUE_RUN_SELECTION_VERSION
        {
            self.failure=Some("This server cannot save the selected agent with queued messages yet. Your draft is preserved; wait for this turn to finish, then Send.".into());
            cx.notify();
            return true;
        }
        if !self.staged_appshots().is_empty() {
            self.failure = Some("Attach the capture as a file for this cloud conversation. Your draft is preserved.".into());
            cx.notify();
            return true;
        }
        if self.staged().len() > zuse_protocol::cloud_assets::MAX_ASSETS {
            self.failure = Some(
                "Attach at most eight files to one cloud message. Your draft is preserved.".into(),
            );
            cx.notify();
            return true;
        }
        if earlier && !retry {
            if let Some(request) = self.new_cloud_request(&text, queue, steer, cx) {
                let staged = self.staged().to_vec();
                self.show_new_cloud_request(&request, &text, &staged, cx);
                self.cloud.outbox.later.push_back(runtime_update::Held {
                    request,
                    text: Some(text),
                    staged,
                });
                cx.notify();
            }
            return true;
        }
        if self.cloud.pending_native.is_none() {
            let Some(request) = self.new_cloud_request(&text, queue, steer, cx) else {
                return true;
            };
            let staged = self.staged().to_vec();
            self.cloud.pending_native = Some(request.clone());
            self.cloud.pending_native_text = Some(text.clone());
            self.cloud.native_staged = staged.clone();
            self.show_new_cloud_request(&request, &text, &staged, cx);
        } else if let Some(request) = self.cloud.pending_native.clone()
            && request["action"]["type"] == "submit"
            && let (Some(message), Some(text)) = (
                request["action"]["messageId"].as_str(),
                self.cloud.pending_native_text.clone(),
            )
        {
            // Retry: the same message identity rejoins the conversation now.
            let session = task.target.initial_session.clone();
            self.show_native_send(&session, message, &text, cx);
        } else if let Some(request) = self
            .cloud
            .pending_native
            .clone()
            .filter(|request| request["action"]["type"] == "queue")
        {
            // Retry of a queued message: its row is sending again.
            if let Some(id) = request["commandId"].as_str() {
                let session = task.target.initial_session.clone();
                self.state.update(cx, |state, cx| {
                    state.set_optimistic_queued(
                        &session,
                        id,
                        crate::state::OptimisticState::Sending,
                    );
                    cx.notify();
                });
            }
            self.clear_sent_draft(cx);
        }
        // Sent or retried by the user: delivery starts afresh.
        self.cloud.outbox.settle();
        if self.cloud_runtime_update_holds() {
            // The computer is updating: the message is in the chat and goes
            // out with the others once it's done.
            self.hold_cloud_send(cx);
            return true;
        }
        self.dispatch_cloud_task(cx);
        true
    }
    /// A new message's request under fresh identities, with the task's saved
    /// agent settings; `None` (and the reason shown, the draft kept) when it
    /// can't be built yet.
    fn new_cloud_request(
        &mut self,
        text: &str,
        queue: bool,
        steer: bool,
        cx: &mut Context<Self>,
    ) -> Option<Value> {
        let task = self.cloud.native.as_ref()?;
        let session = task.target.initial_session.clone();
        let sleeping = task.wake;
        let command = uuid::Uuid::new_v4().to_string();
        let message = uuid::Uuid::new_v4().to_string();
        let Some(resolved) = self.pickers.read(cx).cloud_task_choice() else {
            self.failure = Some(
                "This task's saved agent settings are not available yet. Your draft is preserved."
                    .into(),
            );
            cx.notify();
            return None;
        };
        let harness = resolved.harness;
        let model = resolved.model.clone();
        if harness.is_none() || model.is_none() {
            self.failure = Some(
                "Choose an available agent and model for this task. Your draft is preserved."
                    .into(),
            );
            cx.notify();
            return None;
        }
        let sandbox = resolved.sandbox;
        let kind = cloud_send_kind(
            queue,
            steer,
            !self.staged().is_empty(),
            self.pickers.read(cx).steers_mid_turn(cx),
        );
        let steer = kind == CloudSendKind::Steer;
        let queue = kind == CloudSendKind::Queue;
        let action = if steer {
            json!({"type":"submit","messageId":message,"payload":{"kind":"steer","prompt":text,"messageId":message}})
        } else if queue {
            json!({"type":"queue","queueId":command,"messageId":message,"input":{"text":text,"runSelection":{"harness":harness,"model":model,"reasoning":resolved.reasoning,"modelOptions":resolved.model_options,"sandbox":sandbox}},"holdForTurnEnd":true})
        } else {
            json!({"type":"submit","messageId":message,"payload":{"kind":"run","messageId":message,"request":{
                "prompt":text,"harness":harness,"model":model,"reasoning":resolved.reasoning,
                "modelOptions":resolved.model_options,"cwd":".","sandbox":sandbox,
                "autoApprove":sandbox==SandboxLevel::DangerFullAccess,"resume":null,"attachments":[]
            }}})
        };
        Some(
            json!({"commandId":command,"sessionId":session,"action":action,
            "wake":wake::intent(queue || steer, sleeping)}),
        )
    }
    /// Show a new message where it belongs at once: a sent or steered one in
    /// the chat, a queued one in the queue (the server's row replaces it,
    /// same queue id). Its text and files leave the draft.
    fn show_new_cloud_request(
        &mut self,
        request: &Value,
        text: &str,
        staged: &[StagedAttachment],
        cx: &mut Context<Self>,
    ) {
        let (Some(session), Some(command), Some(message)) = (
            request["sessionId"].as_str().map(str::to_owned),
            request["commandId"].as_str().map(str::to_owned),
            request["action"]["messageId"].as_str().map(str::to_owned),
        ) else {
            return;
        };
        if request["action"]["type"] == "queue" {
            let files = staged
                .iter()
                .map(|file| format!("pending/{}/{}", file.id, file.name))
                .collect();
            self.show_optimistic_queued(&session, &command, &message, text, files, cx);
        } else {
            self.push_cloud_echo(&session, &message, text, cx);
        }
        self.clear_draft_of(staged, cx);
    }
    /// The message joins the conversation the moment it is sent; uploading
    /// its files and server admission continue behind it. A failure puts it
    /// back in the draft (see [`Self::return_native_send`]).
    fn show_native_send(
        &mut self,
        session: &str,
        message: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        self.push_cloud_echo(session, message, text, cx);
        self.clear_sent_draft(cx);
    }
    /// Put a queued message in the queue before the server confirms it.
    fn show_optimistic_queued(
        &mut self,
        session: &str,
        queue_id: &str,
        message: &str,
        text: &str,
        attachments: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let row = crate::state::OptimisticQueued {
            item: zuse_doc::QueuedMessage {
                id: queue_id.to_owned(),
                text: text.to_owned(),
                attachments,
                hold_for_turn_end: true,
                issued_by: "cloud-authority".into(),
                issued_at: chrono::Utc::now().timestamp_millis(),
                edited_at: None,
                delivery_gate: None,
            },
            message_id: message.to_owned(),
            state: crate::state::OptimisticState::Sending,
        };
        let session = session.to_owned();
        self.state.update(cx, |state, cx| {
            state.push_optimistic_queued(&session, row);
            cx.notify();
        });
    }

    /// Retry a queued cloud message the server didn't take: the same message
    /// while its identity is still retained, otherwise a new one with its
    /// text.
    pub(crate) fn retry_unsent_cloud_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(session) = self
            .cloud
            .native
            .as_ref()
            .map(|task| task.target.initial_session.clone())
        else {
            return;
        };
        let retained = self
            .cloud
            .pending_native
            .as_ref()
            .is_some_and(|request| request["commandId"] == id);
        if retained {
            let text = self.cloud.pending_native_text.clone().unwrap_or_default();
            self.keeping_draft(cx, |this, cx| {
                this.retry_cloud_task(text, true, cx);
            });
            return;
        }
        let Some(text) = self
            .state
            .read(cx)
            .optimistic_queued(id)
            .map(|row| row.item.text.clone())
        else {
            return;
        };
        if self.cloud.pending_native.is_some() || self.cloud.task.is_some() {
            self.failure = Some("Wait for the message being sent, then retry this one.".into());
            cx.notify();
            return;
        }
        self.state.update(cx, |state, cx| {
            state.drop_optimistic_queued(&session, id);
            cx.notify();
        });
        self.keeping_draft(cx, |this, cx| {
            this.send_cloud_task(text, true, cx);
        });
    }

    /// Remove a queued cloud message the server didn't take; its text goes
    /// back to an empty draft.
    pub(crate) fn discard_unsent_cloud_queued(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(session) = self
            .cloud
            .native
            .as_ref()
            .map(|task| task.target.initial_session.clone())
        else {
            return;
        };
        let text = self
            .state
            .read(cx)
            .optimistic_queued(id)
            .map(|row| row.item.text.clone());
        if self
            .cloud
            .pending_native
            .as_ref()
            .is_some_and(|request| request["commandId"] == id)
        {
            self.cloud.pending_native = None;
            self.cloud.pending_native_text = None;
            let restored = std::mem::take(&mut self.cloud.native_staged);
            let files = self
                .attachments
                .entry(self.current_key.clone())
                .or_default();
            for file in restored {
                if !files.iter().any(|existing| existing.id == file.id) {
                    files.push(file);
                }
            }
            self.sync_attachment_chips(cx);
        }
        if let Some(text) = text
            && self.input.read(cx).text().is_empty()
        {
            self.input.update(cx, |input, cx| input.set_text(text, cx));
        }
        self.failure = None;
        self.state.update(cx, |state, cx| {
            state.drop_optimistic_queued(&session, id);
            cx.notify();
        });
        if self.cloud.pending_native.is_none() {
            // Messages written after it no longer wait for it.
            self.cloud.outbox.settle();
            self.send_next_waiting_cloud_message(cx);
        }
        cx.notify();
    }

    /// Resume a cloud chat's queue that Stop paused. False for other chats.
    pub(crate) fn resume_cloud_queue(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(task) = &self.cloud.native else {
            return false;
        };
        let target = task.target.clone();
        let session = target.initial_session.clone();
        let request = json!({"commandId":uuid::Uuid::new_v4().to_string(),"sessionId":session,
            "action":{"type":"pause-queue","paused":false}});
        self.state.update(cx, |state, cx| {
            state.set_queue_paused(&session, false);
            cx.notify();
        });
        cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .call_scoped(
                    methods::CLOUD_TASK_COMMAND,
                    json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,
                    "expectedOrganizationId":target.organization,"request":request}),
                )
                .await;
            let resumed = result.is_ok_and(|value| value["data"]["durablyAdmitted"] == true);
            let _ = this.update(cx, |this, cx| {
                if !resumed {
                    this.state.update(cx, |state, cx| {
                        state.set_queue_paused(&session, true);
                        cx.notify();
                    });
                    this.failure = Some("Couldn't resume the queue. Try again.".into());
                }
                cx.notify();
            });
        })
        .detach();
        true
    }

    /// Put a sent message in the conversation before the server confirms it.
    fn push_cloud_echo(
        &mut self,
        session: &str,
        message: &str,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let message = message.to_owned();
        let text = text.to_owned();
        let session = session.to_owned();
        self.state.update(cx, |state, cx| {
            let device = state
                .selected_chat_row()
                .map(|chat| chat.device_id.clone())
                .unwrap_or_default();
            state.push_echo(
                &session,
                zuse_model::SessionMessageEntry {
                    id: message.clone(),
                    role: zuse_model::MessageRole::User,
                    parts: vec![zuse_model::MessagePart::Text {
                        id: format!("text:{message}"),
                        text,
                    }],
                    created_at: chrono::Utc::now().timestamp_millis(),
                    device_id: device,
                    status: None,
                    continuation_of: None,
                    duration_ms: None,
                },
            );
            cx.notify();
        });
    }
    /// The sent text and files leave the draft; delivery continues behind it.
    fn clear_sent_draft(&mut self, cx: &mut Context<Self>) {
        let sent = self.cloud.native_staged.clone();
        self.clear_draft_of(&sent, cx);
    }
    /// The draft's text and the given files leave the composer.
    fn clear_draft_of(&mut self, sent: &[StagedAttachment], cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.set_text("", cx));
        if let Some(files) = self.attachments.get_mut(&self.current_key) {
            files.retain(|file| !sent.iter().any(|sent| sent.id == file.id));
        }
        self.sync_attachment_chips(cx);
        cx.notify();
    }
    /// A send that may still go through on retry (no connection, an upload
    /// or admission that wasn't confirmed): the message stays where it was
    /// shown, marked not sent. A queued row offers Retry and Remove; a sent
    /// message keeps its place in the chat while the tray offers Retry and
    /// Edit. Retry re-sends the same message identity.
    fn keep_unsent_native_send(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.cloud.pending_native.clone() else {
            return;
        };
        let Some(task) = self.cloud.native.as_ref() else {
            return;
        };
        let session = task.target.initial_session.clone();
        match request["action"]["type"].as_str() {
            Some("queue") => {
                if let Some(id) = request["commandId"].as_str() {
                    self.state.update(cx, |state, cx| {
                        state.set_optimistic_queued(
                            &session,
                            id,
                            crate::state::OptimisticState::Failed("not sent".into()),
                        );
                        cx.notify();
                    });
                }
            }
            Some("submit") if request["action"]["messageId"].is_string() => {
                self.cloud.unsent = true;
            }
            _ => {}
        }
        cx.notify();
    }

    /// Whether a sent message is in the chat but wasn't sent (see
    /// [`Self::keep_unsent_native_send`]).
    pub(super) fn cloud_send_unsent(&self) -> bool {
        self.cloud.unsent
            && self
                .cloud
                .pending_native
                .as_ref()
                .is_some_and(|request| request["action"]["type"] == "submit")
    }

    /// Retry the unsent message in the chat under its same identity.
    pub(super) fn retry_unsent_cloud_send(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_send_unsent() {
            return;
        }
        let text = self.cloud.pending_native_text.clone().unwrap_or_default();
        self.keeping_draft(cx, |this, cx| {
            this.retry_cloud_task(text, false, cx);
        });
    }

    /// Run a retry without losing what is being typed meanwhile.
    fn keeping_draft(
        &mut self,
        cx: &mut Context<Self>,
        retry: impl FnOnce(&mut Self, &mut Context<Self>),
    ) {
        let draft = self.input.read(cx).text().to_owned();
        retry(self, cx);
        if !draft.is_empty() && self.input.read(cx).text().is_empty() {
            self.input.update(cx, |input, cx| input.set_text(draft, cx));
        }
    }

    /// Take the unsent message out of the chat and back into the draft.
    pub(super) fn edit_unsent_cloud_send(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_send_unsent() {
            return;
        }
        self.return_native_send(cx);
        self.cloud.pending_native = None;
        self.cloud.pending_native_text = None;
        self.cloud.native_staged.clear();
        self.cloud.outbox.settle();
        self.failure = None;
        self.failure_key = None;
        // Messages written after it no longer wait for it.
        self.send_next_waiting_cloud_message(cx);
        cx.notify();
    }

    /// Undo [`Self::show_native_send`] (or a queued message leaving the
    /// draft) after the server refused the message: it leaves the
    /// conversation or queue and its text and files return to an empty
    /// draft, to be changed and sent again.
    fn return_native_send(&mut self, cx: &mut Context<Self>) {
        self.cloud.unsent = false;
        let Some(request) = self.cloud.pending_native.clone() else {
            return;
        };
        let (Some(task), Some(message)) = (
            self.cloud.native.as_ref(),
            request["action"]["messageId"].as_str(),
        ) else {
            return;
        };
        let kind = request["action"]["type"].as_str();
        let session = task.target.initial_session.clone();
        if kind == Some("submit") {
            let message = message.to_owned();
            self.state
                .update(cx, |state, _| state.remove_echo(&session, &message));
        } else if kind == Some("queue") {
            if let Some(id) = request["commandId"].as_str() {
                self.state
                    .update(cx, |state, _| state.drop_optimistic_queued(&session, id));
            }
        } else {
            return;
        }
        if let Some(text) = self.cloud.pending_native_text.clone() {
            if self.input.read(cx).text().is_empty() {
                self.input.update(cx, |input, cx| input.set_text(text, cx));
            } else {
                // A newer draft is in the box: keep it, let it be sent, and
                // keep the unsent message's text visible instead of retrying it.
                self.cloud.pending_native = None;
                self.cloud.pending_native_text = None;
                self.failure_key = None;
                self.failure = Some(format!("This message wasn't sent: “{text}”").into());
            }
        }
        let restored = self.cloud.native_staged.clone();
        let files = self
            .attachments
            .entry(self.current_key.clone())
            .or_default();
        for file in restored {
            if !files.iter().any(|existing| existing.id == file.id) {
                files.push(file);
            }
        }
        self.sync_attachment_chips(cx);
    }
    pub(super) fn interrupt_cloud_task(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(task) = &self.cloud.native else {
            return false;
        };
        if self.cloud.task.is_some() || self.cloud.pending_native.is_some() {
            return true;
        }
        self.cloud.pending_native = Some(
            json!({"commandId":uuid::Uuid::new_v4().to_string(),"sessionId":task.target.initial_session,
            "action":{"type":"submit","messageId":uuid::Uuid::new_v4().to_string(),"payload":{"kind":"interrupt"}}}),
        );
        self.dispatch_cloud_task(cx);
        true
    }
    fn dispatch_cloud_task(&mut self, cx: &mut Context<Self>) {
        let Some(task) = &self.cloud.native else {
            return;
        };
        let target = task.target.clone();
        let Some(mut request) = self.cloud.pending_native.clone() else {
            return;
        };
        let staged = self.cloud.native_staged.clone();
        let primary = self.primary_claim(&request, cx);
        self.failure = None;
        self.cloud.task = Some(cx.spawn(async move |this, cx| {
            if !staged.is_empty() && !send_upload::delivered(&request, staged.len()) {
                // Straight to its computer while it is connected; server
                // storage when it isn't, or when that fails or stalls. The
                // message goes to the server once its files are available.
                match send_upload::deliver(&this, cx, &target, &mut request, &staged).await {
                    Ok(()) => {}
                    Err(send_upload::Failed::Gone) => return,
                    Err(send_upload::Failed::Unavailable(error)) => {
                        tracing::warn!(%error, "a message's files couldn't be uploaded");
                        // Retried with the message, under its same ids, until
                        // the outbox shows it as not sent.
                        let _ = this.update(cx, |this, cx| {
                            this.cloud.task = None;
                            this.cloud_send_unreachable(cx);
                        });
                        return;
                    }
                }
                // Retain where the files went so a retry doesn't upload again.
                let _ = this.update(cx, |this, cx| {
                    this.cloud.pending_native = Some(request.clone());
                    this.finish_send_upload();
                    cx.notify();
                });
            }
            // A message that has to wait for its computer (runtime update,
            // Update computer) is queued on the server, not refused.
            let client = target.controller.client();
            let call = client.call_scoped(methods::CLOUD_TASK_COMMAND,
                json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,
                    "expectedOrganizationId":target.organization,"request":request,
                    (zuse_protocol::cloud_runtime_updates::QUEUE_WHILE_WAITING):true}));
            // An attempt nobody answers counts as not reaching the server; the
            // retry carries the same ids, so the server takes it at most once.
            let deadline = cx.background_executor().timer(outbox::ATTEMPT_LIMIT);
            let result = match futures::future::select(Box::pin(call), deadline).await {
                futures::future::Either::Left((result, _)) => result,
                futures::future::Either::Right(_) => Err(zuse_rpc::RpcError::Transport(
                    "The cloud service didn't answer in time.".into(),
                )),
            };
            // Only an admitted message moves the chat's default computer to
            // the one it was sent from. The claim names that message, so the
            // server keeps whichever message was admitted last; it never
            // holds up or changes the send.
            let admitted = matches!(&result, Ok(value) if value["accountId"]==target.subject
                && value["organizationId"].as_str()==target.organization.as_deref()
                && value["data"]["commandId"]==request["commandId"]
                && value["data"]["durablyAdmitted"]==true);
            if admitted && let Some((claim, chat)) = primary {
                let command = request["commandId"].as_str().map(str::to_owned);
                let executor = cx.background_executor().clone();
                cx.spawn(async move |_| {
                    super::computers::claim_primary(claim, chat, command, executor).await
                })
                .detach();
            }
            let _ = this.update(cx, |this, cx| {
                this.cloud.task = None;
                if !matches!(&result, Err(error) if outbox::unreachable(error)) {
                    // The server answered: this message's delivery is settled.
                    this.cloud.outbox.settle();
                }
                match result {
                    Ok(value) if value["accountId"]==target.subject && value["organizationId"].as_str()==target.organization.as_deref()
                        && value["data"]["commandId"]==request["commandId"] && value["data"]["durablyAdmitted"]==true => {
                        // Saved in the server's queue until its computer can
                        // take it; the queue shows it from here on.
                        let waiting=value["data"]["waitingFor"].is_string();
                        let foreground=!waiting && request["action"]["type"]=="submit" && request["action"]["payload"]["kind"]=="run";
                        if foreground {
                            this.cloud.admitted_native=Some(json!({"commandId":request["commandId"],"messageId":request["action"]["messageId"]}));
                            this.remember_cloud_run(&request);
                        }
                        this.cloud.pending_native = None;
                        this.cloud.unsent = false;
                        if request["action"]["type"]=="queue" && let Some(id)=request["commandId"].as_str() {
                            // Saved: the row stays until the server's queue lists it.
                            let session=target.initial_session.clone();let revision=value["data"]["revision"].as_i64().unwrap_or(1);
                            this.state.update(cx,|state,cx|{
                                state.cloud_queue_revisions.insert((session.clone(),id.to_owned()),revision);
                                state.set_optimistic_queued(&session,id,crate::state::OptimisticState::Saved(chrono::Utc::now().timestamp_millis()));
                                cx.notify();
                            });
                        }
                        let sent=std::mem::take(&mut this.cloud.native_staged);
                        if let Some(files)=this.attachments.get_mut(&this.current_key){files.retain(|file|!sent.iter().any(|sent|sent.id==file.id));}
                        this.sync_attachment_chips(cx);
                        // Its files stay here until the computer takes it, in
                        // case they go missing there first.
                        let text=this.cloud.pending_native_text.clone();
                        if waiting && !send_upload::stored_on_server(&request) {
                            // Its files go to server storage until it is sent:
                            // the computer may be replaced meanwhile.
                            this.keep_waiting_files_on_server(&request, &value["data"], sent.clone(), cx);
                            // Its bubble stays until its queued row replaces it.
                            this.cloud.pending_native_text = None;
                        }
                        this.keep_cloud_files(&request,text,sent);
                        if let Some(text)=this.cloud.pending_native_text.take() {
                            // A sent or queued message already cleared its draft; anything there now is newer.
                            if request["action"]["type"]=="submit" {
                                if let Some(message)=request["action"]["messageId"].as_str() {
                                    let message=message.to_owned();let session=target.initial_session.clone();
                                    this.state.update(cx,|state,cx| {
                                        state.push_echo(&session,zuse_model::SessionMessageEntry {
                                            id:message.clone(),role:zuse_model::MessageRole::User,
                                            parts:vec![zuse_model::MessagePart::Text{id:format!("text:{message}"),text}],
                                            created_at:chrono::Utc::now().timestamp_millis(),device_id:state.selected_chat_row().map(|chat|chat.device_id.clone()).unwrap_or_default(),
                                            status:None,continuation_of:None,duration_ms:None,
                                        });cx.notify();
                                    });
                                    cx.emit(ComposerEvent::Sent{chat_id:session,message_id:message});
                                }
                            }
                        }
                        if foreground {cx.emit(ComposerEvent::CloudForegroundAdmitted);}
                        cx.emit(ComposerEvent::CloudLaunchChanged);
                        // Messages held for a computer update go out one by one,
                        // then those written while this one was on its way.
                        this.send_next_ready_cloud_message(cx);
                        this.send_next_waiting_cloud_message(cx);
                    }
                    Err(zuse_rpc::RpcError::Capability(code)) if runtime_update::refusal(&code).is_some() => {
                        // The computer's runtime must update first. The message
                        // stays in the chat and is sent again once it has.
                        if let Some(refusal) = runtime_update::refusal(&code) { this.hold_cloud_refusal(refusal, cx); }
                    }
                    Ok(_) => {
                        this.keep_unsent_native_send(cx);
                        this.failure = Some("Message admission was not verified. Retry the original message to check its saved identity.".into());
                    }
                    Err(zuse_rpc::RpcError::Capability(code)) if code.starts_with(zuse_protocol::cloud_task_commands::TASK_REJECTED_PREFIX) => {
                        // Refused outright (model, access, attachment): nothing was
                        // admitted, so the next Send builds a new message with the
                        // corrected choices instead of retrying this one.
                        this.return_native_send(cx);
                        this.cloud.pending_native = None;
                        this.cloud.pending_native_text = None;
                        this.cloud.native_staged.clear();
                        let message = code.trim_start_matches(zuse_protocol::cloud_task_commands::TASK_REJECTED_PREFIX).to_owned();
                        this.failure = Some(format!("Couldn't send: {message}").into());
                        this.release_ready_cloud_messages(cx);
                    }
                    Err(zuse_rpc::RpcError::Capability(code)) if code.starts_with(zuse_protocol::cloud_task_commands::TASK_UNKNOWN_OUTCOME_PREFIX) => {
                        // An earlier message's outcome is unknown, so nothing was
                        // admitted. Keep the draft and read history, which shows
                        // that message with Retry and Discard.
                        this.return_native_send(cx);
                        this.cloud.pending_native = None;
                        this.cloud.pending_native_text = None;
                        this.cloud.native_staged.clear();
                        this.failure = Some("Your earlier message didn't go through. Retry or discard it first. Your draft is kept.".into());
                        this.release_ready_cloud_messages(cx);
                        cx.emit(ComposerEvent::CloudLaunchChanged);
                    }
                    Err(error) if outbox::unreachable(&error) => {
                        // Never reached the server: wait for the connection
                        // and send the same message again.
                        this.cloud_send_unreachable(cx);
                    }
                    Err(error) => {
                        this.keep_unsent_native_send(cx);
                        this.failure = Some(format!("Couldn't send: {error} Retry sends the same message.").into());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Show the cloud computer's stage above the composer (`busy` spins).
    pub(crate) fn set_cloud_status(
        &mut self,
        status: Option<(String, bool)>,
        cx: &mut Context<Self>,
    ) {
        if self.cloud.status != status {
            self.cloud.status = status;
            cx.notify();
        }
    }
    #[cfg(test)]
    pub(crate) fn cloud_status(&self) -> Option<&(String, bool)> {
        self.cloud.status.as_ref()
    }
    /// The conversation's computer needs attention: offer Retry and Delete
    /// on its status line.
    pub(crate) fn set_cloud_status_actions(&mut self, actions: bool, cx: &mut Context<Self>) {
        if self.cloud.status_actions != actions {
            self.cloud.status_actions = actions;
            cx.notify();
        }
    }
    /// The owning cloud conversation's agent started or finished a turn.
    pub(crate) fn set_cloud_turn_running(&mut self, running: bool, cx: &mut Context<Self>) {
        if self.cloud.turn_running != running {
            self.cloud.turn_running = running;
            cx.notify();
        }
    }
    /// Whether the owning cloud conversation's agent is mid-turn.
    pub(crate) fn cloud_turn_running(&self) -> bool {
        self.cloud.turn_running
    }
    /// The one status line above the composer while a cloud computer starts:
    /// a new chat's launch, then the conversation's computer stage. A spinner
    /// and a short label while it works; a failed state with Retry and
    /// Delete when it can't go on.
    pub(super) fn cloud_status_row(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<super::tray::TrayPill> {
        if let Some((label, busy)) = self.cloud_creation_activity(cx) {
            let pill = super::tray::TrayPill::new("native-cloud-status", label.to_owned());
            if busy {
                let pill = match self.cloud.upload.as_ref() {
                    Some(upload) => pill.subtitle(upload.detail()),
                    None => pill,
                };
                return Some(pill.icon_element(crate::loaders::mini_mono_spinner(
                    "native-cloud-status-spinner",
                    2.,
                    theme.text_muted,
                    cx.entity_id(),
                    cx,
                )));
            }
            if self.launch_upload_failed() {
                return Some(
                    pill.glyph(crate::icons::DITHER_CLOUD)
                        .tone(super::tray::TrayTone::Danger)
                        .action(
                            super::tray::tray_text_action(
                                "retry-cloud-upload",
                                "Retry",
                                None,
                                theme,
                            )
                            .tip("Upload the file again")
                            .debug_selector(|| "retry-cloud-upload".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.input.read(cx).text().to_owned();
                                this.send_cloud(text, cx);
                            })),
                        )
                        .action(
                            super::tray::tray_text_action(
                                "remove-cloud-upload",
                                "Remove",
                                None,
                                theme,
                            )
                            .tip("Remove the file and keep your message as a draft")
                            .debug_selector(|| "remove-cloud-upload".into())
                            .on_click(
                                cx.listener(|this, _, _, cx| this.remove_failed_cloud_upload(cx)),
                            ),
                        ),
                );
            }
            return Some(
                pill.glyph(crate::icons::DITHER_CLOUD)
                    .tone(super::tray::TrayTone::Danger)
                    .action(
                        super::tray::tray_text_action("retry-cloud-launch", "Retry", None, theme)
                            .debug_selector(|| "retry-cloud-launch".into())
                            .on_click(cx.listener(|this, _, _, cx| {
                                let text = this.input.read(cx).text().to_owned();
                                this.send_cloud(text, cx);
                            })),
                    )
                    .action(
                        super::tray::tray_text_action("delete-cloud-launch", "Delete", None, theme)
                            .debug_selector(|| "delete-cloud-launch".into())
                            .on_click(cx.listener(|this, _, _, cx| this.discard_cloud_launch(cx))),
                    ),
            );
        }
        if let Some(upload) = self.cloud_send_upload() {
            // A message's files on their way: how far along they are.
            return Some(
                super::tray::TrayPill::new("cloud-send-upload", upload.label())
                    .subtitle(upload.detail())
                    .icon_element(crate::loaders::mini_mono_spinner(
                        "cloud-send-upload-spinner",
                        2.,
                        theme.text_muted,
                        cx.entity_id(),
                        cx,
                    )),
            );
        }
        let (label, busy) = self.cloud.status.clone()?;
        let pill = super::tray::TrayPill::new("native-cloud-status", label);
        if busy {
            return Some(pill.icon_element(crate::loaders::mini_mono_spinner(
                "native-cloud-status-spinner",
                2.,
                theme.text_muted,
                cx.entity_id(),
                cx,
            )));
        }
        let pill = pill.glyph(crate::icons::DITHER_CLOUD);
        if !self.cloud.status_actions {
            return Some(pill);
        }
        Some(
            pill.tone(super::tray::TrayTone::Danger)
                .action(
                    super::tray::tray_text_action("retry-cloud-computer", "Retry", None, theme)
                        .debug_selector(|| "retry-cloud-computer".into())
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::CloudStatusRetry)),
                        ),
                )
                .action(
                    super::tray::tray_text_action("delete-cloud-computer", "Delete", None, theme)
                        .debug_selector(|| "delete-cloud-computer".into())
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::CloudStatusDelete)),
                        ),
                ),
        )
    }
    /// The new chat's status line: its stage while it is created, or why it
    /// stopped. `busy` spins.
    pub(super) fn cloud_creation_activity(&self, cx: &App) -> Option<(String, bool)> {
        if self.cloud.request.is_none() || self.state.read(cx).selected_chat.is_some() {
            return None;
        }
        if self.cloud.task.is_some() {
            if let Some(upload) = &self.cloud.upload {
                return Some((upload.label(), true));
            }
            let label = self
                .cloud
                .message
                .as_deref()
                .map(short_stage)
                .unwrap_or("Starting computer…");
            return Some((label.to_owned(), true));
        }
        let failure = self
            .failure
            .as_ref()
            .filter(|_| {
                self.failure_key
                    .as_ref()
                    .is_none_or(|key| *key == self.current_key)
            })
            .map(|failure| failure.to_string());
        Some((
            failure.unwrap_or_else(|| {
                if self.cloud.admitted() {
                    "Cloud startup needs attention. Your message is queued.".into()
                } else {
                    "Cloud startup needs attention. Your message is saved here.".into()
                }
            }),
            false,
        ))
    }
    pub(crate) fn cloud_activity(&self) -> Option<(&str, bool)> {
        if self.cloud_waiting_for_connection() {
            // Not with the server yet, whether or not an attempt is running.
            return Some((outbox::WAITING, true));
        }
        if self.cloud_reupload_in_flight() {
            // Its files are going to the computer again: still just waiting.
            return Some(("Waiting for agent…", true));
        }
        if self.cloud.task.is_some() {
            return Some((
                match self
                    .cloud
                    .pending_native
                    .as_ref()
                    .and_then(|v| v["action"]["type"].as_str())
                {
                    Some("queue") => "Saving queued message…",
                    _ if self
                        .cloud
                        .pending_native
                        .as_ref()
                        .is_some_and(|v| v["action"]["payload"]["kind"] == "interrupt") =>
                    {
                        "Stopping agent…"
                    }
                    _ => "Sending message…",
                },
                true,
            ));
        }
        if self.cloud_unknown_retrying() {
            return Some(("Sending message…", true));
        }
        self.cloud
            .admitted_native
            .as_ref()
            .map(|_| ("Waiting for agent…", true))
    }
    pub(crate) fn apply_cloud_response_activity(
        &mut self,
        messages: &[zuse_model::SessionMessageEntry],
        cx: &mut Context<Self>,
    ) {
        self.observe_cloud_reupload_transcript(messages);
        if self
            .cloud
            .admitted_native
            .as_ref()
            .and_then(|pending| pending["messageId"].as_str())
            .is_some_and(|message| zuse_viewport::cloud_response_started(messages, message))
        {
            self.cloud.admitted_native = None;
            cx.notify();
        }
    }
    pub(crate) fn apply_cloud_admission_activity(
        &mut self,
        admissions: &Value,
        cx: &mut Context<Self>,
    ) {
        self.observe_cloud_unknown(admissions, cx);
        self.observe_cloud_reupload(admissions, cx);
        let Some(pending) = self.cloud.admitted_native.as_ref() else {
            return;
        };
        // The authoritative command row takes over even when it is queued.
        if admissions.as_array().is_some_and(|rows| {
            rows.iter()
                .any(|row| row["commandId"] == pending["commandId"])
        }) {
            self.cloud.admitted_native = None;
            cx.notify();
        }
    }

    /// The first message of a cloud chat that is still being created.
    pub(crate) fn cloud_launch_prompt(&self) -> Option<String> {
        self.cloud.request.as_ref()?["firstMessage"]
            .as_str()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    }
    /// The new chat is still being created (not failed).
    pub(crate) fn cloud_launch_working(&self) -> bool {
        self.cloud.request.is_some() && self.cloud.task.is_some()
    }
    pub(crate) fn cloud_launch_pending(&self, cx: &App) -> bool {
        self.cloud.request.is_some() && self.state.read(cx).selected_chat.is_none()
    }
    pub(super) fn cached_cloud(&self) -> bool {
        self.cloud.cached.is_some()
    }
    /// Bound to a cloud task, which sends with its own saved settings.
    pub(super) fn native_cloud_task(&self) -> bool {
        self.cloud.native.is_some()
    }
    /// A cloud message whose delivery isn't confirmed keeps its request.
    pub(super) fn cloud_send_unconfirmed(&self) -> bool {
        self.cloud.pending_native.is_some()
    }
    pub(crate) fn set_cached_cloud(&mut self, paused: bool, cx: &mut Context<Self>) {
        if self.cloud.cached != Some(paused) {
            self.cloud.cached = Some(paused);
            cx.notify();
        }
    }
    pub(super) fn live_cloud_row(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<super::tray::TrayPill> {
        if !self.cloud.rejected_drafts.is_empty() {
            return Some(super::tray::TrayPill::new("rejected-cloud-draft","Previous message was not queued")
                .subtitle("Your previous draft and attachments are preserved.")
                .action(super::tray::tray_text_action("restore-rejected-cloud-draft","Restore previous draft",None,theme)
                    .on_click(cx.listener(|this,_,_,cx| {
                        if !this.input.read(cx).text().is_empty() {
                            this.failure=Some("Clear or save the current draft before restoring the previous message.".into());cx.notify();return;
                        }
                        if !this.cloud.rejected_drafts.is_empty() {
                            let (text,staged)=this.cloud.rejected_drafts.remove(0);
                            this.restore_rejected_draft(text,staged,cx);
                            this.failure=Some("Draft restored. Check the selected repository before sending.".into());cx.notify();
                        }
                    }))));
        }
        if let Some(paused) = self.cloud.cached {
            return Some(
                super::tray::TrayPill::new(
                    "cached-cloud-connection",
                    if paused {
                        "Cloud workspace paused"
                    } else {
                        "Reconnect to cloud workspace"
                    },
                )
                .glyph(crate::icons::DITHER_CLOUD)
                .subtitle("Sending a message will resume this workspace.")
                .action(
                    super::tray::tray_text_action(
                        "resume-cloud-workspace",
                        if paused { "Resume" } else { "Reconnect" },
                        None,
                        theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.emit(ComposerEvent::ResumeCloud {
                            text: this.input.read(cx).text().to_owned(),
                            send: false,
                        });
                    })),
                ),
            );
        }
        self.state.read(cx).engine()?.cloud_workspace()?;
        let busy = self.cloud.pause_task.is_some();
        if !busy && self.cloud.pause_error.is_none() {
            return None;
        }
        let mut row = super::tray::TrayPill::new(
            "live-cloud-workspace",
            if busy {
                "Pausing cloud workspace…"
            } else {
                "Couldn’t pause cloud workspace"
            },
        )
        .glyph(crate::icons::DITHER_CLOUD);
        if let Some(error) = &self.cloud.pause_error {
            row = row.subtitle(error.clone());
        }
        Some(
            row.action(
                super::tray::tray_text_action(
                    "pause-cloud-workspace",
                    if busy {
                        "Pausing…"
                    } else if self.cloud.pause_error.is_some() {
                        "Retry pause"
                    } else {
                        "Pause"
                    },
                    None,
                    theme,
                )
                .opacity(if busy { 0.5 } else { 1. })
                .on_click(cx.listener(|this, _, window, cx| this.pause_cloud(window, cx))),
            ),
        )
    }
    pub(crate) fn cloud_has_draft(&self, cx: &App) -> bool {
        self.cloud.request.is_some()
            || self.has_draft(cx)
            || self
                .drafts
                .iter()
                .any(|(key, draft)| key != &self.current_key && !draft.trim().is_empty())
            || self.attachments.values().any(|files| !files.is_empty())
            || self.appshots.values().any(|captures| !captures.is_empty())
            || self.queue_edit_draft.is_some()
    }
    pub(crate) fn pause_cloud(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud.native.is_some() {
            cx.emit(ComposerEvent::CloudSleepRequested);
            return;
        }
        if self.cloud.pause_task.is_some() {
            return;
        }
        let unsaved_files = window
            .root::<crate::shell::Shell>()
            .flatten()
            .is_none_or(|shell| shell.read(cx).cloud_pause_has_unsaved_work(cx));
        if self.cloud_has_draft(cx) || unsaved_files {
            self.cloud.pause_error = Some("Send or clear drafts and save editor changes before pausing. Your work is preserved.".into());
            cx.notify();
            return;
        }
        let state = self.state.read(cx);
        let Some(engine) = state.engine() else {
            return;
        };
        let Some(workspace) = engine.cloud_workspace() else {
            return;
        };
        let Some(subject) = state.cloud_user_id().map(str::to_owned) else {
            return;
        };
        let target = crate::state::CloudTarget {
            controller: engine.controller(),
            subject,
            organization: state.cloud_organization_id().map(str::to_owned),
            workspace,
            initial_session: state.selected_chat.clone().unwrap_or_default(),
        };
        let command = self
            .cloud
            .pause_command
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let root = target.controller.clone();
        let request = json!({"workspaceId":target.workspace,"expectedAccountId":target.subject,"expectedOrganizationId":target.organization,"commandId":command});
        let handle = window.window_handle();
        self.cloud.pause_error = None;
        self.cloud.pause_task = Some(cx.spawn(async move |this, cx| {
            let result = root
                .client()
                .call_scoped(methods::CLOUD_PAUSE, request)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.cloud.pause_task = None;
                match result {
                    Ok(_) => {
                        gpui::App::defer(cx, move |cx| {
                            crate::cloud_history::saved::open(target, cx);
                            if let Some(handle) = handle.downcast::<crate::shell::Shell>() {
                                let _ = handle.update(cx, |shell, window, cx| {
                                    if shell.cloud_pause_can_close(cx)
                                        && shell.prepare_window_close(cx)
                                    {
                                        window.remove_window();
                                    }
                                });
                            }
                        });
                    }
                    Err(error) => this.cloud.pause_error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub(super) fn send_cloud(&mut self, text: String, cx: &mut Context<Self>) -> bool {
        if self.cloud.cached.is_some() {
            cx.emit(ComposerEvent::ResumeCloud { text, send: true });
            return true;
        }
        let state = self.state.read(cx);
        if state.selected_chat.is_some() {
            return false;
        }
        let Some(mut project) = state.cloud_project.clone() else {
            return false;
        };
        if self
            .cloud
            .request
            .as_ref()
            .is_some_and(|request| request["projectId"] != project["projectId"])
        {
            self.failure=Some("A previous cloud request belongs to another project. Check that computer or retry from its original project before starting different work. Your new draft is preserved.".into());
            cx.notify();
            return true;
        }
        if self.cloud.task.is_some() {
            return true;
        }
        if self.cloud.request.is_none() {
            let refreshed = state.cloud_sidebar_projects().into_iter().find(|row| {
                row["projectId"] == project["projectId"] && row["accountId"] == project["accountId"]
            });
            let access = refreshed.as_ref().unwrap_or(&project);
            if access["canCreate"] == false {
                let reason = access["creationBlockedReason"].as_str().unwrap_or(
                    "Select a repository available through your connected GitHub installation.",
                );
                self.failure = Some(format!("Draft saved · not queued. {reason}").into());
                cx.notify();
                return true;
            }
        }
        if self.cloud.request.is_none() {
            match self.pickers.read(cx).cloud_launch_provider(cx) {
                Ok(provider) => project["providerId"] = json!(provider),
                Err(reason) => {
                    self.failure = Some(format!("Draft saved · not queued. {reason}").into());
                    cx.notify();
                    return true;
                }
            }
        }
        if self.cloud.request.is_none() && !self.pickers.read(cx).cloud_model_ready(cx) {
            self.failure = Some(
                "Choose an available Cloud model before starting. Your draft is preserved.".into(),
            );
            self.failure_key = None;
            cx.notify();
            return true;
        }
        if !self.staged_appshots().is_empty() {
            self.failure = Some(
                "Start the cloud thread before attaching app captures. Your draft is preserved."
                    .into(),
            );
            self.failure_key = None;
            cx.notify();
            return true;
        }
        let Some(engine) = state.engine().cloned() else {
            self.cloud_start_refused(
                "Engine not connected. Reconnect, then send again; your draft is preserved.",
                cx,
            );
            return true;
        };
        let controller = engine.controller();
        let Some(subject) = state.cloud_user_id().map(str::to_owned) else {
            self.cloud_start_refused(
                "Sign in to your Zuse account to start a cloud thread. Your draft is preserved.",
                cx,
            );
            return true;
        };
        let organization = state.cloud_organization_id().map(str::to_owned);
        if self.cloud.owner.as_deref() != Some(subject.as_str())
            || self.cloud.organization != organization
        {
            self.cloud = Launch {
                owner: Some(subject.clone()),
                organization: organization.clone(),
                ..Default::default()
            };
        }
        let expected_owner = organization
            .as_ref()
            .map(|org| format!("organization:{org}"))
            .unwrap_or_else(|| subject.clone());
        if project["accountId"].as_str() != Some(expected_owner.as_str()) {
            self.failure = Some(
                "The account or organization changed. Select the cloud destination again.".into(),
            );
            self.failure_key = None;
            cx.notify();
            return true;
        }
        if self.cloud.request.is_none() {
            self.cloud.issue = self
                .pickers
                .read(cx)
                .source_issue(cx)
                .map(|(number, markdown)| {
                    StagedAttachment::from_file(
                        &format!("issue-{number}.md"),
                        markdown.into_bytes(),
                    )
                });
        }
        let mut staged = if self.cloud.request.is_some() {
            self.cloud.staged.clone()
        } else {
            self.staged().to_vec()
        };
        if self.cloud.request.is_none() {
            staged.extend(self.cloud.issue.iter().cloned());
        }
        if self.cloud.request.is_none() {
            let (base_ref, branch) = self
                .pickers
                .read(cx)
                .source_cloud_ref(project["defaultBranch"].as_str().unwrap_or("main"), cx);
            let config = self.pickers.read(cx).resolved(cx);
            let (Some(agent), Some(model)) = (config.harness, config.model) else {
                self.cloud_start_refused(
                    "Choose a coding agent and model before starting. Your draft is preserved.",
                    cx,
                );
                return true;
            };
            if config.plan {
                self.failure = Some(tr(cx, msg::native::composer_cloud_plan_unavailable).into());
                self.failure_key = None;
                cx.notify();
                return true;
            }
            let size = match self
                .pickers
                .read(cx)
                .cloud_launch_size(project["providerId"].as_str().unwrap_or_default(), cx)
            {
                Ok(size) => size,
                Err(reason) => {
                    self.failure = Some(format!("Draft saved · not queued. {reason}").into());
                    self.failure_key = None;
                    cx.notify();
                    return true;
                }
            };
            let Some(connection) = self.pickers.read(cx).cloud_connection_for(agent) else {
                self.failure=Some("This agent needs one usable connection. Check its connection in Settings → Coding agents. Your draft is preserved.".into());
                self.failure_key = None;
                cx.notify();
                return true;
            };
            let runtime_mode = match config.sandbox {
                SandboxLevel::WorkspaceWrite => "approval-required",
                SandboxLevel::Auto => "auto",
                SandboxLevel::DangerFullAccess => "full-access",
            };
            // The launch belongs to this tab: give the draft its own key so
            // the composer can serve other tabs while the chat is created.
            self.ensure_cloud_canvas_key(cx);
            self.cloud.started_at = Some(std::time::Instant::now());
            self.cloud.request = Some(
                json!({"runtimeMode":runtime_mode,"agentConnectionId":connection,"projectId":project["projectId"],"providerId":project["providerId"],"sizeId":size,"baseRef":base_ref,"branch":branch,"agent":agent,"model":model,"reasoning":config.reasoning,"modelOptions":config.model_options,"idempotencyKey":uuid::Uuid::new_v4().to_string(),"firstMessage":text,"expectedAccountId":subject,"expectedOrganizationId":organization,"attachmentTransfers":staged.iter().map(|a|json!({"uploadId":a.id,"fileName":a.name})).collect::<Vec<_>>()}),
            );
            if project[crate::shell::SETUP_PROJECT_FLAG] == true {
                self.cloud.request.as_mut().unwrap()["purpose"] =
                    json!(zuse_protocol::machine_setup::PURPOSE);
            }
            let provider = project["providerId"].as_str().unwrap_or("boxd");
            if self.pickers.read(cx).cloud_v2_available(provider) {
                self.cloud.request.as_mut().unwrap()["initialMessageDelivery"] =
                    json!("server-admitted-v2");
            }
            // Files declared up front: the computer starts now and the
            // bytes upload beside it; the server holds the first turn until
            // they land.
            if !staged.is_empty()
                && self.pickers.read(cx).cloud_launch_attachment_hold(provider)
                && let Some(pending) = launch_upload::pending_attachments(&staged)
            {
                let request = self.cloud.request.as_mut().unwrap();
                request["pendingAttachments"] = json!(pending);
                request["attachmentTransfers"] = json!([]);
            }
            self.cloud.staged = staged.clone();
            self.input.update(cx, |input, cx| input.set_text("", cx));
            let key = self.current_key.clone();
            if let Some(files) = self.attachments.get_mut(&key) {
                files.retain(|file| !staged.iter().any(|sent| sent.id == file.id));
            }
            self.sync_attachment_chips(cx);
            cx.emit(ComposerEvent::CloudLaunchChanged);
        }
        self.pickers
            .update(cx, |pickers, cx| pickers.freeze_cloud_launch(true, cx));
        let request = self.cloud.request.clone().unwrap();
        let accepted = self.cloud.accepted.clone();
        let hold = request["pendingAttachments"]
            .as_array()
            .is_some_and(|files| !files.is_empty());
        self.cloud.message = Some(
            if staged.is_empty() || hold {
                "Starting computer"
            } else {
                "Uploading attachments…"
            }
            .into(),
        );
        if let Some(upload) = self.cloud.upload.as_mut() {
            upload.failed = None;
        }
        self.failure = None;
        let launch_controller = controller.clone();
        // The launch belongs to this tab's draft: progress and the outcome land
        // on it even after the user moves to another tab or chat.
        let key = self.current_key.clone();
        self.cloud.task = Some(cx.spawn(async move |this, cx| {
            if accepted.is_none() && !hold {
                for attachment in &staged {
                    if let Err(error) = attachments::upload_attachment(
                        &engine,
                        cx.background_executor(),
                        None,
                        &attachment.id,
                        attachment,
                        None,
                    )
                    .await
                    {
                        let _ = this.update(cx, |this, cx| {
                            this.on_launch(&key, cx, |this, cx| this.cloud_failure(error, cx))
                        });
                        return;
                    }
                }
            }
            let create = gpui_tokio::Tokio::spawn(cx, async move {
                match accepted {
                    Some(value) => Ok(value),
                    None => {
                        let response = launch_controller
                            .client()
                            .call_scoped(methods::CLOUD_CREATE, request)
                            .await?;
                        let value = &response["workspace"];
                        Ok::<Value, zuse_rpc::RpcError>(
                            value.get("workspace").unwrap_or(value).clone(),
                        )
                    }
                }
            });
            // A server that never answers must not hold the tab: past the
            // deadline the launch shows as failed with Retry and Delete.
            let deadline = cx.background_executor().timer(CLOUD_CREATE_DEADLINE);
            let result = match futures::future::select(create, deadline).await {
                futures::future::Either::Left((result, _)) => Some(result),
                futures::future::Either::Right(_) => None,
            };
            let workspace = match result {
                Some(Ok(Ok(value))) => value,
                _ => {
                    let message = match result {
                        Some(Ok(Err(error))) => error.to_string(),
                        None => CLOUD_CREATE_TIMEOUT.into(),
                        _ => {
                            "Cloud request interrupted. Retry the same request to check acceptance."
                                .into()
                        }
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.on_launch(&key, cx, |this, cx| this.cloud_create_failure(message, cx))
                    });
                    return;
                }
            };
            let id = workspace["workspaceId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let initial = workspace["initialSessionId"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            if id.is_empty() || initial.is_empty() {
                let _ = this.update(cx, |this, cx| {
                    this.on_launch(&key, cx, |this, cx| {
                        this.cloud_failure("Invalid cloud launch response".into(), cx)
                    })
                });
                return;
            }
            // The computer a chat is started from is its primary. Only a
            // created chat claims it; the launch never waits long for it.
            super::computers::claim_primary(
                crate::state::CloudTarget {
                    controller: controller.clone(),
                    subject: subject.clone(),
                    organization: organization.clone(),
                    workspace: id.clone(),
                    initial_session: initial.clone(),
                },
                initial.clone(),
                None,
                cx.background_executor().clone(),
            )
            .await;
            if workspace["commandDeliveryMode"] == "server-admitted-v2"
                && workspace["commandAdmissionVersion"] == 2
            {
                if hold {
                    // The computer is starting: its first turn waits on the
                    // server for these files. A Retry skips files that
                    // already committed.
                    let mut progress = None;
                    let _ = this.update(cx, |this, cx| {
                        this.on_launch(&key, cx, |this, cx| {
                            let resumed = launch_upload::Progress::resume(
                                &staged,
                                this.cloud.upload.as_ref(),
                            );
                            this.cloud.accepted = Some(workspace.clone());
                            this.cloud.upload = Some(resumed.clone());
                            progress = Some(resumed);
                            cx.emit(ComposerEvent::CloudLaunchChanged);
                            cx.notify();
                        })
                    });
                    let Some(progress) = progress else {
                        return;
                    };
                    let engine = controller.clone();
                    let account = subject.clone();
                    let scope = organization.clone();
                    let call =
                        launch_upload::asset_call(&engine, &id, &account, scope.as_deref());
                    let executor = cx.background_executor().clone();
                    let mut upload = std::pin::pin!(launch_upload::upload_files(
                        &call, &executor, &staged, &progress
                    ));
                    let result = loop {
                        let paint = executor.timer(launch_upload::PAINT_INTERVAL);
                        match futures::future::select(upload.as_mut(), paint).await {
                            futures::future::Either::Left((result, _)) => break result,
                            futures::future::Either::Right(_) => {
                                if this
                                    .update(cx, |this, cx| {
                                        this.on_launch(&key, cx, |_, cx| cx.notify())
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.on_launch(&key, cx, |this, cx| match result {
                            Ok(()) => {
                                this.finish_admitted_launch(workspace, subject, organization, cx)
                            }
                            Err(failure) => this.launch_upload_failure(failure, cx),
                        })
                    });
                    return;
                }
                let _ = this.update(cx, |this, cx| {
                    this.on_launch(&key, cx, |this, cx| {
                        this.finish_admitted_launch(workspace, subject, organization, cx)
                    })
                });
                return;
            }
            if this
                .update(cx, |this, cx| {
                    this.on_launch(&key, cx, |this, cx| {
                        this.cloud.accepted = Some(workspace);
                        this.cloud.message = Some("Starting computer".into());
                        cx.emit(ComposerEvent::CloudLaunchChanged);
                        cx.notify();
                    })
                })
                .is_err()
            {
                return;
            }
            let root = controller.clone();
            let workspace_id = id.clone();
            let open_subject = subject.clone();
            let open_organization = organization.clone();
            let open = gpui_tokio::Tokio::spawn(cx, async move {
                root.client()
                    .subscribe_scoped(
                        methods::CLOUD_ENVIRONMENT,
                        json!({"workspaceId":workspace_id,"expectedAccountId":open_subject,"expectedOrganizationId":open_organization}),
                    )
                    .await
            });
            let mut updates = match open.await {
                Ok(Ok(updates)) => updates,
                _ => {
                    let _ = this.update(cx, |this, cx| {
                        this.on_launch(&key, cx, |this, cx| {
                            this.cloud_failure(
                                "Cloud connection interrupted. Retry to reopen the accepted workspace."
                                    .into(),
                                cx,
                            )
                        })
                    });
                    return;
                }
            };
            // A computer that never becomes ready ends in a failed state too.
            let ready_by = std::time::Instant::now() + CLOUD_READY_DEADLINE;
            loop {
                let remaining = ready_by.saturating_duration_since(std::time::Instant::now());
                let next = futures::future::select(
                    Box::pin(updates.recv()),
                    cx.background_executor().timer(remaining),
                )
                .await;
                let value = match next {
                    futures::future::Either::Left((Some(value), _)) => value,
                    futures::future::Either::Left((None, _)) => break,
                    futures::future::Either::Right(_) => {
                        let _ = this.update(cx, |this, cx| {
                            this.on_launch(&key, cx, |this, cx| {
                                this.cloud_failure(CLOUD_READY_TIMEOUT.into(), cx)
                            })
                        });
                        return;
                    }
                };
                if value["state"] == "ready" {
                    let _ = this.update(cx, |this, cx| {
                        this.on_launch(&key, cx, |this, cx| {
                            let workspace = this.cloud.accepted.clone().unwrap_or_default();
                            this.finish_admitted_launch(workspace, subject, organization, cx)
                        })
                    });
                    return;
                }
                if matches!(value["state"].as_str(), Some("blocked" | "failed")) {
                    let _ = this.update(cx, |this, cx| {
                        this.on_launch(&key, cx, |this, cx| {
                            this.cloud_failure(
                                value["message"]
                                    .as_str()
                                    .unwrap_or("Cloud startup failed")
                                    .into(),
                                cx,
                            )
                        })
                    });
                    return;
                }
                if this
                    .update(cx, |this, cx| {
                        this.on_launch(&key, cx, |this, cx| {
                            this.cloud.message = value["message"].as_str().map(str::to_owned);
                            cx.notify();
                        })
                    })
                    .is_err()
                {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.on_launch(&key, cx, |this, cx| {
                    this.cloud_failure(
                        "Cloud startup connection closed. Retry to reopen the accepted workspace."
                            .into(),
                        cx,
                    )
                })
            });
        }));
        cx.emit(ComposerEvent::CloudLaunchChanged);
        cx.notify();
        true
    }
    /// The server admitted the new chat: hand it to the shell, which opens
    /// it in the tab that sent it.
    fn finish_admitted_launch(
        &mut self,
        workspace: Value,
        account: String,
        organization: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let launch = self.cloud.request.take().unwrap_or_default();
        self.cloud.accepted = None;
        self.cloud.upload = None;
        self.cloud.staged.clear();
        self.cloud.issue = None;
        self.cloud.task = None;
        self.cloud.message = None;
        self.cloud.failure = None;
        if !self.parked_launch_run {
            self.pickers
                .update(cx, |pickers, cx| pickers.finish_cloud_launch(cx));
        }
        cx.emit(ComposerEvent::CloudAdmitted {
            key: self.current_key.clone(),
            workspace,
            account,
            organization,
            launch,
        });
        cx.emit(ComposerEvent::CloudLaunchChanged);
        cx.notify();
    }
    /// The server admits this tab's pending creation, as its create response
    /// does: the production hand-off to the shell.
    #[cfg(test)]
    pub(crate) fn admit_pending_cloud_creation_for_test(
        &mut self,
        workspace: Value,
        cx: &mut Context<Self>,
    ) {
        self.finish_admitted_launch(workspace, "owner".into(), None, cx);
    }
    /// Run `update` against the launch that `key`'s tab sent. While that tab
    /// is in front this is the composer's own launch; otherwise the parked
    /// launch is swapped in for the update and parked again after, so the
    /// tab in front keeps its own draft, pickers and errors.
    pub(super) fn on_launch(
        &mut self,
        key: &str,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut Self, &mut Context<Self>),
    ) {
        if self.current_key == key {
            update(self, cx);
            return;
        }
        let Some(parked) = self.parked_launches.remove(key) else {
            // The tab was closed and its launch discarded.
            return;
        };
        let active = self.cloud.take_launch();
        let failure = (self.failure.take(), self.failure_key.take());
        let current = std::mem::replace(&mut self.current_key, key.to_owned());
        self.cloud.put_launch(parked);
        self.parked_launch_run = true;
        update(self, cx);
        self.parked_launch_run = false;
        let mut done = self.cloud.take_launch();
        if let Some(error) = self.failure.take() {
            done.failure = Some(error);
        }
        self.current_key = current;
        if done.pending() {
            self.parked_launches.insert(key.to_owned(), done);
        }
        self.cloud.put_launch(active);
        (self.failure, self.failure_key) = failure;
        cx.emit(ComposerEvent::CloudLaunchChanged);
        cx.notify();
    }
    /// Navigation moved the composer from `from` to `to`: a pending launch
    /// stays with its tab, and the one waiting for `to` comes back.
    pub(super) fn swap_cloud_launch(&mut self, from: &str, to: &str, cx: &mut Context<Self>) {
        if self.cloud.pending() {
            let mut launch = self.cloud.take_launch();
            if self.failure_key.as_deref() == Some(from) {
                launch.failure = self.failure.take();
                self.failure_key = None;
            }
            self.parked_launches.insert(from.to_owned(), launch);
            self.pickers
                .update(cx, |pickers, cx| pickers.freeze_cloud_launch(false, cx));
        }
        if let Some(mut launch) = self.parked_launches.remove(to) {
            if let Some(error) = launch.failure.take() {
                self.failure = Some(error);
                self.failure_key = Some(to.to_owned());
            }
            self.cloud.put_launch(launch);
            // The pickers freeze again once the tab's own choices are back
            // (see `refreeze_cloud_launch`).
        }
    }
    /// The tab with a pending launch is in front again with its choices
    /// restored: lock them while the chat is being created.
    pub(crate) fn refreeze_cloud_launch(&mut self, cx: &mut Context<Self>) {
        if self.cloud.pending() {
            self.pickers
                .update(cx, |pickers, cx| pickers.freeze_cloud_launch(true, cx));
        }
    }
    /// Whether the tab drafted under `key` is creating a cloud chat.
    pub(crate) fn has_cloud_launch(&self, key: &str) -> bool {
        if key == self.current_key {
            self.cloud.pending()
        } else {
            self.parked_launches.contains_key(key)
        }
    }
    /// The pending launch of the tab drafted under `key`, for its sidebar
    /// row and tab label.
    pub(crate) fn cloud_launch_summary(&self, key: &str) -> Option<CloudLaunchSummary> {
        let launch = if key == self.current_key {
            &self.cloud
        } else {
            self.parked_launches.get(key)?
        };
        let request = launch.request.as_ref()?;
        Some(CloudLaunchSummary {
            key: key.to_owned(),
            project_id: request["projectId"].as_str().unwrap_or_default().to_owned(),
            first_message: request["firstMessage"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            harness: serde_json::from_value(request["agent"].clone()).ok(),
            workspace: launch.accepted.as_ref().and_then(|row| {
                row["workspaceId"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .map(str::to_owned)
            }),
            failed: launch.task.is_none(),
            started_at: launch.started_at,
        })
    }
    /// Every pending launch, the one in front first.
    pub(crate) fn cloud_launch_summaries(&self) -> Vec<CloudLaunchSummary> {
        std::iter::once(self.current_key.as_str())
            .chain(self.parked_launches.keys().map(String::as_str))
            .filter_map(|key| self.cloud_launch_summary(key))
            .collect()
    }
    /// Delete the failed (or stuck) chat this tab is creating: an accepted
    /// computer is archived, and the first message returns to the draft.
    pub(crate) fn discard_cloud_launch(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.cloud.request.take() else {
            return;
        };
        let accepted = self.cloud.accepted.take();
        self.cloud.task = None;
        self.cloud.message = None;
        self.cloud.issue = None;
        self.cloud.failure = None;
        self.cloud.upload = None;
        let staged = std::mem::take(&mut self.cloud.staged);
        if self
            .failure_key
            .as_ref()
            .is_none_or(|key| *key == self.current_key)
        {
            self.failure = None;
            self.failure_key = None;
        }
        self.pickers
            .update(cx, |pickers, cx| pickers.freeze_cloud_launch(false, cx));
        let text = request["firstMessage"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if self.input.read(cx).text().is_empty() {
            self.restore_rejected_draft(text, staged, cx);
        } else {
            // A newer draft is in the box: the message and its files wait
            // behind "Restore previous draft" instead of being lost.
            self.cloud.rejected_drafts.push((text, staged));
        }
        self.archive_abandoned_launch(request, accepted, cx);
        cx.emit(ComposerEvent::CloudLaunchChanged);
        cx.notify();
    }
    /// A launch whose tab was closed or deleted must not leave a computer
    /// running unnoticed: an accepted computer is archived; one whose create
    /// may still be in flight is looked up by replaying the same create
    /// (idempotent by its key) and archived once the server returns it.
    pub(super) fn archive_abandoned_launch(
        &mut self,
        request: Value,
        accepted: Option<Value>,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let controller = engine.controller();
        let known = accepted
            .as_ref()
            .and_then(|row| row["workspaceId"].as_str())
            .map(str::to_owned);
        gpui_tokio::Tokio::spawn(cx, async move {
            let workspace = match known {
                Some(workspace) => workspace,
                None => match controller
                    .client()
                    .call_scoped(methods::CLOUD_CREATE, request.clone())
                    .await
                {
                    Ok(response) => {
                        let row = &response["workspace"];
                        let row = row.get("workspace").unwrap_or(row);
                        match row["workspaceId"].as_str().filter(|id| !id.is_empty()) {
                            Some(id) => id.to_owned(),
                            None => return,
                        }
                    }
                    // Refused: no computer exists for this request.
                    Err(_) => return,
                },
            };
            // One command identity across tries: a slow archive that did
            // land is not archived twice. A dropped call is tried again so
            // the computer never keeps running unnoticed.
            let params = json!({"workspaceId":workspace,
                "expectedAccountId":request["expectedAccountId"],
                "expectedOrganizationId":request["expectedOrganizationId"],
                "commandId":uuid::Uuid::new_v4().to_string()});
            for attempt in 1..=ARCHIVE_ATTEMPTS {
                match controller
                    .client()
                    .call_scoped(methods::CLOUD_ARCHIVE, params.clone())
                    .await
                {
                    Ok(_) => return,
                    Err(error) => {
                        tracing::warn!(%error, attempt, "archiving an abandoned cloud launch failed");
                        if attempt < ARCHIVE_ATTEMPTS {
                            tokio::time::sleep(std::time::Duration::from_secs(2u64.pow(attempt)))
                                .await;
                        }
                    }
                }
            }
        })
        .detach();
    }
    /// A tab that is closed takes its pending chat creation with it, without
    /// leaving its computer behind.
    pub(super) fn abandon_launch(&mut self, mut launch: Launch, cx: &mut Context<Self>) {
        launch.task = None;
        if let Some(request) = launch.request.take() {
            self.archive_abandoned_launch(request, launch.accepted.take(), cx);
        }
    }
    fn cloud_create_failure(&mut self, message: String, cx: &mut Context<Self>) {
        // A parked tab keeps its request: Retry there gets the same answer
        // and handles it in front of the user.
        if !self.parked_launch_run
            && self.cloud.accepted.is_none()
            && zuse_engine::cloud::definitive_create_rejection(&message).is_some()
        {
            if let Some(request) = self.cloud.request.take() {
                let text = request["firstMessage"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let staged = std::mem::take(&mut self.cloud.staged);
                let same_project = self
                    .state
                    .read(cx)
                    .cloud_project
                    .as_ref()
                    .is_some_and(|project| project["projectId"] == request["projectId"]);
                if same_project && self.input.read(cx).text().is_empty() {
                    self.restore_rejected_draft(text, staged, cx);
                } else {
                    self.cloud.rejected_drafts.push((text, staged));
                }
            }
            self.cloud.issue = None;
            self.cloud.task = None;
            self.cloud.message = None;
            self.pickers
                .update(cx, |pickers, cx| pickers.finish_cloud_launch(cx));
            self.failure = Some(format!("Draft saved · not queued. {message}").into());
            self.failure_key = None;
            cx.emit(ComposerEvent::CloudLaunchChanged);
            cx.notify();
        } else {
            self.cloud_failure(message, cx);
        }
    }
    fn restore_rejected_draft(
        &mut self,
        text: String,
        staged: Vec<StagedAttachment>,
        cx: &mut Context<Self>,
    ) {
        self.input.update(cx, |input, cx| input.set_text(text, cx));
        let files = self
            .attachments
            .entry(self.current_key.clone())
            .or_default();
        for attachment in staged {
            if !files.iter().any(|file| file.id == attachment.id) {
                files.push(attachment);
            }
        }
        self.sync_attachment_chips(cx);
    }
    /// A cloud start refused before any request: shown inline and logged,
    /// never a silent no-op.
    fn cloud_start_refused(&mut self, reason: &str, cx: &mut Context<Self>) {
        tracing::warn!(%reason, "cloud thread start refused");
        self.failure = Some(reason.to_owned().into());
        self.failure_key = None;
        cx.notify();
    }
    /// A first-message file ran out of attempts while the computer started:
    /// the launch waits with Retry (the same files to the same computer)
    /// and Remove.
    fn launch_upload_failure(&mut self, failure: launch_upload::Failure, cx: &mut Context<Self>) {
        tracing::warn!(error = %failure.error, "cloud launch upload failed");
        if let Some(upload) = self.cloud.upload.as_mut() {
            upload.failed = Some(failure.file);
        }
        self.cloud_failure(
            format!(
                "Couldn't upload {}. Your message is saved here.",
                failure.name
            ),
            cx,
        );
    }
    /// The launch's upload stopped and offers Remove.
    pub(super) fn launch_upload_failed(&self) -> bool {
        self.cloud.task.is_none()
            && self
                .cloud
                .upload
                .as_ref()
                .is_some_and(|upload| upload.failed.is_some())
    }
    /// Remove the file that wouldn't upload. The started computer is archived
    /// (its first turn waits for that file), and the message with its other
    /// files returns to the draft, ready to send again.
    pub(crate) fn remove_failed_cloud_upload(&mut self, cx: &mut Context<Self>) {
        let Some(failed) = self
            .cloud
            .upload
            .as_ref()
            .and_then(|upload| upload.failed.clone())
        else {
            return;
        };
        self.discard_cloud_launch(cx);
        if let Some(files) = self.attachments.get_mut(&self.current_key) {
            files.retain(|file| file.id != failed);
        }
        for (_, files) in &mut self.cloud.rejected_drafts {
            files.retain(|file| file.id != failed);
        }
        self.sync_attachment_chips(cx);
        cx.notify();
    }
    pub(super) fn cloud_failure(&mut self, message: String, cx: &mut Context<Self>) {
        self.cloud.task = None;
        self.cloud.message = None;
        self.failure = Some(message.into());
        // A new chat's failure stays on its own tab.
        self.failure_key = self
            .cloud
            .request
            .is_some()
            .then(|| self.current_key.clone());
        cx.emit(ComposerEvent::CloudLaunchChanged);
        cx.notify();
    }
}

/// How a cloud chat's message goes out, as in a chat on this device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CloudSendKind {
    /// The agent is idle: the message starts the next turn.
    Run,
    /// Into the running turn, shown in the chat at once.
    Steer,
    /// Behind the running turn, shown in the queue at once.
    Queue,
}

/// `busy`: the agent is running. A steer goes into the running turn only
/// when its agent reads messages mid-turn and it carries no files;
/// otherwise it waits in the queue like any message sent while busy.
pub(super) fn cloud_send_kind(
    busy: bool,
    steer: bool,
    has_files: bool,
    steers_mid_turn: bool,
) -> CloudSendKind {
    if !busy {
        CloudSendKind::Run
    } else if steer && !has_files && steers_mid_turn {
        CloudSendKind::Steer
    } else {
        CloudSendKind::Queue
    }
}

/// A cloud chat's computer was asked to wake but isn't connected yet.
pub(crate) enum ComputerWait {
    /// The composer went away while waiting.
    Gone,
    /// The computer didn't connect within [`COMPUTER_WAIT`].
    TimedOut,
}
/// How long a message's files wait for their computer to connect.
pub(crate) const COMPUTER_WAIT: std::time::Duration = std::time::Duration::from_secs(180);
pub(crate) const COMPUTER_TIMEOUT: &str =
    "The computer didn't start in time to receive the files. Send again to retry.";
/// Wake a cloud chat's computer if it sleeps and wait up to
/// [`COMPUTER_WAIT`] for its connection, so files can go straight to it
/// instead of server storage.
pub(crate) async fn computer_engine(
    this: &gpui::WeakEntity<Composer>,
    cx: &mut gpui::AsyncApp,
) -> Result<crate::state::EngineHandle, ComputerWait> {
    let ready = |cx: &mut gpui::AsyncApp| {
        this.read_with(cx, |this, cx| {
            let state = this.state.read(cx);
            (state.connection == crate::state::ConnectionStatus::Ready)
                .then(|| state.engine().cloned())
                .flatten()
        })
    };
    match ready(cx) {
        Ok(Some(engine)) => return Ok(engine),
        Ok(None) => {}
        Err(_) => return Err(ComputerWait::Gone),
    }
    let _ = this.update(cx, |_, cx| cx.emit(ComposerEvent::CloudWakeRequested));
    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > COMPUTER_WAIT {
            return Err(ComputerWait::TimedOut);
        }
        cx.background_executor()
            .timer(std::time::Duration::from_millis(250))
            .await;
        match ready(cx) {
            Ok(Some(engine)) => return Ok(engine),
            Ok(None) => {}
            Err(_) => return Err(ComputerWait::Gone),
        }
    }
}
/// Upload files straight to a connected cloud computer, two at a time (each
/// upload holds its whole encoded file), keeping their order. Returns their
/// paths on the computer.
pub(crate) async fn upload_to_computer(
    engine: &crate::state::EngineHandle,
    executor: &gpui::BackgroundExecutor,
    staged: &[StagedAttachment],
) -> Result<Vec<String>, String> {
    use futures::{StreamExt as _, TryStreamExt as _};
    futures::stream::iter(staged.iter())
        .map(|attachment| {
            attachments::upload_attachment(
                engine,
                executor,
                Some(attachments::COMPUTER),
                &attachment.id,
                attachment,
                None,
            )
        })
        .buffered(2)
        .try_collect::<Vec<_>>()
        .await
        .map_err(|error| format!("The files couldn't reach the computer: {error}"))
}
/// Record where a message's files landed on its computer.
fn set_computer_files(request: &mut Value, paths: Vec<String>) {
    if request["action"]["type"] == "queue" {
        request["action"]["input"]["computerFiles"] = json!(paths);
    } else {
        request["action"]["payload"]["request"]["attachments"] = json!(paths);
    }
}

#[cfg(test)]
mod cached_tests {
    use super::*;

    struct AdmissionReceiptService;
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for AdmissionReceiptService {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            if method == "ClaimComputerPrimary" {
                return Ok(zuse_rpc::RpcReply::Value(json!({"deviceId":null})));
            }
            assert_eq!(method, methods::CLOUD_TASK_COMMAND);
            assert_eq!(params["request"]["commandId"], "sent-once");
            Ok(zuse_rpc::RpcReply::Value(
                json!({"accountId":"owner","organizationId":null,"data":{"commandId":"sent-once","durablyAdmitted":true}}),
            ))
        }
    }
    #[gpui::test]
    fn admitted_followup_keeps_activity_until_its_authoritative_receipt(
        cx: &mut gpui::TestAppContext,
    ) {
        // Drive both executors explicitly; no background Tokio thread can wake
        // the GPUI deterministic scheduler after teardown.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(AdmissionReceiptService),
        ));
        let state = cx.new(|_| AppState::new());
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx,|composer,cx| {
            composer.cloud.native=Some(NativeTask{target:crate::state::CloudTarget{controller,subject:"owner".into(),organization:None,workspace:"task".into(),initial_session:"session".into()},wake:false,queue_selection_version:1});
            composer.cloud.pending_native=Some(json!({"commandId":"sent-once","sessionId":"session","action":{"type":"submit","messageId":"message","payload":{"kind":"run","request":{"prompt":"Do this"}}}}));
            composer.cloud.pending_native_text=Some("Do this".into());
            composer.dispatch_cloud_task(cx);
            assert_eq!(composer.cloud_activity(),Some(("Sending message…",true)));
        });
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        cx.run_until_parked();
        composer.update(cx, |composer, cx| {
            assert!(
                composer.cloud.task.is_none(),
                "actual memory RPC receipt completed"
            );
            assert!(composer.cloud.pending_native.is_none());
            assert_eq!(
                composer.cloud_activity(),
                Some(("Waiting for agent…", true)),
                "receipt must not clear pre-response loader"
            );
            let receipt = composer.cloud.admitted_native.clone();
            let entry = |id: &str, role, text: &str| zuse_model::SessionMessageEntry {
                id: id.into(),
                role,
                parts: vec![zuse_model::MessagePart::Text {
                    id: format!("text:{id}"),
                    text: text.into(),
                }],
                created_at: 0,
                device_id: String::new(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            };
            let mut transcript = vec![
                entry("message", zuse_model::MessageRole::User, "Do this"),
                entry("answer", zuse_model::MessageRole::Assistant, ""),
            ];
            composer.apply_cloud_response_activity(&transcript, cx);
            assert!(
                composer.cloud_activity().is_some(),
                "empty assistant placeholder is not a response"
            );
            transcript[1] = entry(
                "answer",
                zuse_model::MessageRole::Assistant,
                "Working on it",
            );
            composer.apply_cloud_response_activity(&transcript, cx);
            assert!(
                composer.cloud_activity().is_none(),
                "actual exact-message response clears only waiting bridge"
            );
            composer.cloud.admitted_native = receipt;
            composer.apply_cloud_admission_activity(
                &json!([{"commandId":"other","executionState":"completed"}]),
                cx,
            );
            assert!(
                composer.cloud_activity().is_some(),
                "unrelated receipt cannot clear pending send"
            );
            composer.apply_cloud_admission_activity(
                &json!([{"commandId":"sent-once","executionState":"completed"}]),
                cx,
            );
            assert!(composer.cloud_activity().is_none());
            composer.cloud.pending_native = Some(json!({"action":{"type":"queue"}}));
            assert!(
                composer.cloud_activity().is_none(),
                "retained queue request alone is not running work"
            );
        });
    }
    #[gpui::test]
    fn cloud_queue_preserves_selected_run_config_and_old_server_draft(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        for version in [0, 1] {
            let (send, receive) = tokio::sync::mpsc::channel(1);
            let (_replies, replies) = tokio::sync::mpsc::channel(1);
            let controller = crate::state::EngineHandle::from_test_client(
                zuse_rpc::RpcClient::new(send, replies),
            );
            let state = cx.new(|_| {
                let mut state = crate::state::AppState::new();
                state.apply_cloud_account(json!({"user":{"id":"queue-owner"},"orgId":null}));
                state
            });
            let composer = cx.new(|cx| Composer::new(state, cx));
            composer.update(cx,|c,cx| {
                let target=crate::state::CloudTarget{controller,subject:"queue-owner".into(),organization:None,workspace:"task".into(),initial_session:"session".into()};
                c.set_cloud_task(target,json!({"agent":"grok","model":"selected-grok","reasoning":"high","modelOptions":{"speed":"fast"},"sandbox":"workspace-write","queueRunSelectionVersion":version}),cx);
                c.input.update(cx,|input,cx|input.set_text("Keep this queued draft",cx));
                c.attachments.insert(c.current_key.clone(),vec![StagedAttachment::from_file("notes.txt",b"retained bytes".to_vec())]);
                assert!(c.send_cloud_task("Keep this queued draft".into(),true,cx));
                if version==0 {assert_eq!(c.input.read(cx).text(),"Keep this queued draft");assert_eq!(c.staged().len(),1);assert!(c.cloud.pending_native.is_none());assert!(c.failure.as_deref().unwrap().contains("draft is preserved"));}
                else {assert_eq!(c.input.read(cx).text(),"","a queued message leaves the draft at once");assert!(c.staged().is_empty());let request=c.cloud.pending_native.as_ref().unwrap();assert_eq!(request["action"]["input"]["runSelection"],json!({"harness":"grok","model":"selected-grok","reasoning":"high","modelOptions":{"speed":"fast"},"sandbox":"workspace-write"}));assert_eq!(request["action"]["input"]["text"],"Keep this queued draft");assert_eq!(c.cloud.native_staged.len(),1);c.cloud.task=None;}
            });
            drop(receive);
        }
    }
    #[gpui::test]
    fn a_cloud_send_joins_the_chat_at_once_and_returns_to_the_draft_on_failure(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            // A cloud conversation's selected chat is its session.
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
            c.input.update(cx, |input, cx| input.set_text("Look at this", cx));
            assert!(c.send_cloud_task("Look at this".into(), false, cx));
            // Before any upload or admission completes.
            assert_eq!(c.input.read(cx).text(), "", "the draft clears at once");
            let echoed = |cx: &App| {
                state
                    .read(cx)
                    .pending_echoes()
                    .iter()
                    .any(|entry| matches!(&entry.parts[..], [zuse_model::MessagePart::Text { text, .. }] if text == "Look at this"))
            };
            assert!(echoed(cx), "the message is in the chat at once");
            // Delivery fails: it leaves the chat and returns to the draft.
            c.cloud.task = None;
            c.return_native_send(cx);
            assert!(!echoed(cx));
            assert_eq!(c.input.read(cx).text(), "Look at this");
            assert!(c.cloud.pending_native.is_some(), "Retry keeps the same identity");
        });
        drop(receive);
    }
    /// A composer for a cloud chat whose server takes queued messages.
    fn cloud_composer(
        cx: &mut gpui::TestAppContext,
    ) -> (
        Entity<crate::state::AppState>,
        Entity<Composer>,
        Box<dyn std::any::Any>,
    ) {
        let (send, receive) = tokio::sync::mpsc::channel(8);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
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
                json!({"agent":"codex","model":"gpt-6.1-sol","runtimeMode":"full-access","queueRunSelectionVersion":1}),
                cx,
            );
        });
        (state, composer, Box::new(receive))
    }
    #[gpui::test]
    fn a_queued_cloud_message_is_in_the_queue_at_once_and_stays_when_not_sent(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (state, composer, receive) = cloud_composer(cx);
        composer.update(cx, |c, cx| {
            c.input
                .update(cx, |input, cx| input.set_text("and also this", cx));
            assert!(c.send_cloud_task("and also this".into(), true, cx));
            let id = c.cloud.pending_native.as_ref().unwrap()["commandId"]
                .as_str()
                .unwrap()
                .to_owned();
            let row = |cx: &App| {
                let state = state.read(cx);
                let texts: Vec<_> = state.queue.iter().map(|r| r.text.clone()).collect();
                (texts, state.optimistic_queued(&id).map(|r| r.state.clone()))
            };
            // Before the server answers: in the queue, out of the draft.
            assert_eq!(c.input.read(cx).text(), "");
            assert_eq!(
                row(cx),
                (
                    vec!["and also this".to_owned()],
                    Some(crate::state::OptimisticState::Sending)
                )
            );
            // Not confirmed: the row stays, marked not sent; the draft is untouched.
            c.cloud.task = None;
            c.keep_unsent_native_send(cx);
            assert!(matches!(
                row(cx).1,
                Some(crate::state::OptimisticState::Failed(_))
            ));
            assert_eq!(row(cx).0, vec!["and also this".to_owned()]);
            assert_eq!(c.input.read(cx).text(), "");
            // Retry sends the same message again and keeps a newer draft.
            c.input
                .update(cx, |input, cx| input.set_text("a newer draft", cx));
            c.retry_unsent_cloud_queued(&id, cx);
            assert_eq!(row(cx).1, Some(crate::state::OptimisticState::Sending));
            assert_eq!(
                c.cloud.pending_native.as_ref().unwrap()["commandId"],
                id.as_str()
            );
            assert_eq!(c.input.read(cx).text(), "a newer draft");
            // Not sent again, then removed: back into an empty draft.
            c.cloud.task = None;
            c.keep_unsent_native_send(cx);
            c.input.update(cx, |input, cx| input.set_text("", cx));
            c.discard_unsent_cloud_queued(&id, cx);
            assert!(state.read(cx).queue.is_empty());
            assert!(c.cloud.pending_native.is_none());
            assert_eq!(c.input.read(cx).text(), "and also this");
        });
        drop(receive);
    }
    #[gpui::test]
    fn an_unconfirmed_cloud_send_stays_in_the_chat_with_retry_and_edit(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (state, composer, receive) = cloud_composer(cx);
        composer.update(cx, |c, cx| {
            c.input.update(cx, |input, cx| input.set_text("Look at this", cx));
            assert!(c.send_cloud_task("Look at this".into(), false, cx));
            let echoed = |cx: &App| {
                state.read(cx).pending_echoes().iter().filter(|entry| matches!(&entry.parts[..], [zuse_model::MessagePart::Text { text, .. }] if text == "Look at this")).count()
            };
            assert_eq!(echoed(cx), 1);
            c.cloud.task = None;
            c.keep_unsent_native_send(cx);
            assert_eq!(echoed(cx), 1, "it keeps its place in the chat");
            assert!(c.cloud_send_unsent());
            assert_eq!(c.input.read(cx).text(), "");
            // Retry: the same message, still shown once.
            c.retry_unsent_cloud_send(cx);
            assert_eq!(echoed(cx), 1);
            assert!(!c.cloud_send_unsent() || c.cloud.task.is_some());
            // Not sent again; Edit takes it back into the draft.
            c.cloud.task = None;
            c.keep_unsent_native_send(cx);
            c.edit_unsent_cloud_send(cx);
            assert_eq!(echoed(cx), 0);
            assert_eq!(c.input.read(cx).text(), "Look at this");
            assert!(c.cloud.pending_native.is_none());
        });
        drop(receive);
    }
    #[test]
    fn a_cloud_message_sends_queues_or_steers_like_a_local_one() {
        use super::{CloudSendKind as K, cloud_send_kind as kind};
        assert_eq!(kind(false, false, false, true), K::Run);
        assert_eq!(
            kind(false, true, false, true),
            K::Run,
            "idle: a steer just sends"
        );
        assert_eq!(kind(true, false, false, true), K::Queue);
        assert_eq!(kind(true, true, false, true), K::Steer);
        assert_eq!(
            kind(true, true, true, true),
            K::Queue,
            "files wait in the queue"
        );
        assert_eq!(
            kind(true, true, false, false),
            K::Queue,
            "a turn-boundary agent reads it after the turn"
        );
    }
    #[gpui::test]
    fn a_sleeping_cloud_task_takes_a_new_access_mode_into_its_next_message(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            // Selected, but the configuration lives on the sleeping computer.
            state.selected_chat = Some("session".into());
            state
        });
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        composer.update(cx, |c, cx| {
            assert!(c.state.read(cx).configuration_blocked());
            let target = crate::state::CloudTarget {
                controller,
                subject: "owner".into(),
                organization: None,
                workspace: "task".into(),
                initial_session: "session".into(),
            };
            c.set_cloud_task(
                target,
                json!({"agent":"codex","model":"gpt-6.1-sol","runtimeMode":"auto"}),
                cx,
            );
            c.set_cloud_task_sleeping(true);
            assert!(c.set_cloud_task_permission(SandboxLevel::DangerFullAccess, cx));
            assert_eq!(
                c.pickers.read(cx).cloud_task_choice().unwrap().sandbox,
                SandboxLevel::DangerFullAccess
            );
            assert!(c.send_cloud_task("Go ahead".into(), false, cx));
            // While that message is unconfirmed, access stays as it was sent.
            assert!(c.set_cloud_task_permission(SandboxLevel::WorkspaceWrite, cx));
            assert_eq!(
                c.pickers.read(cx).cloud_task_choice().unwrap().sandbox,
                SandboxLevel::DangerFullAccess
            );
            let request = &c.cloud.pending_native.as_ref().unwrap()["action"]["payload"]["request"];
            assert_eq!(request["sandbox"], json!(SandboxLevel::DangerFullAccess));
            assert_eq!(request["autoApprove"], true);
            c.cloud.task = None;
        });
        drop(receive);
    }
    #[gpui::test]
    fn a_sleeping_cloud_task_can_send_before_its_engine_configuration_loads(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            // The cloud chat is selected, but its configuration lives on the
            // sleeping computer's engine and has not been confirmed.
            state.selected_chat = Some("session".into());
            state
        });
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        composer.update(cx, |c, cx| {
            assert!(c.state.read(cx).configuration_blocked());
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
            c.set_cloud_task_sleeping(true);
            assert!(
                !c.send_blocked(cx),
                "the task's own saved settings are enough; Send wakes the computer"
            );
            assert!(c.send_cloud_task("Wake up and continue".into(), false, cx));
            let request = c.cloud.pending_native.as_ref().expect("command built");
            assert_eq!(
                request["action"]["payload"]["request"]["prompt"],
                "Wake up and continue"
            );
            assert_eq!(request["wake"], "explicit-resume");
            c.cloud.task = None;
        });
        drop(receive);
    }
    #[gpui::test]
    fn a_cloud_task_opened_without_saved_settings_waits_for_them_before_send(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
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
            // A sidebar row carries no agent or model.
            c.set_cloud_task(target, json!({"workspaceId":"task"}), cx);
            assert!(
                c.send_blocked(cx),
                "Send waits for the task's saved choices instead of failing"
            );
        });
        drop(receive);
    }
    #[gpui::test]
    fn new_cloud_request_uses_selected_provider_and_its_model_before_dispatch(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
            state.set_test_engine(controller);
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx,|composer,cx| {
            let context=json!({"accountId":"fixture-user","ownerId":"fixture-user","organizationId":null,"commandAdmissionVersion":2,"computerCapabilities":{"providers":[{"id":"e2b","configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":"default","label":"Standard","vcpus":2,"memoryMib":2048,"diskGib":8}],"unavailableReason":null}]},"providerAgentCatalogs":{"e2b":{"providers":[{"id":"codex","name":"Codex","defaultModel":"e2b-model","models":[{"id":"e2b-model","label":"E2B model"}]}]}},"agentConnections":[{"agent":"codex","state":"connected","connectionId":"codex-selected"}]});
            composer.pickers.update(cx,|p,cx|p.seed_cloud_creation_for_test(context,json!({"projectId":"repository","accountId":"fixture-user","providerId":"boxd","defaultBranch":"main","canCreate":true}),cx));
            assert!(composer.send_cloud("First E2B task".into(),cx));
            let request=composer.cloud.request.as_ref().expect("actual production send constructed durable request");assert_eq!(request["providerId"],"e2b");assert_eq!(request["projectId"],"repository");assert_eq!(request["model"],"e2b-model");assert_eq!(request["agentConnectionId"],"codex-selected");assert_eq!(request["initialMessageDelivery"],"server-admitted-v2");assert_eq!(request["sizeId"],"default","actual advertised E2B size must be explicit in the request");
            assert!(composer.cloud_creation_activity(cx).unwrap().1,"actual initial Send must animate existing creation tray before create returns");
            let pending=request.clone();
            composer.cloud_failure("Delivery interrupted".into(),cx);
            assert!(!composer.cloud_creation_activity(cx).unwrap().1,"failure stops spinner");
            assert_eq!(composer.cloud.request.as_ref(),Some(&pending),"uncertain failure retains exact request identity and prompt");
        });
        drop(receive);
    }
    /// Reasoning and fast mode picked in a cloud model's card ride the
    /// create request, not just the card.
    #[gpui::test]
    fn new_cloud_request_carries_card_reasoning_and_fast_mode(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx)
        });
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
            state.set_test_engine(controller);
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            composer.pickers.update(cx, |p, cx| {
                p.seed_cloud_creation_for_test(
                    crate::pickers::configurable_cloud_context_for_test(),
                    json!({"projectId":"repository","accountId":"fixture-user","providerId":"boxd","defaultBranch":"main","canCreate":true,"supportsModelConfiguration":true}),
                    cx,
                );
                p.set_card_settings_for_test(HarnessId::Codex, "gpt-sol", 3, true, cx);
            });
            assert!(composer.send_cloud("Use the configured model".into(), cx));
            let request = composer.cloud.request.as_ref().unwrap();
            assert_eq!(request["model"], "gpt-sol");
            assert_eq!(request["reasoning"], "xhigh");
            assert_eq!(request["modelOptions"]["serviceTier"], "fast");
        });
        drop(receive);
    }
    /// Enter on a new cloud thread whose send is refused says why inline
    /// instead of doing nothing, and keeps the draft.
    #[gpui::test]
    fn a_refused_new_cloud_send_explains_itself(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
            state.set_test_engine(controller);
            state.cloud_project = Some(json!({"projectId":"repository","accountId":"fixture-user","providerId":"boat","defaultBranch":"main","canCreate":true}));
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            // The cloud catalog has not arrived: the model tray stays quiet.
            assert!(composer.send_blocked(cx));
            composer
                .input
                .update(cx, |input, cx| input.set_text("Start the task", cx));
            composer.on_submit(cx);
            assert!(composer.cloud.request.is_none());
            assert_eq!(composer.input.read(cx).text(), "Start the task");
            assert!(
                composer
                    .failure
                    .as_deref()
                    .is_some_and(|failure| failure.contains("still loading")),
                "{:?}",
                composer.failure
            );
            // A request already in flight explains why another can't start.
            composer.failure = None;
            composer.seed_pending_cloud_creation_for_test(json!({"projectId":"repository"}));
            composer.on_submit(cx);
            assert!(
                composer
                    .failure
                    .as_deref()
                    .is_some_and(|failure| failure.contains("still starting a cloud thread"))
            );
        });
        drop(receive);
    }
    #[gpui::test]
    fn new_cloud_request_recomputes_boat_and_e2b_sizes_without_account_reselection(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, receive) = tokio::sync::mpsc::channel(1);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
            state.set_test_engine(controller);
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx, |composer, cx| {
            let mut caps=vec![];
            let mut catalogs=serde_json::Map::new();
            for (provider,size,memory) in [("boat","small",4096),("e2b","default",2048)] {
                caps.push(json!({"id":provider,"configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":provider=="e2b","supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":size,"label":"Standard","vcpus":2,"memoryMib":memory,"diskGib":8}],"unavailableReason":null}));
                catalogs.insert(provider.into(),json!({"providers":[{"id":"codex","name":"Codex","defaultModel":format!("{provider}-model"),"models":[{"id":format!("{provider}-model"),"label":provider}]}]}));
            }
            let context=json!({"accountId":"fixture-user","ownerId":"fixture-user","organizationId":null,"commandAdmissionVersion":2,"computerCapabilities":{"providers":caps},"providerAgentCatalogs":catalogs,"agentConnections":[{"agent":"codex","state":"connected","connectionId":"same-codex-account"}]});
            for (provider,size) in [("boat","small"),("e2b","default"),("boat","small")] {
                // Model a new draft on this same composer, retaining its account
                // and provider catalogs while the server's default provider
                // changes. The project's remembered provider is only a hint.
                composer.cloud.task=None;
                composer.cloud.request=None;
                let mut context=context.clone();
                context["computerCapabilities"]["defaultProvider"]=json!(provider);
                composer.pickers.update(cx,|p,cx|{p.finish_cloud_launch(cx);p.seed_cloud_creation_for_test(context.clone(),json!({"projectId":"repository","accountId":"fixture-user","providerId":provider,"sizeId":"small","defaultBranch":"main","canCreate":true}),cx)});
                assert!(composer.send_cloud("First task on selected computer".into(),cx));
                let request=composer.cloud.request.as_ref().expect("actual Send must construct request");
                assert_eq!(request["providerId"],provider);
                assert_eq!(request["sizeId"],size,"the selected provider's advertised size must be recomputed");
                assert_eq!(request["agentConnectionId"],"same-codex-account");
                assert_eq!(request["model"],format!("{provider}-model"));
            }
            composer.cloud.task=None;
            composer.cloud.request=None;
            let mut unavailable=context.clone();
            unavailable["computerCapabilities"]["providers"]=json!([{"id":"boat","configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":false,"supportsSnapshots":true,"supportsDocker":false,"sizes":[],"unavailableReason":null}]);
            composer.pickers.update(cx,|p,cx|{p.finish_cloud_launch(cx);p.seed_cloud_creation_for_test(unavailable,json!({"projectId":"repository","accountId":"fixture-user","providerId":"boat","defaultBranch":"main","canCreate":true}),cx)});
            composer.input.update(cx,|input,cx|input.set_text("Keep this draft",cx));
            assert!(composer.send_cloud("Keep this draft".into(),cx));
            assert!(composer.cloud.request.is_none());
            assert_eq!(composer.input.read(cx).text(),"Keep this draft");
            assert!(composer.failure.as_deref().unwrap().contains("not queued"));
        });
        drop(receive);
    }
    #[gpui::test]
    fn definitive_create_rejection_restores_draft_but_uncertain_failure_retains_identity(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.cloud_project = Some(json!({"projectId":"project"}));
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx,|composer,cx| {
            let request=json!({"projectId":"project","firstMessage":"Original first message","idempotencyKey":"original-request"});
            composer.cloud.request=Some(request.clone());
            composer.cloud.staged=vec![StagedAttachment::from_file("notes.txt",b"retained bytes".to_vec())];
            composer.cloud_create_failure("This staging account has reached its computer limit. Your new message has not been queued.".into(),cx);
            assert!(composer.cloud.request.is_none());
            assert_eq!(composer.input.read(cx).text(),"Original first message");
            assert_eq!(composer.staged().len(),1);
            assert!(composer.failure.as_ref().unwrap().contains("Draft saved · not queued"));
            composer.cloud.request=Some(request);
            composer.cloud_create_failure("The workspace is busy. Wait for its current operation to finish.".into(),cx);
            assert_eq!(composer.cloud.request.as_ref().unwrap()["idempotencyKey"],"original-request");
            assert_eq!(composer.input.read(cx).text(),"Original first message");
        });
    }

    #[gpui::test]
    fn changing_project_never_retries_a_different_projects_retained_request(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.cloud_project = Some(json!({"projectId":"new-project","accountId":"owner"}));
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        composer.update(cx,|composer,cx| {
            composer.cloud.request=Some(json!({"projectId":"old-project","firstMessage":"Original retained message","idempotencyKey":"original-request"}));
            composer.input.update(cx,|input,cx|input.set_text("New project message",cx));
            assert!(composer.send_cloud("New project message".into(),cx));
            assert_eq!(composer.cloud.request.as_ref().unwrap()["idempotencyKey"],"original-request");
            assert_eq!(composer.cloud.request.as_ref().unwrap()["projectId"],"old-project");
            assert_eq!(composer.input.read(cx).text(),"New project message");
            assert!(composer.cloud.task.is_none());
            assert!(composer.failure.as_ref().unwrap().contains("another project"));
        });
    }

    #[gpui::test]
    fn cloud_launch_does_not_block_a_selected_side_chat(cx: &mut gpui::TestAppContext) {
        let parent = cx.new(|_| crate::state::AppState::new());
        let chat = serde_json::from_value(json!({
            "id":"local-chat", "deviceId":"local",
            "createdAt":"2026-01-01T00:00:00Z", "archived":false
        }))
        .unwrap();
        let state = cx.new(|cx| crate::state::AppState::side_chat_state(&parent, chat, true, cx));
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        composer.update(cx, |composer, cx| {
            composer.cloud.request = Some(json!({"firstMessage":"Pinned Cloud prompt"}));
            assert!(!composer.cloud_launch_pending(cx));
            assert!(!composer.send_blocked(cx));
        });
        state.update(cx, |state, _| state.selected_chat = None);
        composer.update(cx, |composer, cx| {
            assert!(composer.cloud_launch_pending(cx));
            assert!(composer.send_blocked(cx));
        });
    }

    #[gpui::test]
    fn cached_cloud_submit_resumes_without_consuming_the_draft(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.selected_chat = Some("retained-session".into());
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        let actions = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let captured = actions.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&composer, move |_, event, _| {
                if let ComposerEvent::ResumeCloud { text, send } = event {
                    captured.borrow_mut().push((text.clone(), *send));
                }
            })
        });
        composer.update(cx, |composer, cx| {
            composer.set_cached_cloud(true, cx);
            composer.input.update(cx, |input, cx| {
                input.set_text("Continue the retained conversation", cx)
            });
            composer.on_submit(cx);
            assert_eq!(
                composer.input.read(cx).text(),
                "Continue the retained conversation"
            );
            assert!(composer.failure.is_none());
        });
        assert_eq!(
            &*actions.borrow(),
            &[("Continue the retained conversation".to_owned(), true)]
        );
    }

    #[test]
    fn a_messages_computer_files_go_in_its_run_or_its_queued_input() {
        let mut run = json!({"action":{"type":"submit","payload":{"request":{"attachments":[]}}}});
        assert!(!send_upload::delivered(&run, 1));
        set_computer_files(&mut run, vec!["/u/a.png".into()]);
        assert_eq!(
            run["action"]["payload"]["request"]["attachments"],
            json!(["/u/a.png"])
        );
        assert!(send_upload::delivered(&run, 1));
        let mut queued = json!({"action":{"type":"queue","input":{"text":"later"}}});
        assert!(!send_upload::delivered(&queued, 1));
        set_computer_files(&mut queued, vec!["/u/a.png".into(), "/u/b.txt".into()]);
        assert_eq!(
            queued["action"]["input"]["computerFiles"],
            json!(["/u/a.png", "/u/b.txt"])
        );
        assert!(queued["action"]["input"].get("attachments").is_none());
        assert!(send_upload::delivered(&queued, 2));
    }
    #[gpui::test]
    fn a_queued_message_with_files_wakes_the_computer_instead_of_uploading_to_the_server(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, mut receive) = tokio::sync::mpsc::channel(8);
        let (_replies, replies) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, replies));
        let state = cx.new(|_| {
            let mut state = crate::state::AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            state.selected_chat = Some("session".into());
            state
        });
        let composer = cx.new(|cx| Composer::new(state, cx));
        let wakes = std::rc::Rc::new(std::cell::Cell::new(0));
        let woke = wakes.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&composer, move |_, event, _| {
                if matches!(event, ComposerEvent::CloudWakeRequested) {
                    woke.set(woke.get() + 1);
                }
            })
        });
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
                json!({"agent":"codex","model":"gpt-6.1-sol","sandbox":"workspace-write",
                    "queueRunSelectionVersion":zuse_protocol::cloud_task_commands::QUEUE_RUN_SELECTION_VERSION}),
                cx,
            );
            c.set_cloud_task_sleeping(true);
            c.input
                .update(cx, |input, cx| input.set_text("Use this after the turn", cx));
            c.attachments.insert(
                c.current_key.clone(),
                vec![StagedAttachment::from_file("shot.png", b"png bytes".to_vec())],
            );
            assert!(c.send_cloud_task("Use this after the turn".into(), true, cx));
            assert_eq!(c.input.read(cx).text(), "", "the draft clears at once");
            assert!(c.staged().is_empty());
            let request = c.cloud.pending_native.as_ref().unwrap();
            assert_eq!(request["action"]["type"], "queue");
            assert_eq!(request["wake"], "preserve-sleep");
        });
        // The send asks for the computer and waits for its connection.
        cx.run_until_parked();
        assert_eq!(wakes.get(), 1);
        assert!(
            receive.try_recv().is_err(),
            "nothing goes to server asset storage while the computer wakes"
        );
        composer.update(cx, |c, cx| {
            let request = c.cloud.pending_native.as_ref().unwrap();
            assert!(request["action"]["input"].get("attachments").is_none());
            assert!(request["action"]["input"].get("computerFiles").is_none());
            // The computer never connects: the message and its file return.
            c.cloud.task = None;
            c.return_native_send(cx);
            assert_eq!(c.input.read(cx).text(), "Use this after the turn");
            assert_eq!(c.staged().len(), 1);
            assert!(
                c.cloud.pending_native.is_some(),
                "Retry keeps the same identity"
            );
            // Retry leaves the draft again and keeps the same queued message.
            let queue_id = c.cloud.pending_native.as_ref().unwrap()["action"]["queueId"].clone();
            assert!(c.send_cloud_task("Use this after the turn".into(), true, cx));
            assert_eq!(c.input.read(cx).text(), "");
            assert_eq!(
                c.cloud.pending_native.as_ref().unwrap()["action"]["queueId"],
                queue_id
            );
            c.cloud.task = None;
        });
    }
}
