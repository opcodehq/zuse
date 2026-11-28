//! A cloud conversation occupies the current native conversation surface.
//! Account history/admission and execution readiness have separate lifetimes.
use super::*;
use crate::state::CloudTarget;
use serde_json::{Value, json};
mod activity;
mod checkout_sync;
mod computer;
mod environment_card;
mod monitor;
mod open_menu;
mod reconnect;

pub(super) use computer::Tone;
mod history;
mod load;
pub(super) use load::{MachineLoad, machine_load};

pub(super) enum CloudConversationEvent {
    Desktop,
    Terminal,
    Files,
    Skills,
    OpenFile(String),
    /// Open a folder on this computer (the synced copy) in an app.
    OpenPathIn(super::open_in::OpenTarget, String),
    /// Detect this computer's apps again (the Computer menu opened).
    RefreshOpenTargets,
    /// The composer's background-work tray asked for a running subagent.
    OpenSubagent {
        chat_id: String,
        doc_id: String,
        title: String,
    },
}
/// The header cloud glyph's color: the theme's success green while the
/// computer is online, muted grey otherwise.
pub(super) fn header_status_color(online: bool, theme: &Theme) -> gpui::Hsla {
    if online {
        theme.success
    } else {
        theme.text_muted.opacity(0.6)
    }
}

/// The agent a cloud chat's saved config names. Account rows may use the
/// short `claude` id for the agent local chats call `claude-code`.
pub(super) fn cloud_harness(config: &Value) -> Option<zuse_protocol::HarnessId> {
    match config["agent"].as_str()? {
        "" => None,
        "claude" => Some(zuse_protocol::HarnessId::ClaudeCode),
        other => zuse_protocol::HarnessId::try_from(other.to_owned()).ok(),
    }
}

/// The agent a cloud tab shows, from the best source on hand: what the chat
/// itself has learned, then the sidebar's catalog row for its workspace, then
/// the agent this device last saw the workspace run. `None` (the plain chat
/// icon) only when no source names an agent yet.
pub(super) fn resolve_cloud_harness(
    learned: Option<zuse_protocol::HarnessId>,
    catalog: Option<&Value>,
    remembered: Option<zuse_protocol::HarnessId>,
) -> Option<zuse_protocol::HarnessId> {
    learned
        .or_else(|| catalog.and_then(cloud_harness))
        .or(remembered)
}

fn prompt_tab_title(prompt: &str) -> Option<String> {
    let display = zuse_protocol::file_mentions::mention_display_text(prompt);
    let first_line = display.trim().lines().next()?;
    Some(first_line.chars().take(60).collect())
}

/// Whether the agent has replied (with text) after the latest user message.
fn latest_message_answered(transcript: &[zuse_model::SessionMessageEntry]) -> bool {
    let Some(last_user) = transcript
        .iter()
        .rposition(|entry| entry.role == zuse_model::MessageRole::User)
    else {
        return false;
    };
    transcript[last_user + 1..].iter().any(|entry| {
        entry.role == zuse_model::MessageRole::Assistant
            && entry.parts.iter().any(|part| {
                matches!(part, zuse_model::MessagePart::Text { text, .. } if !text.trim().is_empty())
            })
    })
}

pub(super) struct CloudConversation {
    pub(super) target: CloudTarget,
    pub(super) state: Entity<AppState>,
    pub(super) composer: Entity<Composer>,
    transcript: Entity<Transcript>,
    pub(super) terminal: Option<Entity<TerminalPanel>>,
    boot: EngineBootConfig,
    config: Value,
    /// The agent this chat's own data has named. Only ever replaced by
    /// another named agent, so a reload never drops the tab's icon back to
    /// the plain chat glyph.
    learned_harness: Option<zuse_protocol::HarnessId>,
    /// The agent this device last saw the workspace run, read at open.
    remembered_harness: Option<zuse_protocol::HarnessId>,
    status: Value,
    admissions: Value,
    pub(super) first_message: Option<String>,
    pub(super) title: String,
    title_is_placeholder: bool,
    error: Option<String>,
    authorized: bool,
    access_revoked: bool,
    /// When this view opened; phase timings in the log are measured from it.
    pub(super) opened_at: std::time::Instant,
    history_read: Option<Task<()>>,
    history: history::State,
    read_again: bool,
    environment: Option<Task<()>>,
    account: Option<Task<()>>,
    events: Option<Task<()>>,
    execution: Option<Task<()>>,
    /// Failed attempts to connect the computer's tools, retried quietly.
    reconnect: reconnect::Backoff,
    /// The wait before the next attempt.
    reconnect_wait: Option<Task<()>>,
    /// Shows the strip once the failure has lasted.
    reconnect_notice: Option<Task<()>>,
    /// The attempt in flight was asked for with Reconnect.
    reconnect_manual: bool,
    operation: Option<Task<()>>,
    sleep_command: Option<String>,
    /// A message's files asked for a wake while a sleep was still running.
    wake_after_sleep: bool,
    /// The state's wake requests (see `AppState::request_cloud_wake`) this
    /// view has answered.
    answered_wake_requests: u64,
    /// A file tab asked for a wake before the account was confirmed.
    wake_after_authorize: bool,
    computer: computer::State,
    /// The computer's conversations the server has registered, including
    /// ones its computer has not installed yet.
    registered: Vec<zuse_protocol::Chat>,
    /// This conversation was opened before the server finished recording it.
    pub(super) registering: bool,
    /// The request that records it, retried until the server confirms.
    registration: Option<(Value, Task<()>)>,
    _subscriptions: Vec<Subscription>,
}
impl gpui::EventEmitter<CloudConversationEvent> for CloudConversation {}

/// The transcripts of recently closed cloud conversations, so reopening one
/// shows its messages immediately instead of an empty chat while history
/// loads. Keyed by account and conversation; display only, never authority.
#[derive(Default)]
struct TranscriptCache {
    entries: std::collections::HashMap<String, Vec<zuse_model::SessionMessageEntry>>,
    order: std::collections::VecDeque<String>,
}
impl gpui::Global for TranscriptCache {}
impl TranscriptCache {
    const CAPACITY: usize = 24;
    fn key(target: &CloudTarget) -> String {
        format!(
            "{}\u{1f}{}\u{1f}{}\u{1f}{}",
            target.subject,
            target.organization.as_deref().unwrap_or_default(),
            target.workspace,
            target.initial_session
        )
    }
    fn get(&self, key: &str) -> Option<Vec<zuse_model::SessionMessageEntry>> {
        self.entries.get(key).cloned()
    }
    fn put(&mut self, key: String, entries: Vec<zuse_model::SessionMessageEntry>) {
        self.order.retain(|existing| existing != &key);
        self.order.push_back(key.clone());
        self.entries.insert(key, entries);
        while self.order.len() > Self::CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}
impl CloudConversation {
    pub(super) fn new(
        target: CloudTarget,
        row: Value,
        launch: Value,
        boot: EngineBootConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = target.initial_session.clone();
        let cache_key = TranscriptCache::key(&target);
        let cached = cx.default_global::<TranscriptCache>().get(&cache_key);
        if let Some(entries) = &cached {
            tracing::info!(workspace = %target.workspace, ms = 0u64, hit = true, source = "memory",
                messages = entries.len(), "Cloud chat open: cached history read");
        }
        let state = cx.new(|_| {
            let mut state = AppState::new();
            // Account-owned rendering is available before a guest connects.
            state.connection = ConnectionStatus::Ready;
            state.hosted_on_cloud_computer = true;
            state.selected_chat = Some(session);
            state.auto_selected = true;
            // The last transcript this app showed for the chat paints at
            // once; the history read replaces it, like any saved page.
            if let Some(entries) = cached {
                state.apply_transcript(entries);
            }
            state
        });
        let transcript = cx.new(|cx| Transcript::new(state.clone(), cx));
        let composer = cx.new(|cx| Composer::new(state.clone(), cx));
        let mut config = row;
        if let (Some(config), Some(launch)) = (config.as_object_mut(), launch.as_object()) {
            for key in ["agent", "model", "reasoning", "modelOptions", "runtimeMode"] {
                if let Some(value) = launch.get(key) {
                    config.insert(key.into(), value.clone());
                }
            }
        }
        composer.update(cx, |composer, cx| {
            composer.set_cloud_task(target.clone(), config.clone(), cx);
            composer.focus_pending = true;
        });
        let subscriptions = vec![
            cx.on_release({
                let state = state.clone();
                move |_: &mut Self, cx: &mut App| {
                    let entries = state.read(cx).transcript.clone();
                    if !entries.is_empty() {
                        cx.default_global::<TranscriptCache>()
                            .put(cache_key, entries);
                    }
                }
            }),
            cx.subscribe(&composer, |this: &mut Self, _, event, cx| match event {
                ComposerEvent::CloudLaunchChanged => this.refresh_history(cx),
                ComposerEvent::CloudForegroundAdmitted => this.foreground_admitted(cx),
                ComposerEvent::CloudStatusRetry => this.retry_failed_computer(cx),
                ComposerEvent::CloudStatusDelete => this.delete_failed_computer(cx),
                ComposerEvent::CloudSleepRequested => this.sleep(cx),
                ComposerEvent::CloudWakeRequested => this.wake_for_upload(cx),
                ComposerEvent::CloudComputerRecreating { recreation } => {
                    this.computer_recreation_started(recreation.clone(), cx)
                }
                ComposerEvent::Sent {
                    chat_id,
                    message_id,
                } => this.transcript.update(cx, |transcript, cx| {
                    transcript.on_own_send(chat_id.clone(), message_id.clone(), cx)
                }),
                ComposerEvent::Queued {
                    chat_id,
                    message_id,
                } => this.transcript.update(cx, |transcript, cx| {
                    transcript.on_own_queued_send(chat_id.clone(), message_id.clone(), cx)
                }),
                ComposerEvent::WorkspaceCommand(crate::composer::WorkspaceCommand::Terminal) => {
                    cx.emit(CloudConversationEvent::Terminal)
                }
                ComposerEvent::WorkspaceCommand(crate::composer::WorkspaceCommand::Files) => {
                    cx.emit(CloudConversationEvent::Files)
                }
                ComposerEvent::WorkspaceCommand(crate::composer::WorkspaceCommand::Skills) => {
                    cx.emit(CloudConversationEvent::Skills)
                }
                ComposerEvent::OpenFile(path) => {
                    cx.emit(CloudConversationEvent::OpenFile(path.to_string()))
                }
                ComposerEvent::OpenSubagent {
                    chat_id,
                    doc_id,
                    title,
                } => cx.emit(CloudConversationEvent::OpenSubagent {
                    chat_id: chat_id.clone(),
                    doc_id: doc_id.clone(),
                    title: title.clone(),
                }),
                _ => {}
            }),
            cx.subscribe(&transcript, |this: &mut Self, _, event, cx| match event {
                crate::transcript::TranscriptEvent::RetryNotSent => {
                    this.composer.update(cx, |composer, cx| {
                        composer.retry_cloud_messages_not_sent(cx)
                    });
                }
                crate::transcript::TranscriptEvent::RemoveNotSent => {
                    this.composer.update(cx, |composer, cx| {
                        composer.remove_cloud_message_not_sent(cx)
                    });
                }
                _ => {}
            }),
            cx.observe(&composer, |_: &mut Self, _, cx| cx.notify()),
            cx.observe(&state, |this: &mut Self, state, cx| {
                if !this.access_revoked
                    && let Some(title) = this.live_tab_title(state.read(cx))
                {
                    this.title = zuse_protocol::file_mentions::mention_display_text(title);
                    this.title_is_placeholder = false;
                }
                this.publish_computer_online(cx);
                // The tools' connection ended for good (the computer's
                // runtime updated in place, so its identity changed): open
                // a new one rather than leave the tools disconnected.
                if this.authorized
                    && this.execution.is_none()
                    && state
                        .read(cx)
                        .engine()
                        .is_some_and(|engine| engine.connection_stopped())
                {
                    this.detach_execution(cx);
                    this.retry_execution(cx);
                }
                // A file tab that cannot read its file yet asks for the
                // computer.
                let requests = state.read(cx).cloud_wake_requests;
                if requests != this.answered_wake_requests {
                    this.answered_wake_requests = requests;
                    this.wake_for_file(cx);
                }
                this.history.observe_live(state.read(cx));
                // New transcript or admissions reconcile the first message's
                // stand-in and the working lines right away.
                this.sync_progress(cx);
                let transcript = state.read(cx).transcript.clone();
                this.composer.update(cx, |composer, cx| {
                    composer.apply_cloud_response_activity(&transcript, cx)
                });
                cx.notify();
            }),
        ];
        let first_message = launch["firstMessage"].as_str().map(str::to_owned);
        // A conversation added beside the computer's first one starts
        // untitled, like a new chat tab in a local thread.
        let added = config["initialSessionId"]
            .as_str()
            .is_some_and(|first| first != target.initial_session);
        let title = config["title"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .map(zuse_protocol::file_mentions::mention_display_text)
            .or_else(|| first_message.as_deref().and_then(prompt_tab_title));
        let title_is_placeholder = title.is_none();
        let title = title.unwrap_or_else(|| if added { "New chat" } else { "Cloud chat" }.into());
        let initial_metadata = config.clone();
        let learned_harness = cloud_harness(&config);
        let remembered_harness = crate::settings::remembered_cloud_agent(&target.workspace, cx);
        if let Some(agent) = learned_harness {
            crate::settings::remember_cloud_agent(&target.workspace, agent, cx);
        }
        let mut this = Self {
            target,
            state,
            composer,
            transcript,
            terminal: None,
            boot,
            config,
            learned_harness,
            remembered_harness,
            status: json!({"state":"connecting","message":"Opening cloud conversation…"}),
            admissions: Value::Null,
            first_message,
            title,
            title_is_placeholder,
            error: None,
            authorized: false,
            access_revoked: false,
            opened_at: std::time::Instant::now(),
            history_read: None,
            history: history::State::default(),
            read_again: false,
            environment: None,
            account: None,
            events: None,
            execution: None,
            reconnect: reconnect::Backoff::default(),
            reconnect_wait: None,
            reconnect_notice: None,
            reconnect_manual: false,
            operation: None,
            sleep_command: None,
            wake_after_sleep: false,
            answered_wake_requests: 0,
            wake_after_authorize: false,
            computer: computer::State::default(),
            registered: Vec::new(),
            registering: false,
            registration: None,
            _subscriptions: subscriptions,
        };
        this.computer.metadata = initial_metadata;
        this.publish_computer_online(cx);
        this.watch_account(cx);
        // Paint the saved copy now; the fresh read follows the account check.
        this.read_provisional_history(cx);
        this
    }
    /// Route the transcript's links through the shell: file links open on
    /// this computer's own files, web links keep their normal handling.
    pub(super) fn set_link_handler(
        &mut self,
        links: crate::markdown::render::LinkUi,
        cx: &mut Context<Self>,
    ) {
        self.transcript.update(cx, |transcript, _| {
            transcript.set_workspace_link_handler(links)
        });
    }
    #[cfg(test)]
    pub(super) fn set_computer_metadata_for_test(&mut self, metadata: Value) {
        self.computer.metadata = metadata;
    }
    #[cfg(test)]
    pub(super) fn set_admissions_for_test(&mut self, rows: Value) {
        self.admissions = rows;
    }
    #[cfg(any(test, feature = "image-links-fixture"))]
    pub(super) fn transcript_for_test(&self) -> &Entity<Transcript> {
        &self.transcript
    }
    #[cfg(test)]
    fn activity(&self, cx: &App) -> Option<activity::Activity> {
        self.computer_activity(cx)
            .or_else(|| self.agent_activity(cx))
    }
    /// The cloud computer's own stage (starting, cloning, environment).
    fn computer_activity(&self, cx: &App) -> Option<activity::Activity> {
        if self.access_revoked {
            return None;
        }
        let state = self.state.read(cx);
        let launch = self.admissions.as_array().and_then(|rows| {
            rows.iter()
                .find(|row| row["commandId"] == format!("launch:{}", self.target.workspace))
        });
        // Before the first history receipt, this newly admitted session's first
        // canonical user is its launch message. Never credit an older assistant.
        let initial_id = launch
            .and_then(|row| row["messageId"].as_str())
            .or_else(|| {
                state
                    .transcript
                    .iter()
                    .find(|entry| entry.role == zuse_model::MessageRole::User)
                    .map(|entry| entry.id.as_str())
            });
        // The computer may record the first message under its own id: a reply
        // to the transcript's first user message counts as well.
        let first_user = state
            .transcript
            .iter()
            .find(|entry| entry.role == zuse_model::MessageRole::User)
            .map(|entry| entry.id.as_str());
        let response_started = [initial_id, first_user]
            .into_iter()
            .flatten()
            .any(|id| zuse_viewport::cloud_response_started(&state.transcript, id));
        let initial = !response_started
            && (self.first_message.is_some()
                || launch.is_some_and(|row| {
                    matches!(
                        row["executionState"].as_str(),
                        Some("queued" | "accepted" | "admitted")
                    )
                }));
        activity::preparation(&self.computer.metadata, initial)
    }
    /// What this conversation reports for its chat's sidebar row and tab.
    /// `working`: a message it is working on whose turn the transcript
    /// doesn't show finished; tabs and rows spin on it, like a local chat's.
    pub(super) fn live_activity(&self, cx: &App) -> crate::state::CloudLiveActivity {
        let activity = self.agent_activity(cx);
        crate::state::CloudLiveActivity {
            working: activity.as_ref().is_some_and(|activity| activity.busy),
            awaiting_input: activity
                .as_ref()
                .is_some_and(|activity| activity.label == activity::NEEDS_INPUT),
            online: self.computer_online(),
            background: self.background_work(cx),
        }
    }
    /// Background work this conversation's agent holds beside its turn,
    /// what its Background work row lists.
    pub(super) fn background_work(&self, cx: &App) -> u32 {
        if self.access_revoked {
            return 0;
        }
        let state = self.state.read(cx);
        state.selected_chat.as_deref().map_or(0, |chat| {
            state.background_work_for(chat, chrono::Utc::now())
        })
    }
    /// What the computer's live session says about the current turn, when
    /// it is attached: the same state the transcript's "Working for" line
    /// and a local chat's controls follow.
    fn session_turn(&self, cx: &App) -> Option<activity::Activity> {
        let state = self.state.read(cx);
        state.engine()?;
        let chat = state.selected_chat.as_deref()?;
        match state.indicator_for(chat, chrono::Utc::now()) {
            crate::state::Indicator::Working => Some(activity::Activity {
                label: activity::WORKING.into(),
                next: None,
                busy: true,
            }),
            crate::state::Indicator::AwaitingInput => Some(activity::Activity {
                label: activity::NEEDS_INPUT.into(),
                next: None,
                busy: false,
            }),
            _ => None,
        }
    }
    /// The message's progress once it is with the agent. The one answer the
    /// transcript's working line, the composer's Stop and the sidebar row
    /// all follow: a send in flight, the server's command record, or the
    /// attached computer's live session, whichever knows the turn runs.
    fn agent_activity(&self, cx: &App) -> Option<activity::Activity> {
        if self.access_revoked {
            return None;
        }
        if self.composer.read(cx).cloud_messages_not_sent() {
            // The server never took the newest messages: no turn starts for
            // them, whatever an older read of its records still says. Only
            // the computer's live session can still say its agent works.
            return self.session_turn(cx);
        }
        let composer = self
            .composer
            .read(cx)
            .cloud_activity()
            .map(|(label, busy)| activity::Activity {
                label: label.into(),
                next: None,
                busy,
            });
        let command = activity::command(&self.admissions, &self.state.read(cx).transcript);
        let session = self.session_turn(cx);
        // A send in flight first, then whichever record knows the turn
        // runs: the server's command row can trail the live session (a
        // progress event not read yet) and the other way round.
        let reports = [composer, command, session];
        let busy = reports
            .iter()
            .flatten()
            .find(|activity| activity.busy)
            .cloned();
        busy.or_else(|| reports.into_iter().flatten().next())
    }
    /// The agent's turn is running: its working line shows and the composer
    /// offers Stop (or queues a message) instead of sending a new turn.
    fn turn_running(&self, cx: &App) -> bool {
        // Messages the server never took start no turn: as for the working
        // line, an older read of its command rows doesn't count then.
        let records = !self.composer.read(cx).cloud_messages_not_sent();
        !self.access_revoked
            && (records
                && activity::command(&self.admissions, &self.state.read(cx).transcript)
                    .is_some_and(|activity| activity.busy)
                || self.session_turn(cx).is_some_and(|activity| activity.busy))
    }
    /// Id of the stand-in for the launch prompt until history delivers it.
    fn launch_echo_id(&self) -> String {
        format!("launch-echo:{}", self.target.workspace)
    }
    /// Put progress where an ordinary chat shows it: the prompt as the first
    /// message right away, the computer's stage in the composer's status
    /// line, and the agent's progress in the working line under the messages.
    /// Every setter ignores unchanged values, so this is safe on each frame.
    fn sync_progress(&mut self, cx: &mut Context<Self>) {
        let pending = activity::pending_initial_prompt(
            self.first_message.as_deref(),
            &self.admissions,
            &self.state.read(cx).transcript,
        );
        let echo = self.launch_echo_id();
        let session = self.state.read(cx).selected_chat.clone();
        if let Some(session) = session {
            let shown = self
                .state
                .read(cx)
                .pending_echoes()
                .iter()
                .any(|entry| entry.id == echo);
            // The first message's stand-in is in the server's transcript now
            // (under its own id): the send anchor moves to it, then the
            // stand-in goes, so the message keeps its place with no jump.
            if pending.is_none() && shown {
                let real = self
                    .state
                    .read(cx)
                    .transcript
                    .iter()
                    .find(|entry| entry.role == zuse_model::MessageRole::User)
                    .map(|entry| entry.id.clone());
                if let Some(real) = real {
                    self.transcript.update(cx, |transcript, cx| {
                        transcript.retarget_own_send(&echo, &real, cx)
                    });
                }
            }
            let anchor = pending.is_some() && !shown;
            let pending_changed = pending.is_some() != shown;
            match (pending, shown) {
                (Some(text), false) => self.state.update(cx, |state, cx| {
                    let device = state.local_device_id.clone().unwrap_or_default();
                    state.push_echo(
                        &session,
                        zuse_model::SessionMessageEntry {
                            id: echo.clone(),
                            role: zuse_model::MessageRole::User,
                            parts: vec![zuse_model::MessagePart::Text {
                                id: format!("text:{echo}"),
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
                }),
                (None, true) => self.state.update(cx, |state, cx| {
                    state.remove_echo(&session, &echo);
                    cx.notify();
                }),
                _ => {}
            }
            if pending_changed {
                // This can run while the conversation renders, where the
                // transcript would not hear the state change: tell it, so it
                // rebuilds its rows (the stand-in never lingers).
                self.transcript.update(cx, |_, cx| cx.notify());
            }
            if anchor {
                // Like any send: the first message moves to the top and the
                // reply streams in below it.
                self.transcript.update(cx, |transcript, cx| {
                    transcript.on_own_first_send(session.clone(), echo.clone(), cx)
                });
            }
        }
        let computer = self.computer_activity(cx);
        let agent = self.agent_activity(cx);
        // Only issues (a failed stage, a message that needs checking) reach
        // the status line; startup phases are the environment card's.
        let status = activity::status_line(computer.as_ref(), agent.as_ref());
        // A computer stage that failed (the server's failed or
        // needs-attention status) offers Retry and Delete on that line.
        let failed = computer.as_ref().is_some_and(|stage| !stage.busy);
        // Once the agent answered the latest message, the computer is clearly
        // up: no waiting line.
        let answered = latest_message_answered(&self.state.read(cx).transcript);
        // A message still on its way to the server says so first: what the
        // computer was last read doing is not news about it.
        // Nothing runs before a held message is reviewed, however the
        // computer is doing.
        let waiting = self.composer.read(cx).cloud_waiting_for_connection()
            || agent
                .as_ref()
                .is_some_and(|activity| activity.label == activity::REVIEW_FIRST);
        let line = if !waiting && !answered && computer.as_ref().is_some_and(|stage| stage.busy) {
            Some(SharedString::from("Waiting for computer…"))
        } else {
            agent
                .filter(|activity| activity.busy || activity.label == activity::REVIEW_FIRST)
                .map(|activity| SharedString::from(activity.label))
        };
        let not_sent = self.composer.read(cx).cloud_messages_not_sent();
        let line = line.filter(|_| !not_sent);
        let turn_running = self.turn_running(cx);
        self.composer.update(cx, |composer, cx| {
            composer.set_cloud_status(status, cx);
            composer.set_cloud_status_actions(failed, cx);
            composer.set_cloud_turn_running(turn_running, cx);
        });
        self.transcript.update(cx, |transcript, cx| {
            transcript.set_external_activity(line, cx);
            transcript.set_external_not_sent(not_sent, cx);
        });
    }

    pub(super) fn branch_label(&self) -> Option<String> {
        if self.access_revoked {
            return None;
        }
        self.computer.metadata["branch"]
            .as_str()
            .or_else(|| self.config["branch"].as_str())
            .filter(|branch| !branch.trim().is_empty())
            .map(str::to_owned)
    }

    /// The agent this chat runs, for its tab's icon. `catalog` is the
    /// sidebar's row for this workspace, when the account list has one.
    pub(super) fn harness(&self, catalog: Option<&Value>) -> Option<zuse_protocol::HarnessId> {
        resolve_cloud_harness(self.learned_harness, catalog, self.remembered_harness)
    }

    /// Adopt the agent the chat's config now names, and remember it for the
    /// next time this workspace opens. A config without one changes nothing.
    fn learn_harness(&mut self, cx: &mut Context<Self>) {
        let Some(agent) = cloud_harness(&self.config) else {
            return;
        };
        if self.learned_harness != Some(agent) {
            self.learned_harness = Some(agent);
            cx.notify();
        }
        crate::settings::remember_cloud_agent(&self.target.workspace, agent, cx);
    }

    /// Whether the computer is up and usable right now. Asleep, paused,
    /// starting, failed, deleted or no longer this account's all read as
    /// unavailable. Follows the workspace state the history and environment
    /// streams already deliver.
    pub(super) fn computer_online(&self) -> bool {
        !self.access_revoked
            && matches!(self.computer_summary().tone, Tone::Running)
            && self.computer.metadata["runtimeState"].as_str() != Some("offline")
    }

    /// The same cloud icon placement as an ordinary conversation. Computer
    /// details never consume a second header or push the transcript down.
    /// The glyph is green while the computer is online and grey otherwise;
    /// startup progress never shows in the header.
    pub(super) fn render_header_control(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let open = self.computer.open;
        let online = self.computer_online();
        let glyph = icon(icons::DITHER_CLOUD)
            .size(px(14.0))
            .text_color(header_status_color(online, &theme));
        let mut button = header_icon_button_with(
            "native-cloud-computer",
            glyph,
            if online {
                "Cloud computer: running"
            } else {
                "Cloud computer: not available"
            },
            cx.listener(|this, _, _, cx| {
                if this.computer.menu.take_press_was_open() {
                    this.close_computer(cx);
                } else {
                    this.toggle_computer(cx);
                }
            }),
        )
        .role(gpui::Role::Button)
        .aria_label("Cloud computer")
        .aria_expanded(open)
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, _| {
                window.prevent_default();
                this.computer.menu.note_trigger_press();
            }),
        );
        if self.computer.menu.get().is_some() {
            let panel = self.render_computer(cx);
            let card = popover::popover_card(&theme)
                .id("native-cloud-computer-card")
                .w(px(320.))
                .max_h(px(560.))
                .overflow_y_scroll()
                .on_mouse_down_out(cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    // The app menu floats outside this card; a press in it
                    // isn't outside.
                    if !this
                        .computer
                        .nested_bounds
                        .is_some_and(|bounds| bounds.contains(&event.position))
                    {
                        this.close_computer(cx);
                    }
                }))
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if this.computer_menu_key(event, cx) {
                        cx.stop_propagation();
                    }
                }))
                .child(panel);
            button = button.relative().child(popover::anchored_menu_below_end(
                "native-cloud-computer-menu",
                card.into_any_element(),
                self.computer.menu.closing_since(),
            ));
        }
        match self.render_checkout_badge(cx) {
            Some(badge) => div()
                .flex()
                .items_center()
                .gap(px(2.))
                .child(badge)
                .child(button)
                .into_any_element(),
            None => button.into_any_element(),
        }
    }
    /// The workspace's thread: every conversation on this computer belongs
    /// to it, as every conversation tab of a local thread does.
    pub(super) fn thread_id(&self) -> Option<String> {
        self.computer.metadata["chatId"]
            .as_str()
            .or_else(|| self.config["chatId"].as_str())
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    }
    /// This computer's active conversations, oldest first: the computer's
    /// own thread members and those the server has registered.
    pub(super) fn conversations(&self, cx: &App) -> Vec<zuse_protocol::Chat> {
        let Some(thread) = self.thread_id() else {
            return Vec::new();
        };
        let mut rows: Vec<zuse_protocol::Chat> = Vec::new();
        for chat in self.state.read(cx).chats.iter().chain(&self.registered) {
            if chat.thread_id.as_deref() == Some(thread.as_str())
                && chat.parent_chat_id.is_none()
                && !chat.archived
                && !rows.iter().any(|row| row.id == chat.id)
            {
                rows.push(chat.clone());
            }
        }
        rows.sort_by_key(|chat| chat.created_at);
        rows
    }
    /// The chat a new conversation on this computer copies its agent and
    /// model from: this conversation's own.
    pub(super) fn template_chat(&self, cx: &App) -> Value {
        let state = self.state.read(cx);
        state
            .chats
            .iter()
            .chain(&self.registered)
            .find(|chat| chat.id == self.target.initial_session)
            .and_then(|chat| serde_json::to_value(chat).ok())
            .unwrap_or_else(|| json!({}))
    }
    /// The computer record for another conversation on this computer: this
    /// one's, without this conversation's title.
    pub(super) fn sibling_row(&self) -> Value {
        let mut row = self.config.clone();
        if let Some(row) = row.as_object_mut() {
            row.remove("title");
            row.remove("firstMessage");
        }
        row
    }
    /// Opened at once, before the server has recorded this conversation:
    /// sends and history reads wait until `request` (the `CloudRpc` that
    /// records it) is confirmed.
    pub(super) fn start_registering(
        &mut self,
        chat: zuse_protocol::Chat,
        request: Value,
        cx: &mut Context<Self>,
    ) {
        self.registering = true;
        self.remember_conversation(chat.clone());
        self.state.update(cx, |state, cx| {
            state.pending_cloud_conversation = Some(chat.clone());
            if !state.chats.iter().any(|row| row.id == chat.id) {
                state.chats.push(chat);
            }
            cx.notify();
        });
        self.composer
            .update(cx, |composer, cx| composer.set_cloud_registering(true, cx));
        self.register(request, std::time::Duration::ZERO, cx);
    }
    /// Send the request that records this conversation, after `delay`. A
    /// failure shows and is retried with the same request (so the same
    /// conversation), backing off to half a minute, while the tab is open.
    fn register(&mut self, request: Value, delay: std::time::Duration, cx: &mut Context<Self>) {
        let controller = self.target.controller.clone();
        let session = self.target.initial_session.clone();
        let started = std::time::Instant::now();
        let params = request.clone();
        let task = cx.spawn(async move |this, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let result = async {
                let mut reply = controller
                    .client()
                    .subscribe_scoped(methods::CLOUD_RPC, params)
                    .await
                    .map_err(|error| error.to_string())?;
                let frame = reply
                    .recv()
                    .await
                    .ok_or_else(|| "The new chat was not confirmed.".to_owned())?;
                if let Some(error) = frame["err"].as_str() {
                    return Err(error.to_owned());
                }
                serde_json::from_value::<zuse_protocol::Chat>(frame["ok"].clone())
                    .ok()
                    .filter(|chat| chat.id == session)
                    .map(|_| ())
                    .ok_or_else(|| "The new chat was not confirmed.".to_owned())
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                tracing::info!(workspace = %this.target.workspace,
                    ms = started.elapsed().as_millis() as u64, ok = result.is_ok(),
                    "Cloud conversation recorded");
                this.registered(result, delay, cx)
            });
        });
        self.registration = Some((request, task));
    }
    /// The server recorded this conversation, or the attempt failed.
    fn registered(
        &mut self,
        result: Result<(), String>,
        delay: std::time::Duration,
        cx: &mut Context<Self>,
    ) {
        if !self.registering {
            return;
        }
        match result {
            Ok(()) => {
                self.registering = false;
                self.registration = None;
                self.error = None;
                self.composer
                    .update(cx, |composer, cx| composer.set_cloud_registering(false, cx));
                if std::mem::take(&mut self.read_again) {
                    self.refresh_history(cx);
                }
            }
            Err(error) => {
                let message =
                    format!("This chat isn't open on its computer yet: {error} Retrying…");
                self.error = Some(message.clone());
                self.composer
                    .update(cx, |composer, cx| composer.show_error(message, cx));
                if let Some((request, _)) = self.registration.take() {
                    let next = (delay * 2).clamp(
                        std::time::Duration::from_secs(2),
                        std::time::Duration::from_secs(30),
                    );
                    self.register(request, next, cx);
                }
            }
        }
        cx.notify();
    }
    /// Learn a conversation just added to this computer before the server's
    /// list is read again.
    pub(super) fn remember_conversation(&mut self, chat: zuse_protocol::Chat) {
        if !self.registered.iter().any(|row| row.id == chat.id) {
            self.registered.push(chat);
        }
    }

    fn live_tab_title<'a>(&self, state: &'a AppState) -> Option<&'a str> {
        if !state.chats_synced || state.engine().is_none() {
            return None;
        }
        state
            .chats
            .iter()
            .find(|chat| chat.id == self.target.initial_session)?
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
    }

    pub(super) fn tab_title(&self, cx: &App) -> String {
        if self.title_is_placeholder && !self.access_revoked {
            let state = self.state.read(cx);
            if let Some(title) = state
                .transcript
                .iter()
                .chain(state.pending_echoes())
                .filter(|entry| entry.role == zuse_model::MessageRole::User)
                .flat_map(|entry| &entry.parts)
                .find_map(|part| match part {
                    zuse_model::MessagePart::Text { text, .. } => prompt_tab_title(text),
                    _ => None,
                })
            {
                return title;
            }
        }
        self.title.clone()
    }

    fn params(&self) -> Value {
        json!({"workspaceId":self.target.workspace,"sessionId":self.target.initial_session,
            "expectedAccountId":self.target.subject,"expectedOrganizationId":self.target.organization})
    }
    pub(super) fn ready(&self, cx: &App) -> bool {
        self.authorized
            && !self.domain_sleeping()
            && self.status["state"] == "ready"
            && self.computer.execution_controls_enabled()
            && self.state.read(cx).engine().is_some()
            && matches!(self.state.read(cx).connection, ConnectionStatus::Ready)
    }
    /// Authorize this chat while the signed-in account is its owner. The
    /// watch never ends on its own: a different account (or signing out)
    /// withdraws access and the owner's return restores it, and a dropped
    /// status stream (an engine restart) is resubscribed. Ending it on the
    /// first mismatch used to hide the composer until the chat was reopened.
    fn watch_account(&mut self, cx: &mut Context<Self>) {
        let target = self.target.clone();
        self.account = Some(cx.spawn(async move |this, cx| {
            loop {
                let result = target
                    .controller
                    .client()
                    .subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS, json!({}))
                    .await;
                if let Ok(mut stream) = result {
                    while let Some(value) = stream.recv().await {
                        let owner = value["user"]["id"].as_str() == Some(target.subject.as_str())
                            && value["orgId"].as_str() == target.organization.as_deref();
                        let loading = value["user"].is_null();
                        let alive = this.update(cx, |this, cx| {
                            if owner {
                                if !this.authorized {
                                    tracing::info!(workspace = %this.target.workspace, ms = this.opened_at.elapsed().as_millis() as u64, "Cloud chat open: account confirmed");
                                    this.authorized = true;
                                    this.access_revoked = false;
                                    this.read_provisional_history(cx);
                                    this.refresh_history(cx);
                                    this.watch_environment(false, cx);
                                    this.watch_events(cx);
                                    if std::mem::take(&mut this.wake_after_authorize) {
                                        this.wake_for_file(cx);
                                    }
                                    cx.notify();
                                }
                            } else if this.authorized || !(loading || this.access_revoked) {
                                this.revoke(cx);
                            }
                        });
                        if alive.is_err() {
                            return;
                        }
                    }
                }
                if this
                    .update(cx, |this, cx| {
                        if this.authorized {
                            this.revoke(cx);
                        }
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(2))
                    .await;
            }
        }));
    }
    fn revoke(&mut self, cx: &mut Context<Self>) {
        self.authorized = false;
        self.access_revoked = true;
        self.admissions = Value::Null;
        self.environment = None;
        self.events = None;
        self.history_read = None;
        self.history = history::State::default();
        self.execution = None;
        self.reconnect = reconnect::Backoff::default();
        self.reconnect_wait = None;
        self.reconnect_notice = None;
        self.operation = None;
        self.computer = computer::State::default();
        self.state
            .update(cx, |state, cx| state.prepare_runtime_replacement(cx));
        self.publish_computer_online(cx);
        self.composer
            .update(cx, |composer, cx| composer.clear_cloud_task(cx));
        self.title = "Cloud chat".into();
        self.first_message = None;
        self.error = Some(
            "Cloud access changed. Select this conversation from its original account.".into(),
        );
        cx.notify();
    }
    fn revoke_for_history_error(&mut self, error: &zuse_rpc::RpcError, cx: &mut Context<Self>) {
        self.revoke(cx);
        if matches!(error, zuse_rpc::RpcError::Capability(code) if code == "workspace_not_found") {
            self.error = Some("This conversation is no longer available. Choose another conversation or start a new chat.".into());
        }
    }
    fn watch_events(&mut self, cx: &mut Context<Self>) {
        let target = self.target.clone();
        self.events=Some(cx.spawn(async move |this,cx| {
            let result=target.controller.client().subscribe_scoped(methods::CLOUD_EVENTS,
                json!({"expectedAccountId":target.subject,"expectedOrganizationId":target.organization})).await;
            let Ok(mut stream)=result else{return};
            while let Some(value)=stream.recv().await {
                if value["accountId"]!=target.subject || value["organizationId"].as_str()!=target.organization.as_deref(){continue;}
                let data=&value["data"];
                let reset = data["type"] == "reset"
                    && data.get("reason").is_none_or(|reason| reason.is_null()
                        || matches!(reason.as_str(), Some("cursor_expired" | "consumer_lagged")));
                if data["revokedWorkspaceIds"].as_array().is_some_and(|ids|ids.iter().any(|id|id==&target.workspace))
                    || matches!(data["type"].as_str(), Some("error" | "closed"))
                    || (data["cacheRevoked"] == true && !reset) {
                    let _=this.update(cx,|this,cx|this.revoke(cx));return;
                }
                let dirty=matches!(data["type"].as_str(),Some("ready"|"reset"|"revalidate")) || data["events"].as_array().is_some_and(|events|
                    events.iter().any(|event|event["workspaceId"]==target.workspace||event["resourceId"]==target.workspace));
                if dirty {let _=this.update(cx,|this,cx|this.refresh_history(cx));}
            }
        }));
    }
    fn refresh_history(&mut self, cx: &mut Context<Self>) {
        if !self.authorized {
            return;
        }
        if self.registering {
            // Its history exists once the server has recorded it.
            self.read_again = true;
            return;
        }
        if self.history_read.is_some() {
            self.read_again = true;
            return;
        }
        let target = self.target.clone();
        let mut params = self.params();
        params["incremental"] = json!(true);
        self.history_read = Some(cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .call_scoped(methods::CLOUD_HISTORY, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.history_read = None;
                if !this.authorized || this.access_revoked {
                    return;
                }
                match result {
                    Ok(value)
                        if value["accountId"] == target.subject
                            && value["organizationId"].as_str()
                                == target.organization.as_deref() =>
                    {
                        this.apply_history(value, cx)
                    }
                    Ok(_) => this.revoke(cx),
                    Err(error) if history::access_revoked(&error) => {
                        this.revoke_for_history_error(&error, cx)
                    }
                    Err(error) => {
                        this.error = Some(error.to_string());
                        cx.notify();
                    }
                }
                if std::mem::take(&mut this.read_again) {
                    this.refresh_history(cx);
                }
            });
        }));
    }
    fn apply_history(&mut self, value: Value, cx: &mut Context<Self>) {
        if !self.history.fresh_received {
            tracing::info!(workspace = %self.target.workspace, ms = self.opened_at.elapsed().as_millis() as u64,
                messages = value["head"]["transcript"]["reset"].as_array().map_or(0, Vec::len),
                replaced_cache = self.history.provisional_painted,
                "Cloud chat open: fresh history painted");
        }
        // A validated fresh receipt wins even if the local cache finishes later.
        self.history.fresh_received = true;
        self.history.provisional_read = None;
        self.history.provisional_request = None;
        if std::mem::take(&mut self.history.provisional_painted) {
            // Cached cursors are provisional too: they cannot reject a fresh
            // authoritative receipt as stale after a local snapshot replay.
            self.history.head = None;
            self.history.older_read = None;
        }
        let decoded = serde_json::from_value::<zuse_model::cloud::Head>(value["head"].clone());
        if decoded.as_ref().is_ok_and(|head| self.history.stale(head)) {
            return;
        }
        self.admissions = value["commandAdmissions"].clone();
        self.composer.update(cx, |composer, cx| {
            composer.apply_cloud_admission_activity(&self.admissions, cx);
            // The server answered: a message waiting for the connection goes now.
            composer.cloud_connection_restored(cx);
        });
        self.apply_computer_metadata(&value["workspace"], cx);
        let Ok(head) = decoded else {
            self.error = Some(
                "This conversation's saved history is unavailable. Refresh to try again.".into(),
            );
            cx.notify();
            return;
        };
        if head.workspace_id != self.target.workspace
            || head.session_id != self.target.initial_session
        {
            self.revoke(cx);
            return;
        }
        let Some((head, projection_changed)) = self.history.latest(head) else {
            return;
        };
        if projection_changed {
            self.execution = None;
            self.state
                .update(cx, |state, cx| state.prepare_runtime_replacement(cx));
        }
        let saved_title = head
            .chat
            .title
            .as_deref()
            .filter(|title| !title.trim().is_empty());
        if let Some(title) = self.live_tab_title(self.state.read(cx)).or(saved_title) {
            self.title = zuse_protocol::file_mentions::mention_display_text(title);
            self.title_is_placeholder = false;
        }
        self.first_message = None;
        self.error = None;
        if let Some(config) = &head.chat.config {
            self.config["agent"] = serde_json::to_value(config.harness).unwrap_or_default();
            self.config["model"] = json!(config.model);
            self.config["reasoning"] = serde_json::to_value(config.reasoning).unwrap_or_default();
            self.config["sandbox"] = serde_json::to_value(config.sandbox).unwrap_or_default();
            self.config["modelOptions"] = json!(config.model_options);
            self.config["runtimeMode"] = json!(match config.sandbox {
                zuse_protocol::SandboxLevel::DangerFullAccess => "full-access",
                zuse_protocol::SandboxLevel::Auto => "auto",
                _ => "approval-required",
            });
            self.learn_harness(cx);
        }
        self.config["queueRunSelectionVersion"] = value["queueRunSelectionVersion"].clone();
        if let Ok(registered) = serde_json::from_value::<Vec<zuse_protocol::Chat>>(
            value["registeredConversations"].clone(),
        ) {
            self.registered = registered;
        }
        // Registered here but not yet on the computer: keep its row while
        // the computer's chat list catches up, or attaching drops it.
        // The row lives until the computer lists the conversation (then
        // `apply_chats` drops it) or this tab closes with its state.
        let registered_only = value["source"] == "admission"
            && value["session"].is_object()
            && value["admission"].is_null();
        if registered_only {
            self.state.update(cx, |state, _| {
                state.pending_cloud_conversation = Some(head.chat.clone());
            });
        }
        // Never inject saved pages into the live watch's delta baseline.
        self.project_saved_history(&head, cx);
        self.composer.update(cx, |composer, cx| {
            composer.set_cloud_task(self.target.clone(), self.config.clone(), cx)
        });
        if self.authorized && !self.domain_sleeping() && self.status["state"] == "ready" {
            self.connect_execution(cx);
        }
        cx.notify();
    }
    fn domain_sleeping(&self) -> bool {
        matches!(
            self.computer.metadata["state"].as_str(),
            Some("paused" | "pausing" | "archived" | "archiving")
        ) || matches!(
            self.computer.metadata["desiredState"].as_str(),
            Some("paused" | "archived")
        )
    }
    fn detach_execution(&mut self, cx: &mut Context<Self>) {
        self.history.observe_live(self.state.read(cx));
        self.history.materialize_live(self.state.read(cx));
        self.execution = None;
        self.state.update(cx, |state, cx| {
            state.detach_cloud_conversation_execution(cx)
        });
    }
    fn apply_computer_metadata(&mut self, row: &Value, cx: &mut Context<Self>) {
        if row["workspaceId"] != self.target.workspace {
            return;
        }
        let refresh_logs = self.computer.observe_metadata(row.clone());
        // The header's status glyph follows the new state.
        self.publish_computer_online(cx);
        cx.notify();
        let sleeping = self.domain_sleeping();
        self.composer
            .update(cx, |composer, _| composer.set_cloud_task_sleeping(sleeping));
        if sleeping && (self.execution.is_some() || self.state.read(cx).engine().is_some()) {
            self.detach_execution(cx);
        }
        if refresh_logs {
            self.refresh_computer(cx);
        }
    }
    fn foreground_admitted(&mut self, cx: &mut Context<Self>) {
        if !self.authorized || self.access_revoked {
            return;
        }
        // A durable Run already owns the wake. This only replaces a retired
        // attachment; neither history nor queue/config changes can resume it.
        if !self.ready(cx) {
            self.environment = None;
            self.detach_execution(cx);
            self.watch_environment(false, cx);
        }
        self.refresh_history(cx);
    }
    /// Wake the computer so a message's files can go straight to it.
    fn wake_for_upload(&mut self, cx: &mut Context<Self>) {
        if self.ready(cx) {
            return;
        }
        // A sleep in progress would drop the wake; start it once sleep ends.
        if self.operation.is_some() {
            self.wake_after_sleep = true;
            return;
        }
        self.environment = None;
        self.watch_environment(true, cx);
    }
    /// Bring the computer's tools up for a file tab that cannot read its
    /// file yet. A running computer whose connection is ready only needs
    /// the connection attached; anything else takes the same wake a
    /// message's files take (it resumes nothing on a running computer and
    /// reopens a retired or dropped connection). A request that cannot
    /// start, or fails on the way, says so on the shared state so the tab
    /// offers a retry instead of waiting on a computer that is not coming.
    fn wake_for_file(&mut self, cx: &mut Context<Self>) {
        if self.state.read(cx).engine().is_some() {
            return;
        }
        if self.authorized
            && self.operation.is_none()
            && !self.domain_sleeping()
            && self.status["state"] == "ready"
        {
            self.reconnect_wait = None;
            self.connect_execution(cx);
            return;
        }
        if self.access_revoked {
            self.report_wake_failure("This computer belongs to another account.", cx);
            return;
        }
        if !self.authorized {
            self.wake_after_authorize = true;
            return;
        }
        if self.operation.is_some() {
            self.wake_after_sleep = true;
            return;
        }
        if !self.computer.execution_controls_enabled() {
            self.report_wake_failure("The computer can't be woken right now.", cx);
            return;
        }
        self.environment = None;
        self.watch_environment(true, cx);
    }
    /// Tell this chat's views whether the computer is up, as the header's
    /// glyph shows it, so a file tab waiting on its connection says it is
    /// loading rather than waking a computer that is already awake.
    pub(super) fn publish_computer_online(&mut self, cx: &mut Context<Self>) {
        let online = self.computer_online();
        if self.state.read(cx).cloud_computer_online != online {
            self.state.update(cx, |state, cx| {
                state.cloud_computer_online = online;
                cx.notify();
            });
        }
    }
    fn report_wake_failure(&mut self, message: &str, cx: &mut Context<Self>) {
        let message = gpui::SharedString::from(message.to_owned());
        self.state.update(cx, |state, cx| {
            state.cloud_wake_failure = Some(message);
            cx.notify();
        });
    }
    fn watch_environment(&mut self, interactive: bool, cx: &mut Context<Self>) {
        if !self.authorized
            || interactive
                && (self.operation.is_some() || !self.computer.execution_controls_enabled())
        {
            return;
        }
        let target = self.target.clone();
        let mut params = self.params();
        params["interactiveResume"] = json!(interactive);
        self.environment = Some(cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .subscribe_scoped(methods::CLOUD_ENVIRONMENT, params)
                .await;
            let Ok(mut stream) = result else {
                let _ = this.update(cx, |this, cx| {
                    this.environment = None;
                    if interactive {
                        this.report_wake_failure("Couldn't reach the computer.", cx);
                    }
                    // Watched again after a backoff; shown only if it lasts.
                    this.reconnect_later(false, cx);
                    cx.notify();
                });
                return;
            };
            while let Some(status) = stream.recv().await {
                let _ = this.update(cx, |this, cx| {
                    let ready = status["state"] == "ready";
                    let sleeping = this.domain_sleeping() || status["state"] == "paused";
                    // Retired transport says blocked, not paused. It cannot
                    // erase the authoritative retained computer lifecycle.
                    if sleeping || ready {
                        this.composer
                            .update(cx, |composer, _| composer.set_cloud_task_sleeping(sleeping));
                    }
                    // A wait for the computer ends at a status that will not
                    // become ready on its own: a failed start, or a blocked
                    // connection (retired, refused, or out of retries).
                    if interactive {
                        match status["state"].as_str() {
                            Some("failed" | "error") => {
                                this.report_wake_failure("The computer couldn't start.", cx)
                            }
                            Some("blocked") => this.report_wake_failure(
                                status["message"]
                                    .as_str()
                                    .filter(|message| !message.is_empty())
                                    .unwrap_or("Couldn't reach the computer."),
                                cx,
                            ),
                            _ => {}
                        }
                    }
                    // The connection came back on its own (a network change):
                    // try the tools now rather than wait out the backoff.
                    if ready && this.status["state"] != "ready" {
                        this.reconnect_wait = None;
                    }
                    this.status = status;
                    this.publish_computer_online(cx);
                    if ready && !this.domain_sleeping() {
                        this.connect_execution(cx);
                    }
                    cx.notify();
                });
            }
            // A wake whose status stream ends before the computer's tools
            // connect (or start connecting) did not bring it up.
            if interactive {
                let _ = this.update(cx, |this, cx| {
                    if this.state.read(cx).engine().is_none() && this.execution.is_none() {
                        this.report_wake_failure("Couldn't reach the computer.", cx);
                    }
                });
            }
        }));
    }
    fn connect_execution(&mut self, cx: &mut Context<Self>) {
        // A pending backoff wait is honored; its timer, Reconnect, a file tab
        // asking for the computer, or the connection coming back clear it.
        if !self.authorized || self.execution.is_some() || self.reconnect_wait.is_some() {
            return;
        }
        if let Some(engine) = self.state.read(cx).engine() {
            if matches!(self.state.read(cx).connection, ConnectionStatus::Ready)
                || engine.retry_connection()
            {
                return;
            }
            // That connection stopped for good: the computer's runtime was
            // replaced (it updates in place) or its access ended. A new one
            // is opened instead of leaving the tools disconnected.
            self.detach_execution(cx);
        }
        let mut boot = self.boot.clone();
        boot.target = Some(crate::state::RemoteTarget::Cloud(self.target.clone()));
        let session = self.target.initial_session.clone();
        let connect = Tokio::spawn(cx, crate::state::EngineHandle::bootstrap(boot));
        self.execution = Some(cx.spawn(async move |this, cx| {
            let result = connect.await;
            let _ = this.update(cx, |this, cx| {
                this.execution = None;
                let manual = std::mem::take(&mut this.reconnect_manual);
                if !this.authorized {
                    return;
                }
                match result {
                    Ok(Ok(handle)) => {
                        this.reconnect.succeeded();
                        this.reconnect_wait = None;
                        this.reconnect_notice = None;
                        this.history
                            .start_live(this.state.read(cx).transcript_revision);
                        this.state.update(cx, |state, cx| {
                            state.attach_cloud_conversation(handle, session, cx)
                        });
                    }
                    result => {
                        let error = match result {
                            Ok(Err(error)) => error.to_string(),
                            _ => "connection task ended".into(),
                        };
                        tracing::warn!(workspace = %this.target.workspace, %error,
                            "Cloud computer tools did not connect; retrying");
                        this.reconnect_later(manual, cx);
                    }
                }
                cx.notify();
            });
        }));
    }
    /// The computer's tools did not connect: try again after a backoff
    /// (sooner when the machine wakes). Only a failure that lasts is shown.
    fn reconnect_later(&mut self, manual: bool, cx: &mut Context<Self>) {
        let first = !self.reconnect.failing();
        let shown = self.reconnect.visible();
        let delay = self
            .reconnect
            .failed(cx.background_executor().now(), manual);
        if self.reconnect.visible() && !shown {
            self.report_wake_failure("Couldn't connect to the computer.", cx);
        }
        if first && !self.reconnect.visible() {
            // Shown once the failure has lasted, even while an attempt or a
            // wait is still running.
            let executor = cx.background_executor().clone();
            self.reconnect_notice = Some(cx.spawn(async move |this, cx| {
                executor.timer(reconnect::SHOW_AFTER).await;
                let _ = this.update(cx, |this, cx| {
                    this.reconnect_notice = None;
                    if this.reconnect.failing() && !this.reconnect.visible() {
                        this.reconnect.show();
                        this.report_wake_failure("Couldn't connect to the computer.", cx);
                        cx.notify();
                    }
                });
            }));
        }
        let executor = cx.background_executor().clone();
        self.reconnect_wait = Some(cx.spawn(async move |this, cx| {
            reconnect::wait(executor, delay).await;
            let _ = this.update(cx, |this, cx| {
                this.reconnect_wait = None;
                this.retry_execution(cx);
            });
        }));
    }
    /// Try the computer's tools again. The environment is watched afresh,
    /// which asks a connection that gave up to try again; its ready status
    /// then opens the tools. Never resumes a sleeping computer.
    fn retry_execution(&mut self, cx: &mut Context<Self>) {
        if !self.authorized
            || self.execution.is_some()
            || self.domain_sleeping()
            || self.state.read(cx).engine().is_some()
                && matches!(self.state.read(cx).connection, ConnectionStatus::Ready)
        {
            return;
        }
        if self.operation.is_none() {
            self.environment = None;
            self.watch_environment(false, cx);
        }
    }
    /// The strip's Reconnect: try now, and say so if it fails.
    fn reconnect_now(&mut self, cx: &mut Context<Self>) {
        self.reconnect_wait = None;
        self.reconnect_manual = true;
        self.retry_execution(cx);
        cx.notify();
    }
    fn sleep(&mut self, cx: &mut Context<Self>) {
        if !self.authorized
            || self.operation.is_some()
            || !self.computer.execution_controls_enabled()
        {
            return;
        }
        self.environment = None;
        self.detach_execution(cx);
        // A sleeping computer is not reconnected; waking it starts afresh.
        self.reconnect = reconnect::Backoff::default();
        self.reconnect_wait = None;
        self.reconnect_notice = None;
        self.status = json!({"state":"connecting","message":"Putting computer to sleep…"});
        let target = self.target.clone();
        let mut params = self.params();
        params["commandId"] = json!(
            self.sleep_command
                .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
        );
        self.operation = Some(cx.spawn(async move |this, cx| {
            let result = target
                .controller
                .client()
                .call_scoped(methods::CLOUD_PAUSE, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.operation = None;
                match result {
                    Ok(value) => {
                        this.sleep_command = None;
                        this.apply_computer_metadata(&value["workspace"], cx);
                        if std::mem::take(&mut this.wake_after_sleep) {
                            this.environment = None;
                            this.watch_environment(true, cx);
                        }
                    }
                    Err(error) => {
                        this.error = Some(error.to_string());
                        // A wake asked for during the pause still happens.
                        let wake = std::mem::take(&mut this.wake_after_sleep);
                        this.watch_environment(wake, cx);
                    }
                }
                this.refresh_history(cx);
                cx.notify();
            });
        }));
    }
}
impl CloudConversation {
    /// The computer's tools have stayed disconnected on an awake computer.
    fn tools_offline(&self, cx: &App) -> bool {
        self.authorized
            && self.reconnect.visible()
            && !self.domain_sleeping()
            && !(self.state.read(cx).engine().is_some()
                && matches!(self.state.read(cx).connection, ConnectionStatus::Ready))
    }
    fn render_tools_offline(
        &self,
        theme: &Theme,
        reconnecting: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id("native-cloud-tools-offline")
            .debug_selector(|| "native-cloud-tools-offline".into())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(Theme::SPACE_SM))
            .px(px(12.))
            .py(px(6.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(theme.text_muted)
                    .child(reconnect::NOTICE),
            )
            .child(
                crate::popover::btn_primary(
                    theme,
                    if reconnecting {
                        "Reconnecting…"
                    } else {
                        "Reconnect"
                    },
                )
                .id("native-cloud-tools-reconnect")
                .debug_selector(|| "native-cloud-tools-reconnect".into())
                .flex_none()
                .when(reconnecting, |button| button.opacity(0.6))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !reconnecting {
                        this.reconnect_now(cx);
                    }
                })),
            )
    }
}
impl Render for CloudConversation {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        self.sync_progress(cx);
        let composer_visible = self.authorized && self.computer.composer_enabled();
        let composer_width =
            settings::current(cx).transcript_width + 2. * crate::composer::COMPOSER_OUTER_PAD;
        let notice = self.error.clone();
        let tools_offline = self.tools_offline(cx);
        let reconnecting = self.execution.is_some();
        let show_tray = notice.is_some() || tools_offline;
        div()
            .id("native-cloud-conversation")
            .size_full()
            .flex()
            .flex_col()
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .children(self.render_older_history(cx))
            .child(div().flex_1().min_h_0().child(self.transcript.clone()))
            .when(show_tray || composer_visible, |view| {
                view.child(
                    div()
                        .w_full()
                        .max_w(px(composer_width))
                        .mx_auto()
                        .pb(px(12.))
                        .when(show_tray, |stack| {
                            stack.child(
                                div()
                                    .id("native-cloud-queue-tray")
                                    .debug_selector(|| "native-cloud-queue-tray".into())
                                    .mx(px(24.))
                                    .rounded_t(px(14.))
                                    .border_1()
                                    .border_b_0()
                                    .border_color(theme.border)
                                    .bg(theme.bg)
                                    .overflow_hidden()
                                    .when_some(notice, |tray, text| {
                                        tray.child(
                                            div()
                                                .id("native-cloud-conversation-notice")
                                                .px(px(12.))
                                                .py(px(8.))
                                                .text_size(px(12.))
                                                .text_color(theme.text_muted)
                                                .child(text),
                                        )
                                    })
                                    .when(tools_offline, |tray| {
                                        tray.child(self.render_tools_offline(
                                            &theme,
                                            reconnecting,
                                            cx,
                                        ))
                                    }),
                            )
                        })
                        .when(composer_visible, |stack| stack.child(self.composer.clone())),
                )
            })
    }
}

impl Shell {
    /// A new cloud chat between Send and the conversation opening. It already
    /// reads as the conversation it becomes: the prompt is the first message,
    /// with the working line under it, laid out where the transcript puts
    /// them, so nothing moves when the conversation takes over.
    pub(super) fn render_cloud_launch_outlet(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let Some(prompt) = self.composer.read(cx).cloud_launch_prompt() else {
            return div().into_any_element();
        };
        // A failed start says why on the status line; nothing spins here.
        let working = self.composer.read(cx).cloud_launch_working();
        let width = crate::settings::transcript_width(cx);
        // The first message sits where the transcript puts a new chat's
        // first message when its send anchors it (the top, the reply below),
        // so the conversation that takes over shows it in the same place.
        div()
            .id("cloud-launch-outlet")
            .debug_selector(|| "cloud-launch-outlet".into())
            .size_full()
            .flex()
            .flex_col()
            .justify_start()
            .pt(px(crate::transcript::FIRST_ROW_TOP_GAP_PX))
            .child(
                div().w_full().flex().justify_center().px(px(48.0)).child(
                    div()
                        .w_full()
                        .max_w(px(width))
                        .min_w_0()
                        .child(
                            div().w_full().flex().justify_end().child(
                                div()
                                    .id("cloud-launch-prompt")
                                    .debug_selector(|| "cloud-launch-prompt".into())
                                    .min_w_0()
                                    .max_w(px(width * 0.8))
                                    .bg(theme.user_bubble_fill())
                                    .rounded(px(12.0))
                                    .rounded_tr(px(4.0))
                                    .px(px(14.0))
                                    .py(px(8.0))
                                    .text_size(crate::typography::text_md())
                                    .line_height(crate::typography::ui_rems(
                                        crate::transcript::USER_LINE_HEIGHT,
                                    ))
                                    .text_color(theme.text)
                                    .opacity(0.65)
                                    .child(prompt),
                            ),
                        )
                        .when(working, |column| {
                            column.child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap(px(Theme::SPACE_SM))
                                    .pt(px(Theme::SPACE_LG))
                                    .child(crate::loaders::retro_loader(
                                        "cloud-launch-working",
                                        11.0,
                                        theme.loader_glyph(),
                                    ))
                                    .child(
                                        div()
                                            .text_size(crate::typography::text_xs())
                                            .text_color(theme.text_muted)
                                            .child("Waiting for computer…"),
                                    ),
                            )
                        }),
                ),
            )
            .into_any_element()
    }

    /// Cloud conversations keep their selected session in task state, not the
    /// navigation controller. Both kinds need the same chat chrome.
    pub(super) fn has_conversation(&self, cx: &App) -> bool {
        self.cloud_conversation.is_some() || self.state.read(cx).selected_chat_row().is_some()
    }

    /// Focus and commands follow the displayed conversation, while navigation
    /// keeps its original controller composer and drafts.
    pub(super) fn conversation_composer(&self, cx: &App) -> Entity<Composer> {
        self.cloud_conversation
            .as_ref()
            .map(|view| view.read(cx).composer.clone())
            .unwrap_or_else(|| self.composer.clone())
    }

    /// Task tools never inherit the local controller's execution target.
    pub(super) fn conversation_state(&self, cx: &App) -> Entity<AppState> {
        self.cloud_conversation
            .as_ref()
            .map(|view| view.read(cx).state.clone())
            .unwrap_or_else(|| self.state.clone())
    }
    pub(super) fn subscribe_cloud_conversation(
        &mut self,
        view: &Entity<CloudConversation>,
        cx: &mut Context<Self>,
    ) {
        self.mark_cloud_conversation_seen(view, cx);
        let (session, state, transcript) = {
            let view = view.read(cx);
            (
                view.target.initial_session.clone(),
                view.state.clone(),
                view.transcript.clone(),
            )
        };
        self.rebind_cloud_file_tabs(&session, &state, cx);
        // Spawn chips and workflow rows open their agents in the right pane,
        // fed by this conversation's state: its engine reaches the computer.
        // Plugin cards' buttons (Connect, Manage…) act on the app's plugins.
        self.cloud_transcript_events =
            Some(cx.subscribe(&transcript, |this, transcript, event, cx| {
                if matches!(
                    event,
                    TranscriptEvent::OpenSubagent { .. }
                        | TranscriptEvent::OpenWorkflowAgent { .. }
                        | TranscriptEvent::UiBlockPlugin(_)
                ) {
                    this.on_transcript_event(transcript, event, cx);
                }
            }));
        self.cloud_conversation_events =
            Some(cx.subscribe(view, move |this, _, event, cx| match event {
                CloudConversationEvent::Desktop => this.open_desktop_panel(cx),
                CloudConversationEvent::Terminal => this.add_terminal_surface(cx),
                CloudConversationEvent::Skills => this.open_skills_page(cx),
                CloudConversationEvent::Files => {
                    this.pending_workspace_command = Some(crate::composer::WorkspaceCommand::Files);
                    cx.notify();
                }
                CloudConversationEvent::OpenFile(path) => {
                    this.pending_chip_file = Some(path.clone());
                    cx.notify();
                }
                CloudConversationEvent::OpenPathIn(target, path) => {
                    this.open_path_in(target.clone(), path.clone(), cx);
                }
                CloudConversationEvent::RefreshOpenTargets => this.refresh_open_targets(cx),
                CloudConversationEvent::OpenSubagent {
                    chat_id,
                    doc_id,
                    title,
                } => this.add_subagent_surface_in(
                    state.clone(),
                    chat_id.clone(),
                    doc_id.clone(),
                    title.clone(),
                    false,
                    cx,
                ),
            }));
    }
    pub(super) fn open_cloud_conversation(
        &mut self,
        target: CloudTarget,
        row: Value,
        launch: Value,
        cx: &mut Context<Self>,
    ) {
        self.sync_cloud_tab_scope(cx);
        let state = self.state.read(cx);
        if state.cloud_user_id() != Some(target.subject.as_str())
            || state.cloud_organization_id() != target.organization.as_deref()
        {
            return;
        }
        if !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        if self.cloud_conversation.as_ref().is_some_and(|view| {
            view.read(cx).target.subject == target.subject
                && view.read(cx).target.organization == target.organization
                && view.read(cx).target.workspace == target.workspace
                && view.read(cx).target.initial_session == target.initial_session
        }) {
            self.state
                .update(cx, |state, cx| state.select_chat(None, cx));
            self.set_route(Route::Chat, cx);
            return;
        }
        // Chats left with a single tab are forgotten as this one opens.
        self.cloud_tabs
            .forget_single_tab_chats(&target.workspace, cx);
        let retained = self.cloud_tabs.retained_index(&target, cx);
        if let Some(index) = retained {
            self.select_cloud_tab(index, cx);
            return;
        }
        self.track_current_cloud_canvas(cx);
        let session = target.initial_session.clone();
        // A conversation still working in the background comes back as is.
        let view = match self.background_cloud_conversation(&target, cx) {
            Some(view) => view,
            None => self.new_cloud_conversation_view(target, row, launch, cx),
        };
        // The chat shows its own tabs; a canvas still creating a chat keeps
        // it (the sidebar reopens it).
        self.retain_cloud_tab(&view, cx);
        self.subscribe_cloud_conversation(&view, cx);
        self.cloud_conversation = Some(view);
        self.active_chat = session;
        self.state
            .update(cx, |state, cx| state.select_chat(None, cx));
        self.set_route(Route::Chat, cx);
        cx.notify();
    }
    /// A cloud conversation view for `target`, its row enriched with the
    /// repository's name from the account's project list.
    pub(super) fn new_cloud_conversation_view(
        &mut self,
        target: CloudTarget,
        row: Value,
        launch: Value,
        cx: &mut Context<Self>,
    ) -> Entity<CloudConversation> {
        // The computer record names its project only by id; carry the
        // repository's name in from the account's project list.
        let mut row = row;
        let project_id = row["projectId"]
            .as_str()
            .or_else(|| launch["projectId"].as_str())
            .map(str::to_owned);
        if let Some(project) = project_id.and_then(|id| {
            self.state
                .read(cx)
                .cloud_sidebar_projects()
                .into_iter()
                .find(|project| project["projectId"].as_str() == Some(id.as_str()))
        }) && let Some(object) = row.as_object_mut()
        {
            for key in ["displayName", "repositoryIdentity", "repositoryUrl"] {
                if let Some(value) = project.get(key).filter(|value| value.is_string()) {
                    let field = if key == "displayName" {
                        "repositoryDisplayName"
                    } else {
                        key
                    };
                    object.entry(field).or_insert_with(|| value.clone());
                }
            }
        }
        let links = Self::session_links(Some(target.initial_session.clone()), cx);
        let view = cx.new(|cx| CloudConversation::new(target, row, launch, self.boot.clone(), cx));
        view.update(cx, |view, cx| view.set_link_handler(links, cx));
        self.track_cloud_conversation(&view, cx);
        view
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    #[test]
    fn transcript_cache_keeps_the_most_recent_conversations() {
        let mut cache = TranscriptCache::default();
        let entry = |id: &str| zuse_model::SessionMessageEntry {
            id: id.into(),
            role: zuse_model::MessageRole::User,
            parts: vec![],
            created_at: 0,
            device_id: String::new(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        };
        for n in 0..=TranscriptCache::CAPACITY {
            cache.put(format!("chat-{n}"), vec![entry(&format!("m{n}"))]);
        }
        assert!(
            cache.get("chat-0").is_none(),
            "the oldest conversation is evicted"
        );
        // Revisiting refreshes a conversation's place.
        cache.put("chat-1".into(), vec![entry("again")]);
        cache.put("extra".into(), vec![entry("x")]);
        assert_eq!(cache.get("chat-1").unwrap()[0].id, "again");
        assert!(cache.get("chat-2").is_none());
    }
    struct PendingController;
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for PendingController {
        async fn handle(
            &self,
            _method: &str,
            _params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                futures::stream::pending(),
            )))
        }
    }
    fn initialize(cx: &mut App) {
        gpui_base::init(cx);
        cx.set_global(Theme::default());
        crate::app_menus::init(cx);
    }
    struct ChatSurface(Entity<CloudConversation>);
    impl Render for ChatSurface {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let header = self.0.update(cx, |view, cx| view.render_header_control(cx));
            div()
                .relative()
                .w(px(1000.))
                .h(px(720.))
                .bg(Theme::of(cx).bg)
                .child(self.0.clone())
                .child(div().absolute().top(px(4.)).right(px(12.)).child(header))
        }
    }
    #[gpui::test]
    fn cloud_chat_uses_one_composer_tray_and_an_icon_menu(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            // The test's own current-thread runtime: an owned multi-thread
            // runtime adds worker threads the deterministic scheduler rejects.
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let directory = tempfile::tempdir().unwrap();
        let view = cx.new(|cx| CloudConversation::new(CloudTarget {
            controller, subject: "owner".into(), organization: None, workspace: "task".into(), initial_session: "session".into(),
        }, json!({"workspaceId":"task","state":"provisioning","agent":"codex","model":"gpt-6.1-sol"}), json!({"firstMessage":"Build a small website"}), boot(directory.path()), cx));
        view.update(cx, |view, _| {
            view.account = None;
            view.authorized = true;
        });
        let window = cx.add_window(|_, _| ChatSurface(view.clone()));
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            assert!(
                visual.debug_bounds("native-cloud-queue-tray").is_none(),
                "the prompt is never parked in a queue card"
            );
            assert!(
                visual.debug_bounds("native-cloud-terminal").is_none(),
                "no permanent cloud toolbar"
            );
            let button = visual
                .debug_bounds("native-cloud-computer")
                .expect("normal header icon");
            visual.simulate_click(button.center(), gpui::Modifiers::default());
        }
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            let echoes = view.state.read(cx).pending_echoes();
            assert_eq!(
                echoes.len(),
                1,
                "the prompt is the conversation's first message"
            );
            assert!(matches!(
                &echoes[0].parts[..],
                [zuse_model::MessagePart::Text { text, .. }] if text == "Build a small website"
            ));
            assert_eq!(
                view.composer.read(cx).cloud_status(),
                None,
                "routine startup phases stay out of the composer's status line"
            );
        });
        assert!(view.read_with(cx, |view, _| view.computer.open));
        view.update(cx, |view, cx| {
            view.close_computer(cx);
        });
        assert!(!view.read_with(cx, |view, _| view.computer.open));
    }

    #[test]
    fn cloud_harness_reads_saved_and_account_agent_ids() {
        use zuse_protocol::HarnessId;
        assert_eq!(
            cloud_harness(&json!({"agent":"codex"})),
            Some(HarnessId::Codex)
        );
        assert_eq!(
            cloud_harness(&json!({"agent":"claude-code"})),
            Some(HarnessId::ClaudeCode)
        );
        assert_eq!(
            cloud_harness(&json!({"agent":"claude"})),
            Some(HarnessId::ClaudeCode)
        );
        assert_eq!(
            cloud_harness(&json!({"agent":"grok"})),
            Some(HarnessId::Grok)
        );
        assert_eq!(cloud_harness(&json!({"agent":""})), None);
        assert_eq!(cloud_harness(&json!({})), None);
    }

    #[test]
    fn cloud_tab_agent_prefers_the_chat_then_the_catalog_then_the_saved_agent() {
        use zuse_protocol::HarnessId;
        let catalog = json!({"workspaceId":"task","agent":"claude"});
        assert_eq!(
            resolve_cloud_harness(
                Some(HarnessId::Codex),
                Some(&catalog),
                Some(HarnessId::Grok)
            ),
            Some(HarnessId::Codex)
        );
        assert_eq!(
            resolve_cloud_harness(None, Some(&catalog), Some(HarnessId::Grok)),
            Some(HarnessId::ClaudeCode)
        );
        // A catalog row without an agent (an older task, or the saved
        // catalog painted at startup) falls through to the saved agent.
        assert_eq!(
            resolve_cloud_harness(
                None,
                Some(&json!({"workspaceId":"task"})),
                Some(HarnessId::Grok)
            ),
            Some(HarnessId::Grok)
        );
        // Only a brand-new chat with no agent anywhere shows the chat glyph.
        assert_eq!(resolve_cloud_harness(None, None, None), None);
    }

    #[gpui::test]
    fn cloud_tab_agent_paints_from_the_saved_agent_and_never_reverts(cx: &mut TestAppContext) {
        use zuse_protocol::HarnessId;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
            crate::settings::init(crate::settings::UiSettings::default(), directory.path(), cx);
            crate::settings::remember_cloud_agent("task", HarnessId::Grok, cx);
        });
        let open = |cx: &mut TestAppContext, workspace: &str| {
            let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
                std::sync::Arc::new(PendingController),
            ));
            let path = directory.path().to_owned();
            let workspace = workspace.to_owned();
            cx.new(move |cx| {
                CloudConversation::new(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: workspace.clone(),
                        initial_session: "session".into(),
                    },
                    // The saved catalog row carries no agent.
                    json!({"workspaceId":workspace,"state":"running"}),
                    Value::Null,
                    boot(&path),
                    cx,
                )
            })
        };
        let view = open(cx, "task");
        view.update(cx, |view, cx| {
            view.account = None;
            assert_eq!(
                view.harness(None),
                Some(HarnessId::Grok),
                "the first frame uses the agent saved for this workspace"
            );
            // The chat's details name the real agent: adopt and remember it.
            view.config["agent"] = json!("codex");
            view.learn_harness(cx);
            assert_eq!(view.harness(None), Some(HarnessId::Codex));
            // A reload whose config has no agent keeps the known icon.
            view.config["agent"] = Value::Null;
            view.learn_harness(cx);
            assert_eq!(view.harness(None), Some(HarnessId::Codex));
        });
        cx.update(|cx| {
            assert_eq!(
                crate::settings::remembered_cloud_agent("task", cx),
                Some(HarnessId::Codex),
                "the learned agent replaces the saved one"
            );
        });
        // A brand-new chat no source knows shows the plain chat glyph.
        let fresh = open(cx, "fresh");
        fresh.update(cx, |view, _| {
            view.account = None;
            assert_eq!(view.harness(None), None);
        });
    }

    #[test]
    fn header_status_is_green_only_while_online() {
        let theme = Theme::default();
        assert_eq!(header_status_color(true, &theme), theme.success);
        assert_ne!(header_status_color(false, &theme), theme.success);
    }

    #[gpui::test]
    fn header_status_follows_workspace_state_events(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let directory = tempfile::tempdir().unwrap();
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"provisioning","agent":"codex"}),
                Value::Null,
                boot(directory.path()),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.account = None;
            view.authorized = true;
            assert_eq!(view.harness(None), Some(zuse_protocol::HarnessId::Codex));
            assert!(!view.computer_online(), "starting is not online yet");
            let mut revision = 1;
            let mut apply = |view: &mut CloudConversation, state: &str, cx: &mut Context<_>| {
                revision += 1;
                view.apply_computer_metadata(
                    &json!({"workspaceId":"task","state":state,"revision":revision}),
                    cx,
                );
            };
            apply(view, "ready", cx);
            assert!(view.computer_online());
            for unavailable in ["paused", "failed", "deleted"] {
                apply(view, unavailable, cx);
                assert!(
                    !view.computer_online(),
                    "{unavailable} reads as unavailable"
                );
                apply(view, "ready", cx);
                assert!(view.computer_online());
            }
            view.revoke(cx);
            assert!(!view.computer_online(), "another account's chat is grey");
        });
    }

    fn boot(path: &std::path::Path) -> EngineBootConfig {
        EngineBootConfig {
            target: None,
            data_dir: path.into(),
            ipc_port: 0,
            edge_url: "http://127.0.0.1:1".into(),
            edge_token: None,
            org_id: None,
            workos_client_id: None,
            default_harness: zuse_protocol::HarnessId::Mock,
        }
    }
    fn account(state: &mut AppState) -> Value {
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        let project = json!({"projectId":"project","accountId":"owner","repositoryIdentity":"github.com/owner/repo","displayName":"repo"});
        assert!(state.apply_cloud_sidebar_catalog(
            &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[project]}),
            &json!({"accountId":"owner","organizationId":null,"workspaces":[]})));
        project
    }
    struct PreparationLogController {
        requests: std::sync::atomic::AtomicUsize,
        release: tokio::sync::Semaphore,
        metadata: std::sync::Mutex<Value>,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for PreparationLogController {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            if method != methods::CLOUD_WORKSPACE {
                return Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::pending(),
                )));
            }
            assert_eq!(params["workspaceId"], "task");
            assert_eq!(params["includePreparationLogs"], true);
            self.requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.release.acquire().await.unwrap().forget();
            Ok(zuse_rpc::RpcReply::Value(
                json!({"accountId":"owner","organizationId":null,"workspace":self.metadata.lock().unwrap().clone(),"preparationLogs":{"log":{"stdout":"Latest initialization output","stderr":""}}}),
            ))
        }
    }
    #[gpui::test]
    fn open_computer_logs_follow_stage_events_with_one_coalesced_trailing_refresh(
        cx: &mut TestAppContext,
    ) {
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(initialize);
        let service = Arc::new(PreparationLogController {
            requests: AtomicUsize::new(0),
            release: tokio::sync::Semaphore::new(0),
            metadata: Mutex::new(json!({"workspaceId":"task","state":"provisioning","revision":1})),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let directory = tempfile::tempdir().unwrap();
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task"}),
                Value::Null,
                boot(directory.path()),
                cx,
            )
        });
        view.update(cx,|view,cx|{
            view.account=None;view.authorized=true;
            view.apply_computer_metadata(&json!({"workspaceId":"task","state":"provisioning","revision":1,"preparation":{"repository":{"state":"running"}}}),cx);
        });
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        assert_eq!(
            service.requests.load(Ordering::SeqCst),
            0,
            "closed Details never fetches setup output on stage events"
        );
        view.update(cx, |view, cx| view.toggle_computer(cx));
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        assert_eq!(service.requests.load(Ordering::SeqCst), 1);
        let latest = json!({"workspaceId":"task","state":"provisioning","revision":3,"preparation":{"repository":{"state":"completed"},"environment":{"state":"running"},"credentials":{"state":"running"}}});
        *service.metadata.lock().unwrap() = latest.clone();
        view.update(cx,|view,cx|{
            view.apply_computer_metadata(&json!({"workspaceId":"task","state":"provisioning","revision":2,"preparation":{"repository":{"state":"completed"}}}),cx);
            view.apply_computer_metadata(&latest,cx);
        });
        assert_eq!(
            service.requests.load(Ordering::SeqCst),
            1,
            "changes while a request is active only mark it dirty"
        );
        service.release.add_permits(1);
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        assert_eq!(
            service.requests.load(Ordering::SeqCst),
            2,
            "multiple live stage changes produce one trailing refresh"
        );
        service.release.add_permits(1);
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        view.update(cx, |view, cx| {
            assert_eq!(
                view.computer.setup_output_for_test()["stdout"],
                "Latest initialization output"
            );
            view.toggle_computer(cx);
            view.apply_computer_metadata(
                &json!({"workspaceId":"task","state":"ready","revision":4}),
                cx,
            );
        });
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        assert_eq!(
            service.requests.load(Ordering::SeqCst),
            2,
            "closed Details do not poll or retain a refresh loop"
        );
        view.update(cx, |view, cx| view.revoke(cx));
        for _ in 0..8 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
    }
    struct RetiredEnvironmentController {
        awake: std::sync::atomic::AtomicBool,
        environment_calls: std::sync::Mutex<Vec<Value>>,
        requests: std::sync::Mutex<Vec<Value>>,
        admit: tokio::sync::Semaphore,
    }
    fn retained_history(awake: bool) -> Value {
        json!({"accountId":"owner","organizationId":null,"workspace":{"workspaceId":"task","providerId":"boxd","state":if awake{"ready"}else{"paused"},"desiredState":if awake{"ready"}else{"paused"}},"head":{"schemaVersion":1,"payloadFormat":"fixture","workspaceId":"task","sessionId":"session","runtimeGeneration":1,"cursor":{"epoch":"fixture","version":1},"chat":{"id":"session","deviceId":"cloud:task","archived":false,"createdAt":"2026-10-31T00:00:00Z","title":"Retained work"},"conversations":[],"transcript":{"reset":[{"id":"saved-user","role":"user","parts":[{"id":"saved-text","kind":"text","text":"Earlier saved work"}],"createdAt":1,"deviceId":"cloud:task"}]},"queue":[],"queuePaused":false,"olderMessageSequence":null}})
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for RetiredEnvironmentController {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            use futures::StreamExt as _;
            let pending = |initial: Value| {
                zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::once(async move { initial }).chain(futures::stream::pending()),
                ))
            };
            match method {
                methods::CLOUD_ACCOUNT_STATUS => {
                    Ok(pending(json!({"user":{"id":"owner"},"orgId":null})))
                }
                methods::CLOUD_EVENTS => Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::pending(),
                ))),
                methods::CLOUD_HISTORY => Ok(zuse_rpc::RpcReply::Value(retained_history(
                    self.awake.load(std::sync::atomic::Ordering::SeqCst),
                ))),
                methods::CLOUD_ENVIRONMENT => {
                    self.environment_calls.lock().unwrap().push(params);
                    Ok(pending(
                        if self.awake.load(std::sync::atomic::Ordering::SeqCst) {
                            json!({"state":"ready","epoch":2})
                        } else {
                            json!({"state":"blocked","epoch":1,"message":zuse_engine::cloud::environment::PAUSING_MESSAGE})
                        },
                    ))
                }
                methods::CLOUD_TASK_COMMAND => {
                    self.requests.lock().unwrap().push(params.clone());
                    self.admit.acquire().await.unwrap().forget();
                    self.awake.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(zuse_rpc::RpcReply::Value(
                        json!({"accountId":"owner","organizationId":null,"data":{"commandId":params["request"]["commandId"],"durablyAdmitted":true}}),
                    ))
                }
                methods::CLOUD_RPC => {
                    let request = params["requestId"].clone();
                    let value = match params["method"].as_str().unwrap_or_default() {
                        methods::ENGINE_INFO => {
                            json!({"id":request,"ok":{"deviceId":"cloud:task","workspaceScope":"synced","capabilities":[]}})
                        }
                        methods::WATCH_CHATS => {
                            json!({"id":request,"item":[retained_history(true)["head"]["chat"].clone()]})
                        }
                        methods::WATCH_DOC_MESSAGES => {
                            json!({"id":request,"item":retained_history(true)["head"]["transcript"]})
                        }
                        _ => json!({"id":request,"ok":[]}),
                    };
                    // RpcReply::Stream supplies the controller's outer item.
                    // Its payload is already the complete guest server frame.
                    Ok(pending(value))
                }
                _ => Err(zuse_rpc::RpcError::UnknownMethod(method.into())),
            }
        }
    }
    #[gpui::test]
    fn paused_history_and_retired_transport_send_reopens_passively_without_losing_native_canvas(
        cx: &mut TestAppContext,
    ) {
        use std::sync::{Arc, Mutex, atomic::AtomicBool};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        });
        let service = Arc::new(RetiredEnvironmentController {
            awake: AtomicBool::new(false),
            environment_calls: Mutex::new(vec![]),
            requests: Mutex::new(vec![]),
            admit: tokio::sync::Semaphore::new(0),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let old_engine = controller.clone();
        let directory = tempfile::tempdir().unwrap();
        let view=cx.new(|cx|CloudConversation::new(CloudTarget{controller,subject:"owner".into(),organization:None,workspace:"task".into(),initial_session:"session".into()},json!({"workspaceId":"task","agent":"codex","model":"gpt-6.1-sol","runtimeMode":"full-access"}),Value::Null,boot(directory.path()),cx));
        view.update(cx, |view, cx| {
            view.account = None;
            view.authorized = true;
            view.state
                .update(cx, |state, _| state.set_test_engine(old_engine));
            view.apply_history(retained_history(false), cx);
            view.watch_environment(false, cx);
        });
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        let (composer_id, state_id) = view.read_with(cx, |view, _| {
            (view.composer.entity_id(), view.state.entity_id())
        });
        view.update(cx, |view, cx| {
            view.composer.update(cx, |composer, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("Continue retained work", cx));
                composer.send_for_test("Continue retained work".into(), false, cx);
            })
        });
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        {
            let requests = service.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(
                requests[0]["request"]["wake"], "explicit-resume",
                "real foreground Send must wake despite blocked transport"
            );
        }
        view.update(cx, |view, cx| {
            view.composer.update(cx, |composer, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("Next retained draft", cx))
            })
        });
        service.admit.add_permits(1);
        for _ in 0..40 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(view.composer.entity_id(), composer_id);
            assert_eq!(view.state.entity_id(), state_id);
            assert_eq!(
                view.composer.read(cx).input.read(cx).text(),
                "Next retained draft"
            );
            assert!(
                view.state
                    .read(cx)
                    .transcript
                    .iter()
                    .any(|entry| entry.id == "saved-user")
            );
            assert!(
                view.state.read(cx).engine().is_some(),
                "fresh passive connection must attach after retired handle"
            );
            assert_eq!(
                view.state
                    .read(cx)
                    .engine()
                    .unwrap()
                    .engine_info()
                    .device_id,
                "cloud:task",
                "old memory-test engine must be replaced"
            );
        });
        let calls = service.environment_calls.lock().unwrap();
        assert!(
            calls.len() >= 2,
            "durable foreground admission must reopen the retired environment"
        );
        assert!(
            calls.iter().all(|p| p["interactiveResume"] == false),
            "attachment must not issue a second Resume"
        );
        assert_eq!(
            service.requests.lock().unwrap().len(),
            1,
            "one durable Run only"
        );
        drop(calls);
        view.update(cx, |view, cx| view.revoke(cx));
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
    }
    #[gpui::test]
    fn admitted_cloud_activity_is_visible_before_response_and_clears_on_terminal_history(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"allocating"}),
                json!({"firstMessage":"Build this"}),
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx,|view,cx| {
            let composer=view.composer.entity_id();let state=view.state.entity_id();
            assert_eq!(view.activity(cx).unwrap().label,"Starting computer");
            view.computer.metadata=json!({"state":"ready","preparation":{"repository":{"state":"completed"},"environment":{"state":"completed"}}});
            assert!(view.activity(cx).unwrap().busy,"initial launch before history waits visibly");
            let entry=|id:&str,role,text:&str|zuse_model::SessionMessageEntry{id:id.into(),role,parts:vec![zuse_model::MessagePart::Text{id:format!("text:{id}"),text:text.into()}],created_at:0,device_id:String::new(),status:None,continuation_of:None,duration_ms:None};
            view.state.update(cx,|state,_|state.transcript=vec![entry("first",zuse_model::MessageRole::User,"Build this"),entry("answer",zuse_model::MessageRole::Assistant,"")]);
            assert!(view.activity(cx).unwrap().busy,"empty first placeholder is not a response");
            view.state.update(cx,|state,_|state.transcript[1]=entry("answer",zuse_model::MessageRole::Assistant,"I am working"));
            assert!(view.first_message.is_some(),"test covers pre-history launch fallback");
            assert!(view.activity(cx).is_none(),"visible exact launch response clears initial waiting fallback");
            view.admissions=json!([{"commandId":"launch:task","messageId":"first","text":"Build this","executionState":"running"}]);
            assert_eq!(view.activity(cx).unwrap().label,"Agent working…","actual running tools remain visible after response");
            view.state.update(cx,|state,_|state.transcript.clear());
            view.first_message=None;
            for execution in ["queued","starting","running"] {
                view.admissions=json!([{"commandId":"launch:task","messageId":"first","text":"Build this","executionState":execution}]);
                assert!(view.activity(cx).unwrap().busy,"admitted-before-response must not look idle");
            }
            view.admissions=json!([{"commandId":"queued-control","executionState":"running","messageId":null}]);
            assert!(view.activity(cx).is_none(),"queue/control delivery is not inference");
            for terminal in ["completed","failed","cancelled"] {
                view.admissions=json!([{"commandId":"launch:task","messageId":"first","text":"Build this","executionState":terminal}]);
                assert!(view.activity(cx).is_none(),"terminal receipt clears loader");
            }
            assert_eq!(view.composer.entity_id(),composer);assert_eq!(view.state.entity_id(),state,"activity never replaces desktop/controller state");
            view.authorized=true;
            view.admissions=json!([{"commandId":"launch:task","messageId":"first","text":"Build this","executionState":"running"}]);
            assert!(view.activity(cx).unwrap().busy);
            view.revoke(cx);
            assert!(view.admissions.is_null(),"revocation clears admission presentation from old account");
            assert!(view.activity(cx).is_none(),"revocation is terminal, not a new computer starting");
            assert!(view.error.as_deref().unwrap().contains("Cloud access changed"));

        });
    }
    /// A long cloud turn (text, a finished segment, then a command still
    /// running) reads as running everywhere at once: the transcript's working
    /// line, the sidebar row and tab, and the composer's Stop. Whichever
    /// record knows first (the server's command row or the attached
    /// computer's live session) is enough, and both settle together.
    #[gpui::test]
    fn a_running_cloud_turn_reads_running_in_the_transcript_sidebar_and_composer(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller: controller.clone(),
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"ready"}),
                json!({}),
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            use zuse_model::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
            let entry = |id: &str, role, text: &str, status| SessionMessageEntry {
                id: id.into(),
                role,
                parts: vec![MessagePart::Text {
                    id: format!("text:{id}"),
                    text: text.into(),
                }],
                created_at: 1,
                device_id: String::new(),
                status,
                continuation_of: None,
                duration_ms: None,
            };
            view.authorized = true;
            view.computer.metadata = json!({"state":"ready"});
            view.state.update(cx, |state, _| {
                state.selected_chat = Some("session".into());
                state.transcript = vec![
                    entry("runtime-user", MessageRole::User, "compare features", None),
                    entry(
                        "segment-1",
                        MessageRole::Assistant,
                        "I'll inspect the app first.",
                        Some(MessageStatus::Complete),
                    ),
                    entry(
                        "segment-2",
                        MessageRole::Assistant,
                        "Running the build now.",
                        Some(MessageStatus::Streaming),
                    ),
                ];
            });
            let running = |view: &mut CloudConversation, cx: &mut Context<CloudConversation>| {
                view.sync_progress(cx);
                let working_line = view.transcript.read(cx).external_activity_for_test();
                (
                    working_line.is_some(),
                    view.live_activity(cx).working,
                    view.composer.read(cx).run_live(cx),
                )
            };
            // The server's command row says running; an earlier finished
            // segment of the same turn does not end it.
            view.admissions = json!([{"commandId":"launch:task","messageId":"admitted",
                "text":"compare features","executionState":"running","createdAt":1}]);
            assert_eq!(running(view, cx), (true, true, true), "command row running");
            // No command row read yet and no computer attached: nothing
            // knows the turn runs.
            view.admissions = json!([]);
            assert_eq!(
                running(view, cx),
                (false, false, false),
                "nothing knows it runs"
            );
            // The attached computer's live session is working: running
            // everywhere, though the row has not caught up.
            let session = |status: &str| {
                serde_json::from_value::<zuse_protocol::Session>(json!({
                    "chatId":"session","deviceId":"cloud:task","status":status,
                    "startedAt":chrono::Utc::now(),"updatedAt":chrono::Utc::now()}))
                .unwrap()
            };
            view.state.update(cx, |state, _| {
                state.set_test_engine(controller.clone());
                state.apply_sessions(vec![session("working")]);
            });
            assert_eq!(
                running(view, cx),
                (true, true, true),
                "live session working"
            );
            // The turn ends: the session settles and its row completes.
            view.state.update(cx, |state, _| {
                state.apply_sessions(vec![session("idle")]);
                state.transcript[2].status = Some(MessageStatus::Complete);
            });
            view.admissions = json!([{"commandId":"launch:task","messageId":"admitted",
                "text":"compare features","executionState":"completed","createdAt":1}]);
            assert_eq!(running(view, cx), (false, false, false), "turn over");
        });
    }
    /// The owner's "you are stuck?" report: messages the server never took
    /// read as waiting for the connection, then as not sent with Retry,
    /// never as "Starting agent turn…" from an older read of a command row.
    #[gpui::test]
    fn an_undelivered_cloud_message_never_reads_as_a_starting_turn(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"ready"}),
                json!({}),
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.authorized = true;
            view.computer.metadata = json!({"state":"ready"});
            view.state.update(cx, |state, _| {
                state.selected_chat = Some("session".into());
            });
            // The last read before the connection dropped: a turn starting.
            view.admissions = json!([{"commandId":"earlier","messageId":"earlier-message",
                "text":"what was the issue?","executionState":"starting","createdAt":1}]);
            let shown = |view: &mut CloudConversation, cx: &mut Context<CloudConversation>| {
                view.sync_progress(cx);
                let transcript = view.transcript.read(cx);
                (
                    transcript
                        .external_activity_for_test()
                        .map(|line| line.to_string()),
                    transcript.external_not_sent_for_test(),
                )
            };
            assert_eq!(
                shown(view, cx),
                (Some("Starting agent turn…".into()), false),
                "the server's record alone still reads as starting"
            );
            // A message on its way while the server can't be reached.
            view.composer.update(cx, |composer, _| {
                composer.hold_cloud_send_offline_for_test(false)
            });
            assert_eq!(
                shown(view, cx),
                (Some("Waiting for connection".into()), false)
            );
            // Retried until the limit: not sent, with Retry; no working line,
            // nothing running, and the composer offers Send.
            view.composer.update(cx, |composer, _| {
                composer.hold_cloud_send_offline_for_test(true)
            });
            assert_eq!(shown(view, cx), (None, true));
            assert!(!view.live_activity(cx).working);
            assert!(!view.composer.read(cx).run_live(cx), "Send, not Stop");
        });
    }
    /// The owner's "yoo" sequence: the first message shows at once as the
    /// launch's stand-in with "Waiting for computer…"; when the computer is
    /// ready and its transcript has the message (under its own id) and the
    /// reply, the stand-in and the waiting line are gone: one user bubble.
    #[gpui::test]
    fn first_message_reconciles_to_one_bubble_without_a_stale_waiting_line(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"provisioning","statusCode":"allocating"}),
                json!({"firstMessage":"yoo"}),
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.sync_progress(cx);
            let echo = view.launch_echo_id();
            assert!(
                view.state
                    .read(cx)
                    .pending_echoes()
                    .iter()
                    .any(|e| e.id == echo),
                "the first message shows at once"
            );
            assert_eq!(
                view.transcript.read(cx).own_send_target().as_deref(),
                Some(echo.as_str()),
                "the first message anchors like any send"
            );
            assert!(view.computer_activity(cx).is_some_and(|stage| stage.busy));
            // Ready; history and the live transcript arrive together.
            view.computer.metadata = json!({"workspaceId":"task","state":"ready"});
            view.admissions = json!([{"commandId":"launch:task","messageId":"admitted-first",
                "text":"yoo","executionState":"completed"}]);
            let entry = |id: &str, role, text: &str| zuse_model::SessionMessageEntry {
                id: id.into(),
                role,
                parts: vec![zuse_model::MessagePart::Text {
                    id: format!("text:{id}"),
                    text: text.into(),
                }],
                created_at: 1,
                device_id: String::new(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            };
            view.state.update(cx, |state, _| {
                state.apply_transcript(vec![
                    entry("runtime-user", zuse_model::MessageRole::User, "yoo"),
                    entry(
                        "runtime-reply",
                        zuse_model::MessageRole::Assistant,
                        "Yoo! What are we working on?",
                    ),
                ])
            });
            view.sync_progress(cx);
            let state = view.state.read(cx);
            assert!(
                state.pending_echoes().is_empty(),
                "the stand-in is replaced"
            );
            assert_eq!(
                view.transcript.read(cx).own_send_target().as_deref(),
                Some("runtime-user"),
                "the anchor moves to the server's copy, so nothing jumps"
            );
            let users = state
                .transcript
                .iter()
                .filter(|e| e.role == zuse_model::MessageRole::User)
                .count();
            assert_eq!(users, 1, "exactly one user bubble");
            assert!(latest_message_answered(&state.transcript));
            assert!(
                !view.computer_activity(cx).is_some_and(|stage| stage.busy),
                "no stale waiting line"
            );
        });
    }
    /// The real server's order: the stand-in, then the admitted row (its
    /// own id) while the computer starts, then the computer's own record
    /// with the reply. The first message stays anchored through every
    /// replacement instead of dropping to the bottom.
    #[gpui::test]
    fn first_message_stays_anchored_through_each_server_copy(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"provisioning","statusCode":"allocating"}),
                json!({"firstMessage":"yoo"}),
                boot(dir.path()),
                cx,
            )
        });
        let entry = |id: &str, role, text: &str| zuse_model::SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![zuse_model::MessagePart::Text {
                id: format!("text:{id}"),
                text: text.into(),
            }],
            created_at: 1,
            device_id: String::new(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        };
        let target = |view: &Entity<CloudConversation>, cx: &mut TestAppContext| {
            view.read_with(cx, |view, cx| view.transcript.read(cx).own_send_target())
        };
        view.update(cx, |view, cx| view.sync_progress(cx));
        cx.run_until_parked();
        assert_eq!(target(&view, cx).as_deref(), Some("launch-echo:task"));
        // Admitted: the saved history lists the message under its admitted id.
        view.update(cx, |view, cx| {
            view.admissions = json!([{"commandId":"launch:task","messageId":"admitted-first",
                "text":"yoo","executionState":"admitted"}]);
            view.state.update(cx, |state, cx| {
                state.apply_transcript(vec![entry(
                    "admitted-first",
                    zuse_model::MessageRole::User,
                    "yoo",
                )]);
                cx.notify();
            });
        });
        cx.run_until_parked();
        view.update(cx, |view, cx| view.sync_progress(cx));
        cx.run_until_parked();
        assert_eq!(target(&view, cx).as_deref(), Some("admitted-first"));
        // Ready: the computer's own record replaces it, with the reply.
        view.update(cx, |view, cx| {
            view.computer.metadata = json!({"workspaceId":"task","state":"ready"});
            view.state.update(cx, |state, cx| {
                state.apply_transcript(vec![
                    entry("runtime-user", zuse_model::MessageRole::User, "yoo"),
                    entry(
                        "runtime-reply",
                        zuse_model::MessageRole::Assistant,
                        "Yoo! What are we working on?",
                    ),
                ]);
                cx.notify();
            });
        });
        cx.run_until_parked();
        assert_eq!(
            target(&view, cx).as_deref(),
            Some("runtime-user"),
            "the anchor follows the server's copy instead of retiring"
        );
    }
    struct OlderHistoryController {
        calls: std::sync::Mutex<Vec<Value>>,
        page: Value,
        release: tokio::sync::Semaphore,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for OlderHistoryController {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            if method == methods::CLOUD_HISTORY {
                self.calls.lock().unwrap().push(params);
                self.release.acquire().await.unwrap().forget();
                return Ok(zuse_rpc::RpcReply::Value(self.page.clone()));
            }
            assert_eq!(
                method,
                methods::CLOUD_ACCOUNT_STATUS,
                "paging must never allocate/wake/execute"
            );
            Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                futures::stream::pending(),
            )))
        }
    }
    fn saved_page(ids: &[&str], before: Option<i64>) -> Value {
        let mut value = retained_history(false);
        value["head"]["transcript"]["reset"]=json!(ids.iter().map(|id|json!({"id":id,"role":"assistant","parts":[{"id":format!("text:{id}"),"kind":"text","text":id}],"createdAt":1,"deviceId":"cloud:task"})).collect::<Vec<_>>());
        value["head"]["olderMessageSequence"] = json!(before);
        value
    }
    struct NativeOlderButton(Entity<CloudConversation>);
    impl Render for NativeOlderButton {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(600.))
                .h(px(200.))
                .children(self.0.update(cx, |view, cx| view.render_older_history(cx)))
        }
    }
    #[gpui::test]
    fn native_older_button_reads_one_scoped_page_without_wake_and_keeps_current_view(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(initialize);
        let service = std::sync::Arc::new(OlderHistoryController {
            calls: Default::default(),
            page: saved_page(&["one", "two"], None),
            release: tokio::sync::Semaphore::new(0),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let dir = tempfile::tempdir().unwrap();
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"paused","desiredState":"paused"}),
                Value::Null,
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.account = None;
            view.authorized = true;
            view.apply_history(saved_page(&["tail"], Some(2)), cx);
        });
        let transcript = view.read_with(cx, |view, _| view.transcript.entity_id());
        let window = cx.add_window(|_, _| NativeOlderButton(view.clone()));
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            let bounds = visual
                .debug_bounds("native-cloud-history-older")
                .expect("real native older button is painted");
            visual.simulate_click(bounds.center(), gpui::Modifiers::default());
        }
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        let calls = service.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["workspaceId"], "task");
        assert_eq!(calls[0]["sessionId"], "session");
        assert_eq!(calls[0]["beforeSequence"], 2);
        assert_eq!(calls[0]["cursor"]["epoch"], "fixture");
        assert_eq!(calls[0]["expectedAccountId"], "owner");
        assert!(calls[0]["interactiveResume"].is_null());
        assert!(calls[0]["wake"].is_null());
        view.update(cx, |view, cx| view.load_older(cx));
        assert_eq!(
            service.calls.lock().unwrap().len(),
            1,
            "inflight page is not fetched twice"
        );
        service.release.add_permits(1);
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        view.update(cx, |view, cx| {
            assert_eq!(view.transcript.entity_id(), transcript);
            assert_eq!(
                view.state
                    .read(cx)
                    .transcript
                    .iter()
                    .map(|row| row.id.as_str())
                    .collect::<Vec<_>>(),
                vec!["one", "two", "tail"]
            );
            assert!(
                view.history
                    .head
                    .as_ref()
                    .unwrap()
                    .older_message_sequence
                    .is_none()
            );
            assert!(view.render_older_history(cx).is_none());
            let mut latest = saved_page(&["tail", "new"], Some(2));
            latest["head"]["cursor"]["version"] = json!(2);
            view.apply_history(latest, cx);
            assert_eq!(
                view.state
                    .read(cx)
                    .transcript
                    .iter()
                    .map(|row| row.id.as_str())
                    .collect::<Vec<_>>(),
                vec!["one", "two", "tail", "new"]
            );
            view.load_older(cx);
        });
        assert_eq!(
            service.calls.lock().unwrap().len(),
            1,
            "loaded range is retained and never refetched"
        );
        cx.update_window(window.into(), |_, window, _| window.remove_window())
            .unwrap();
    }
    #[gpui::test]
    fn native_history_revoke_cancels_pending_page_and_full_live_replay_survives_detach(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(initialize);
        let service = std::sync::Arc::new(OlderHistoryController {
            calls: Default::default(),
            page: saved_page(&["one", "two"], None),
            release: tokio::sync::Semaphore::new(0),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let engine = controller.clone();
        let dir = tempfile::tempdir().unwrap();
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"paused","desiredState":"paused"}),
                Value::Null,
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx, |view, cx| {
            view.account = None;
            view.authorized = true;
            view.apply_history(saved_page(&["tail"], Some(2)), cx);
            view.load_older(cx);
        });
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        assert_eq!(service.calls.lock().unwrap().len(), 1);
        view.update(cx, |view, cx| view.revoke(cx));
        service.release.add_permits(1);
        for _ in 0..12 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        view.read_with(cx, |view, cx| {
            assert!(view.history.head.is_none());
            assert!(view.state.read(cx).transcript.is_empty());
            assert!(!view.authorized);
        });
        view.update(cx, |view, cx| {
            view.access_revoked = false;
            view.authorized = true;
            view.apply_history(saved_page(&["tail"], Some(2)), cx);
            view.history
                .start_live(view.state.read(cx).transcript_revision);
            view.state.update(cx, |state, cx| {
                state.set_test_engine(engine);
                let full = serde_json::from_value::<zuse_model::cloud::Head>(
                    saved_page(&["full-old", "live-new"], None)["head"].clone(),
                )
                .unwrap();
                state
                    .receive_transcript_update(full.transcript, cx)
                    .unwrap();
            });
            view.history.observe_live(view.state.read(cx));
            let mut active = saved_page(&["tail"], Some(2));
            active["workspace"]["state"] = json!("ready");
            active["workspace"]["desiredState"] = json!("ready");
            view.apply_history(active, cx);
            assert!(
                view.state.read(cx).engine().is_some(),
                "fixture keeps live execution attached"
            );
            assert_eq!(
                view.state.read(cx).transcript[0].id,
                "full-old",
                "API never overwrites attached watch baseline"
            );
            view.detach_execution(cx);
            view.apply_history(saved_page(&["tail"], Some(2)), cx);
            assert_eq!(
                view.state.read(cx).transcript[0].id,
                "full-old",
                "full live history survives passive detach/latest refresh"
            );
            assert!(view.render_older_history(cx).is_none());
            let mut newer = saved_page(&["live-new", "new-api-tail"], Some(1));
            newer["head"]["cursor"]["version"] = json!(2);
            newer["head"]["transcript"]["reset"][0]["parts"][0]["text"] = json!("Authoritative revised reply");
            view.apply_history(newer, cx);
            assert!(view.state.read(cx).engine().is_none());
            assert_eq!(view.state.read(cx).transcript.iter().map(|row|row.id.as_str()).collect::<Vec<_>>(),vec!["full-old","live-new","new-api-tail"],"newer authoritative API tail updates retained full history after detach");
            assert!(matches!(&view.state.read(cx).transcript[1].parts[0], zuse_model::MessagePart::Text{text,..} if text=="Authoritative revised reply"));
            let mut reset = saved_page(&["new-generation"], Some(2));
            reset["head"]["runtimeGeneration"] = json!(2);
            reset["head"]["cursor"]["epoch"] = json!("new-epoch");
            view.apply_history(reset, cx);
            assert_eq!(
                view.state.read(cx).transcript[0].id,
                "new-generation",
                "generation reset cannot retain previous projection rows"
            );
        });
    }
    #[gpui::test]
    fn native_active_history_keeps_loaded_prefix_when_latest_account_event_revalidates(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"paused","desiredState":"paused"}),
                Value::Null,
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx,|view,cx| {
            let mut initial = retained_history(false);
            let row=|id:&str|json!({"id":id,"role":"assistant","parts":[{"id":format!("text:{id}"),"kind":"text","text":id}],"createdAt":1,"deviceId":"cloud:task"});
            initial["head"]["transcript"]["reset"]=json!([row("older"),row("tail")]);
            initial["head"]["olderMessageSequence"]=json!(50);
            view.apply_history(initial.clone(),cx);
            let mut latest=initial;
            latest["head"]["cursor"]["version"]=json!(2);
            latest["head"]["transcript"]["reset"]=json!([row("tail"),row("new")]);
            latest["head"]["olderMessageSequence"]=json!(51);
            view.apply_history(latest,cx);
            assert_eq!(view.state.read(cx).transcript.iter().map(|entry|entry.id.as_str()).collect::<Vec<_>>(),vec!["older","tail","new"],"latest account history page must not discard the already loaded native prefix");
        });
    }
    #[gpui::test]
    fn cloud_tab_title_survives_placeholder_clear_and_uses_saved_history_title(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","title":"Stored title"}),
                json!({"firstMessage":"Prompt title"}),
                boot(dir.path()),
                cx,
            )
        });
        view.update(cx,|view,cx| {
            assert_eq!(view.title,"Stored title");
            let mut value=json!({"head":{"schemaVersion":1,"payloadFormat":"fixture","workspaceId":"task","sessionId":"session","runtimeGeneration":1,"cursor":{"epoch":"fixture","version":1},"chat":{"id":"session","deviceId":"fixture","archived":false,"createdAt":"2026-10-31T00:00:00Z","title":"Canonical title"},"conversations":[],"transcript":{"reset":[]},"queue":[],"queuePaused":false,"olderMessageSequence":null}});
            let _:zuse_model::cloud::Head=serde_json::from_value(value["head"].clone()).expect("real typed history fixture");view.apply_history(value.clone(),cx);assert!(view.first_message.is_none());assert_eq!(view.title,"Canonical title");value["head"]["chat"]["title"]=Value::Null;view.apply_history(value,cx);assert_eq!(view.title,"Canonical title","an empty later history title cannot erase the stable label");
        });
    }
    #[gpui::test]
    fn cloud_only_new_chat_reuses_native_shell_composer_and_window(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                let valid=account(&mut state);
                let unavailable=json!({"projectId":"unavailable","accountId":"owner","canCreate":false,"repositoryIdentity":"github.com/owner/unavailable"});
                state.cloud_project=Some(unavailable.clone());
                assert!(state.apply_cloud_sidebar_catalog(
                    &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[unavailable,valid]}),
                    &json!({"accountId":"owner","organizationId":null,"workspaces":[]})));
                state
            });
            Shell::new(state, boot(dir.path()), cx)
        });
        let windows = cx.update(|cx| cx.windows().len());
        window
            .update(cx, |shell, _, cx| {
                let controller = shell.state.entity_id();
                let composer = shell.composer.entity_id();
                let transcript = shell.transcript.entity_id();
                shell.open_new_session(None, cx);
                assert_eq!(shell.state.entity_id(), controller);
                assert_eq!(shell.composer.entity_id(), composer);
                assert_eq!(shell.transcript.entity_id(), transcript);
                assert!(matches!(shell.route, Route::Chat));
                assert!(shell.state.read(cx).selected_chat.is_none());
                assert_eq!(
                    shell.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],
                    "project"
                );
                assert_eq!(shell.state.read(cx).cloud_sidebar_projects().len(), 2);
                assert!(shell.cloud_conversation.is_none());
            })
            .unwrap();
        assert_eq!(
            cx.update(|cx| cx.windows().len()),
            windows,
            "New Chat must not create another window"
        );
    }
    #[gpui::test]
    fn admitted_cloud_task_selects_current_native_view_before_execution_ready(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        cx.update(initialize);
        // The admission refreshes the sidebar's account list.
        cx.update(|cx| gpui_tokio::init_from_handle(cx, runtime.handle().clone()));
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let window = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                account(&mut state);
                state
            });
            state.update(cx, |state, _| state.set_test_engine(controller.clone()));
            Shell::new(state, boot(dir.path()), cx)
        });
        let windows = cx.update(|cx| cx.windows().len());
        let (controller_state,sidebar,composer)=window.update(cx,|shell,_,cx| {
            let controller_state=shell.state.entity_id();let sidebar=shell.sidebar_pane.entity_id();let composer=shell.composer.entity_id();
            // Exercise the production composer event subscription, not a root replacement.
            shell.composer.update(cx,|c,cx|cx.emit(ComposerEvent::CloudAdmitted {key:c.current_key.clone(),
                workspace:json!({"workspaceId":"task","initialSessionId":"session","commandDeliveryMode":"server-admitted-v2","commandAdmissionVersion":2}),
                account:"owner".into(),organization:None,
                launch:json!({"firstMessage":"One admitted message","agent":"codex","model":"fixture"}),
            }));
            (controller_state,sidebar,composer)
        }).unwrap();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.state.entity_id(), controller_state);
                assert_eq!(shell.sidebar_pane.entity_id(), sidebar);
                assert_eq!(shell.composer.entity_id(), composer);
                let conversation = shell
                    .cloud_conversation
                    .as_ref()
                    .expect("admission selects current conversation");
                let task = conversation.read(cx);
                assert_eq!(task.first_message.as_deref(), Some("One admitted message"));
                assert_eq!(
                    task.state.read(cx).selected_chat.as_deref(),
                    Some("session")
                );
                assert!(task.state.read(cx).engine().is_none());
                assert!(!task.ready(cx));
                assert_ne!(shell.conversation_state(cx).entity_id(), controller_state);
                shell.add_terminal_surface(cx);
                assert!(
                    shell.right_terminal.is_none(),
                    "preparing cloud tools must never fall back to local execution"
                );
                assert!(
                    shell
                        .cloud_conversation
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .terminal
                        .is_none()
                );
            })
            .unwrap();
        assert_eq!(
            cx.update(|cx| cx.windows().len()),
            windows,
            "Send must not create another window"
        );
    }
    /// Paint the same chrome gate, header, tab row and right-pane host as
    /// Shell::render without starting a live account or guest connection.
    struct CloudChromeHost {
        shell: Entity<Shell>,
        _directory: tempfile::TempDir,
    }
    impl Render for CloudChromeHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.shell.update(cx, |shell, cx| {
                let header = shell
                    .main_chrome_visible(cx)
                    .then(|| shell.render_title_bar(cx));
                let tabs = shell
                    .conversation_tab_row_visible(cx)
                    .then(|| shell.render_conversation_tab_row(cx));
                div()
                    .relative()
                    .size_full()
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .child(div().flex_1())
                            .child(shell.render_right_pane(window, cx)),
                    )
                    .children(
                        header.map(|header| {
                            div().absolute().top_0().left_0().right_0().child(header)
                        }),
                    )
                    .children(tabs)
                    .child(shell.workspace_sidebar_header(cx))
            })
        }
    }
    #[gpui::test]
    fn a_cloud_chats_file_links_open_on_its_own_computer(cx: &mut TestAppContext) {
        use crate::markdown::render::{LinkAction, LinkActivation, LinkOutcome, LinkTarget};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            initialize(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    account(&mut state);
                    state
                });
                let mut shell = Shell::new(state, boot(directory.path()), cx);
                shell.reduced_motion = true;
                shell.open_cloud_conversation(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task".into(),
                        initial_session: "session".into(),
                    },
                    json!({"workspaceId":"task","initialSessionId":"session"}),
                    Value::Null,
                    cx,
                );
                shell
            });
            CloudChromeHost {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        shell.update_in(cx, |shell, window, cx| {
            let view = shell.cloud_conversation.clone().unwrap();
            let state = view.read(cx).state.clone();
            // The computer's engine reports its own device id; its paths are
            // still never this desktop's.
            state.update(cx, |state, _| {
                state.local_device_id = Some("cloud-computer".into())
            });
            assert!(state.read(cx).hosted_on_cloud_computer);
            assert!(!state.read(cx).chat_is_local("session"));
            // Before the fix the cloud transcript had no link handler, so
            // every file link fell back to web validation and was dropped.
            let transcript = view.read(cx).transcript_for_test().clone();
            let link = transcript
                .update(cx, |transcript, cx| transcript.link_ui(cx))
                .expect("a cloud chat's links are routed");
            assert_eq!(link.source_session.as_deref(), Some("session"));
            assert!(!link.source_local, "no desktop file actions");

            // The shell's own state has no selection while a cloud chat is
            // open; the link still counts as the open chat's.
            assert!(shell.state.read(cx).selected_chat.is_none());
            let page = "/home/zuse/workspace/artifacts/web-preview/onboarding.html";
            let activation = LinkActivation {
                target: LinkTarget::new("Web onboarding", page),
                action: LinkAction::Primary,
                source_session: Some("session".into()),
            };
            assert_eq!(
                shell.activate_session_link(&activation, window, cx),
                LinkOutcome::Internal
            );
            let id = shell.file_surface_seq;
            assert_eq!(shell.file_surfaces[&id].read(cx).chat_id(), "session");
            assert_eq!(shell.file_surface_paths[&id], page);
            // Web links keep their normal handling.
            let web = LinkActivation {
                target: LinkTarget::new("Docs", "https://example.com/docs"),
                action: LinkAction::External,
                source_session: Some("session".into()),
            };
            assert_eq!(
                shell.activate_session_link(&web, window, cx),
                LinkOutcome::External("https://example.com/docs".into())
            );
        });
    }
    #[gpui::test]
    fn a_cloud_chats_spawn_chip_opens_its_subagent_in_the_right_pane(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            initialize(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    account(&mut state);
                    state
                });
                let mut shell = Shell::new(state, boot(directory.path()), cx);
                shell.reduced_motion = true;
                shell.open_cloud_conversation(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task".into(),
                        initial_session: "session".into(),
                    },
                    json!({"workspaceId":"task","initialSessionId":"session"}),
                    Value::Null,
                    cx,
                );
                shell
            });
            CloudChromeHost {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        let (transcript, state) = shell.read_with(cx, |shell, cx| {
            let view = shell.cloud_conversation.clone().unwrap();
            let view = view.read(cx);
            (view.transcript_for_test().clone(), view.state.clone())
        });
        // Before the fix nothing listened to the cloud transcript, so the
        // click was dropped.
        transcript.update(cx, |_, cx| {
            cx.emit(TranscriptEvent::OpenSubagent {
                chat_id: "session".into(),
                doc_id: "session--sub--toolu_1".into(),
                title: "Explore".into(),
                frozen: false,
            })
        });
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            assert_eq!(shell.subagent_tabs.len(), 1);
            let (&id, tab) = shell.subagent_tabs.iter().next().unwrap();
            assert_eq!(tab.doc_id, "session--sub--toolu_1");
            // Fed by the conversation's own state, whose engine reaches the
            // computer holding the doc.
            assert_eq!(tab.state.entity_id(), state.entity_id());
            assert!(
                shell
                    .right_tabs
                    .values()
                    .any(|tabs| tabs.contains(&RightSurface::Subagent(id)))
            );
        });
    }
    /// The cloud computer's files, once it is awake: every image reads as
    /// one small PNG.
    struct ComputerFiles {
        png: Vec<u8>,
        image_reads: std::sync::Mutex<Vec<Value>>,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for ComputerFiles {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            use base64::Engine as _;
            if method == methods::READ_WORKSPACE_IMAGE {
                self.image_reads.lock().unwrap().push(params);
                return zuse_rpc::RpcReply::value(&zuse_protocol::WorkspaceImageChunk {
                    checkout_id: "checkout".into(),
                    content_hash: "hash".into(),
                    mime_type: "image/png".into(),
                    data: base64::engine::general_purpose::STANDARD.encode(&self.png),
                    next_offset: self.png.len(),
                    size: self.png.len(),
                    done: true,
                });
            }
            Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                futures::stream::pending(),
            )))
        }
    }
    /// Clicking an image link in a cloud chat whose computer is asleep opens
    /// a tab at once in a waking state and asks the conversation to wake the
    /// computer. A failed wake shows an inline retry; a tab closed while
    /// waiting is gone for good; once the computer's tools connect, the
    /// remaining tab shows the image.
    #[gpui::test]
    fn a_sleeping_cloud_chats_image_link_opens_a_waking_tab(cx: &mut TestAppContext) {
        use crate::files::computer_wake::WakeView;
        use crate::markdown::render::{LinkAction, LinkActivation, LinkOutcome, LinkTarget};
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.executor().allow_parking();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            initialize(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    account(&mut state);
                    state
                });
                let mut shell = Shell::new(state, boot(directory.path()), cx);
                shell.reduced_motion = true;
                shell.open_cloud_conversation(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task".into(),
                        initial_session: "session".into(),
                    },
                    json!({"workspaceId":"task","initialSessionId":"session"}),
                    Value::Null,
                    cx,
                );
                shell
            });
            CloudChromeHost {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        let (view, state, transcript) = shell.update(cx, |shell, cx| {
            let view = shell.cloud_conversation.clone().unwrap();
            let state = view.read(cx).state.clone();
            let transcript = view.read(cx).transcript_for_test().clone();
            (view, state, transcript)
        });
        // The saved history names the chat; its computer is asleep, so no
        // engine is attached.
        let chat: zuse_protocol::Chat = serde_json::from_value(json!({
            "id": "session", "deviceId": "cloud-computer", "archived": false,
            "cwd": "/home/zuse/workspace", "spaceId": "space", "checkoutId": "checkout",
            "createdAt": "2026-11-23T11:00:00Z",
        }))
        .unwrap();
        state.update(cx, |state, cx| {
            state.local_device_id = Some("cloud-computer".into());
            state.apply_chats(vec![chat]);
            cx.notify();
        });
        assert!(state.read_with(cx, |state, _| state.cloud_computer_asleep()));
        let click = |target: &str, cx: &mut gpui::VisualTestContext| {
            let link = transcript
                .update(cx, |transcript, cx| transcript.link_ui(cx))
                .expect("a cloud chat's links are routed");
            let activation = LinkActivation {
                target: LinkTarget::new("Updated reasoning slider", target),
                action: LinkAction::Primary,
                source_session: Some("session".into()),
            };
            let outcome = cx.update(|window, cx| (link.handler)(&activation, window, cx));
            assert_eq!(outcome, LinkOutcome::Internal);
            cx.run_until_parked();
            shell.read_with(cx, |shell, cx| match shell.active_main_tab(cx) {
                Some(main_tabs::MainTab::File(id)) => (id, shell.file_surfaces[&id].clone()),
                other => panic!("expected a file tab, got {other:?}"),
            })
        };

        // The click opens the tab at once, waking, and asks for the computer.
        let (_, file) = click("/home/zuse/workspace/artifacts/reasoning-slider.png", cx);
        file.read_with(cx, |file, cx| {
            assert_eq!(file.wake_view(cx), Some(WakeView::Waking))
        });
        assert_eq!(state.read_with(cx, |state, _| state.cloud_wake_requests), 1);
        view.read_with(cx, |view, _| {
            assert_eq!(view.answered_wake_requests, 1, "the conversation took it");
            // The account is still being confirmed here; the wake starts
            // the moment it is.
            assert!(view.wake_after_authorize);
        });
        cx.update(|_, cx| assert!(crate::toast::messages(cx).is_empty(), "no notice"));

        // A wake that fails is a quiet inline error with a retry, which
        // asks again.
        state.update(cx, |state, cx| {
            state.cloud_wake_failure = Some("Couldn't reach the computer.".into());
            cx.notify();
        });
        file.read_with(cx, |file, cx| {
            assert_eq!(
                file.wake_view(cx),
                Some(WakeView::Failed("Couldn't reach the computer.".into()))
            )
        });
        file.update(cx, |file, cx| file.retry_computer_wake(cx));
        cx.run_until_parked();
        file.read_with(cx, |file, cx| {
            assert_eq!(file.wake_view(cx), Some(WakeView::Waking))
        });
        state.read_with(cx, |state, _| {
            assert_eq!(state.cloud_wake_requests, 2);
            assert!(state.cloud_wake_failure.is_none());
        });
        view.read_with(cx, |view, _| assert_eq!(view.answered_wake_requests, 2));

        // A second link's tab, closed while it waits, stays closed.
        let (other_id, other) = click("/home/zuse/workspace/artifacts/other.png", cx);
        let weak_other = other.downgrade();
        drop(other);
        shell.update(cx, |shell, cx| {
            shell.close_main_tab(main_tabs::MainTab::File(other_id), cx);
            assert!(!shell.file_surfaces.contains_key(&other_id));
            assert_eq!(shell.main_tab_rows(cx).len(), 1);
        });
        cx.run_until_parked();
        assert!(
            weak_other.upgrade().is_none(),
            "the closed tab's wait is dropped"
        );

        // The computer comes up: the remaining tab reads the image from it.
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgba8(32, 24)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let files = std::sync::Arc::new(ComputerFiles {
            png: png.into_inner(),
            image_reads: Default::default(),
        });
        state.update(cx, |state, cx| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zuse_rpc::memory_client(files.clone()),
            ));
            cx.notify();
        });
        cx.run_until_parked();
        file.read_with(cx, |file, cx| assert_eq!(file.wake_view(cx), None));
        file.update(cx, |file, cx| file.show_active_image(cx));
        for _ in 0..400 {
            cx.run_until_parked();
            if file.read_with(cx, |file, cx| file.active_image_loaded(cx)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        file.read_with(cx, |file, cx| {
            assert!(file.active_image_loaded(cx), "the image is on screen");
        });
        {
            let reads = files.image_reads.lock().unwrap();
            assert!(!reads.is_empty());
            assert!(
                reads.iter().all(|read| read["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("artifacts/reasoning-slider.png"))),
                "only the open tab's image is read: {reads:?}"
            );
        }
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell.main_tab_rows(cx).len(),
                1,
                "the closed tab stays closed"
            )
        });
    }
    /// A sleeping computer that a file tab asks to wake. `fail` refuses the
    /// interactive wake the way an unreachable computer does.
    struct FileWakeController {
        fail: bool,
        /// The wake's connection is refused for good (it stays blocked).
        blocked: bool,
        /// The computer is running and its connection is ready.
        awake: bool,
        environment_calls: std::sync::Mutex<Vec<Value>>,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for FileWakeController {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            use futures::StreamExt as _;
            let pending = |initial: Value| {
                zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::once(async move { initial }).chain(futures::stream::pending()),
                ))
            };
            match method {
                methods::CLOUD_ACCOUNT_STATUS => {
                    Ok(pending(json!({"user":{"id":"owner"},"orgId":null})))
                }
                methods::CLOUD_HISTORY => {
                    Ok(zuse_rpc::RpcReply::Value(retained_history(self.awake)))
                }
                methods::CLOUD_ENVIRONMENT => {
                    let interactive = params["interactiveResume"] == true;
                    if self.awake {
                        self.environment_calls.lock().unwrap().push(params);
                        return Ok(pending(json!({"state":"ready","epoch":1})));
                    }
                    self.environment_calls.lock().unwrap().push(params);
                    if interactive && self.fail {
                        return Err(zuse_rpc::RpcError::Failed("unreachable".into()));
                    }
                    if interactive && self.blocked {
                        return Ok(pending(
                            json!({"state":"blocked","epoch":1,"message":"Connection refused."}),
                        ));
                    }
                    Ok(pending(json!({"state":"paused","epoch":1})))
                }
                _ => Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::pending(),
                ))),
            }
        }
    }
    /// A file tab's wake request takes the same interactive wake a message's
    /// files take; a wake that cannot reach the computer is reported back
    /// on the shared state so the tab can offer a retry.
    #[gpui::test]
    fn a_file_tabs_wake_request_wakes_the_computer_and_reports_failure(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        });
        let directory = tempfile::tempdir().unwrap();
        let pump = |cx: &mut TestAppContext| {
            for _ in 0..12 {
                cx.run_until_parked();
                runtime.block_on(async { tokio::task::yield_now().await });
            }
        };
        for (fail, blocked) in [(false, false), (true, false), (false, true)] {
            let service = std::sync::Arc::new(FileWakeController {
                fail,
                blocked,
                awake: false,
                environment_calls: Default::default(),
            });
            let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
                service.clone(),
            ));
            let view = cx.new(|cx| {
                CloudConversation::new(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task".into(),
                        initial_session: "session".into(),
                    },
                    json!({"workspaceId":"task","state":"paused","desiredState":"paused"}),
                    Value::Null,
                    boot(directory.path()),
                    cx,
                )
            });
            pump(cx);
            let state = view.read_with(cx, |view, _| {
                assert!(view.authorized);
                view.state.clone()
            });
            assert!(
                service
                    .environment_calls
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|call| call["interactiveResume"] == false),
                "opening the chat never wakes the computer"
            );
            state.update(cx, |state, cx| state.request_cloud_wake(cx));
            pump(cx);
            let woke = service
                .environment_calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| call["interactiveResume"] == true);
            assert!(woke, "the request wakes the computer");
            state.read_with(cx, |state, _| {
                if fail {
                    assert_eq!(
                        state.cloud_wake_failure.as_deref(),
                        Some("Couldn't reach the computer.")
                    );
                } else if blocked {
                    // A connection that stays blocked ends the wait with
                    // its reason instead of leaving the tab waking.
                    assert_eq!(
                        state.cloud_wake_failure.as_deref(),
                        Some("Connection refused.")
                    );
                } else {
                    assert!(state.cloud_wake_failure.is_none());
                }
            });
            view.update(cx, |view, cx| view.revoke(cx));
            pump(cx);
        }
    }
    /// A file tab on a running computer whose tools are not connected yet
    /// is answered with the connection alone: the computer reads as online
    /// on the shared state, and nothing asks to resume it.
    #[gpui::test]
    fn a_file_request_on_a_running_computer_connects_without_waking(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        });
        let directory = tempfile::tempdir().unwrap();
        let pump = |cx: &mut TestAppContext| {
            for _ in 0..12 {
                cx.run_until_parked();
                runtime.block_on(async { tokio::task::yield_now().await });
            }
        };
        let service = std::sync::Arc::new(FileWakeController {
            fail: false,
            blocked: false,
            awake: true,
            environment_calls: Default::default(),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","state":"ready","desiredState":"ready"}),
                Value::Null,
                boot(directory.path()),
                cx,
            )
        });
        pump(cx);
        let state = view.read_with(cx, |view, _| view.state.clone());
        state.read_with(cx, |state, _| {
            assert!(state.cloud_computer_online, "the header shows it running");
            assert!(
                state.cloud_computer_asleep(),
                "its tools are still connecting"
            );
        });
        state.update(cx, |state, cx| state.request_cloud_wake(cx));
        pump(cx);
        view.read_with(cx, |view, _| assert_eq!(view.answered_wake_requests, 1));
        assert!(
            service
                .environment_calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| call["interactiveResume"] == false),
            "a running computer is never asked to resume"
        );
        state.read_with(cx, |state, _| assert!(state.cloud_wake_failure.is_none()));
        view.update(cx, |view, cx| view.revoke(cx));
        pump(cx);
        state.read_with(cx, |state, _| assert!(!state.cloud_computer_online));
    }
    /// A computer whose tools' identity check can be made to fail, and whose
    /// runtime can be replaced in place (a new identity on a new epoch).
    struct ToolsController {
        /// Identity checks still to fail; `usize::MAX` fails until changed.
        failures: std::sync::atomic::AtomicUsize,
        identity_calls: std::sync::atomic::AtomicUsize,
        environment_calls: std::sync::Mutex<Vec<Value>>,
        environment: tokio::sync::watch::Sender<Value>,
        /// The runtime's capabilities, as its identity reports them.
        capabilities: std::sync::Mutex<Vec<&'static str>>,
    }
    impl ToolsController {
        fn new(failures: usize) -> std::sync::Arc<Self> {
            std::sync::Arc::new(Self {
                failures: failures.into(),
                identity_calls: 0.into(),
                environment_calls: Default::default(),
                environment: tokio::sync::watch::Sender::new(json!({"state":"ready","epoch":1})),
                capabilities: std::sync::Mutex::new(vec!["runtime-0.26"]),
            })
        }
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for ToolsController {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            use futures::StreamExt as _;
            use std::sync::atomic::Ordering;
            match method {
                methods::CLOUD_ACCOUNT_STATUS => Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::once(async { json!({"user":{"id":"owner"},"orgId":null}) })
                        .chain(futures::stream::pending()),
                ))),
                methods::CLOUD_HISTORY => Ok(zuse_rpc::RpcReply::Value(retained_history(true))),
                methods::CLOUD_ENVIRONMENT => {
                    self.environment_calls.lock().unwrap().push(params);
                    let status = self.environment.subscribe();
                    Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                        futures::stream::unfold((status, true), |(mut status, first)| async move {
                            if !first {
                                status.changed().await.ok()?;
                            }
                            let value = status.borrow_and_update().clone();
                            Some((value, (status, false)))
                        }),
                    )))
                }
                methods::CLOUD_RPC if params["method"] == methods::ENGINE_INFO => {
                    self.identity_calls.fetch_add(1, Ordering::SeqCst);
                    let failed = self
                        .failures
                        .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| match left {
                            0 => None,
                            usize::MAX => Some(usize::MAX),
                            left => Some(left - 1),
                        })
                        .is_ok();
                    if failed {
                        // The gateway socket was reset under the check.
                        return Err(zuse_rpc::RpcError::Transport(
                            "Cloud connection interrupted".into(),
                        ));
                    }
                    let capabilities = self.capabilities.lock().unwrap().clone();
                    Ok(zuse_rpc::RpcReply::Stream(Box::pin(futures::stream::iter(
                        [
                            json!({"id":params["requestId"],"ok":{"deviceId":"cloud:task","workspaceScope":"synced","capabilities":capabilities},"done":true}),
                        ],
                    ))))
                }
                _ => Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::pending(),
                ))),
            }
        }
    }
    struct ToolsFixture {
        runtime: tokio::runtime::Runtime,
        _directory: tempfile::TempDir,
        service: std::sync::Arc<ToolsController>,
        view: Entity<CloudConversation>,
    }
    impl ToolsFixture {
        fn open(cx: &mut TestAppContext, failures: usize) -> Self {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let service = ToolsController::new(failures);
            let directory = tempfile::tempdir().unwrap();
            let view = {
                let _entered = runtime.enter();
                cx.update(|cx| {
                    initialize(cx);
                    gpui_tokio::init_from_handle(cx, runtime.handle().clone());
                });
                let controller = crate::state::EngineHandle::from_test_client(
                    zuse_rpc::memory_client(service.clone()),
                );
                cx.new(|cx| {
                    CloudConversation::new(
                        CloudTarget {
                            controller,
                            subject: "owner".into(),
                            organization: None,
                            workspace: "task".into(),
                            initial_session: "session".into(),
                        },
                        json!({"workspaceId":"task","state":"ready","desiredState":"ready"}),
                        Value::Null,
                        boot(directory.path()),
                        cx,
                    )
                })
            };
            let fixture = Self {
                runtime,
                _directory: directory,
                service,
                view,
            };
            fixture.pump(cx);
            fixture
        }
        fn pump(&self, cx: &mut TestAppContext) {
            let _entered = self.runtime.enter();
            for _ in 0..40 {
                cx.run_until_parked();
                self.runtime.block_on(async {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await
                });
            }
        }
        /// Let the quiet retry's wait run out, then the attempt it starts.
        fn wait(&self, cx: &mut TestAppContext, seconds: u64) {
            for _ in 0..seconds {
                cx.executor()
                    .advance_clock(std::time::Duration::from_secs(1));
                self.pump(cx);
            }
        }
        fn identity_calls(&self) -> usize {
            self.service
                .identity_calls
                .load(std::sync::atomic::Ordering::SeqCst)
        }
        fn connected(&self, cx: &mut TestAppContext) -> bool {
            self.view.read_with(cx, |view, cx| {
                view.state.read(cx).engine().is_some()
                    && matches!(view.state.read(cx).connection, ConnectionStatus::Ready)
            })
        }
        fn strip(&self, cx: &mut TestAppContext) -> bool {
            self.view.read_with(cx, |view, cx| view.tools_offline(cx))
        }
        fn passive(&self) -> bool {
            self.service
                .environment_calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| call["interactiveResume"] == false)
        }
        fn close(self, cx: &mut TestAppContext) {
            {
                let _entered = self.runtime.enter();
                self.view.update(cx, |view, cx| view.revoke(cx));
            }
            self.pump(cx);
        }
    }
    /// A tools connection that fails for a moment (a gateway reset while it
    /// opens) reconnects on its own and never shows a strip or an error.
    #[gpui::test]
    fn a_brief_tools_failure_reconnects_silently(cx: &mut TestAppContext) {
        let fixture = ToolsFixture::open(cx, 2);
        assert_eq!(fixture.identity_calls(), 1, "the first attempt failed");
        assert!(!fixture.connected(cx));
        assert!(!fixture.strip(cx), "a brief failure is not shown");
        fixture.wait(cx, 1);
        assert_eq!(fixture.identity_calls(), 2, "retried after a second");
        assert!(!fixture.strip(cx));
        fixture.wait(cx, 2);
        assert_eq!(fixture.identity_calls(), 3);
        assert!(fixture.connected(cx), "the tools connected on their own");
        fixture.view.read_with(cx, |view, cx| {
            assert!(!view.reconnect.failing());
            assert!(!view.tools_offline(cx));
            assert!(view.error.is_none(), "no error was ever left behind");
            assert!(view.state.read(cx).cloud_wake_failure.is_none());
        });
        assert!(fixture.passive(), "a retry never wakes the computer");
        // Nothing more is retried once connected.
        let calls = fixture.identity_calls();
        fixture.wait(cx, 60);
        assert_eq!(fixture.identity_calls(), calls);
        fixture.close(cx);
    }
    /// A tools connection that stays down shows one strip with a Reconnect
    /// button; the button tries at once and the strip goes when it works.
    #[gpui::test]
    fn a_lasting_tools_failure_shows_a_strip_with_a_working_reconnect(cx: &mut TestAppContext) {
        let fixture = ToolsFixture::open(cx, usize::MAX);
        fixture.wait(cx, 16);
        assert!(!fixture.strip(cx), "not shown before it has lasted");
        assert!(
            fixture.identity_calls() >= 4,
            "retried quietly with backoff"
        );
        fixture.wait(cx, 4);
        assert!(
            fixture.strip(cx),
            "shown once the failure lasted 20s, between attempts"
        );
        fixture.view.read_with(cx, |view, cx| {
            assert!(view.error.is_none(), "the strip is not a stale error");
            assert_eq!(
                view.state.read(cx).cloud_wake_failure.as_deref(),
                Some("Couldn't connect to the computer.")
            );
        });
        // The computer becomes reachable; Reconnect tries now instead of
        // waiting out the backoff.
        fixture
            .service
            .failures
            .store(0, std::sync::atomic::Ordering::SeqCst);
        let calls = fixture.identity_calls();
        let window = cx.add_window(|_, _| ChatSurface(fixture.view.clone()));
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
        {
            let _entered = fixture.runtime.enter();
            let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
            assert!(
                visual.debug_bounds("native-cloud-tools-offline").is_some(),
                "the strip is drawn above the composer"
            );
            let button = visual
                .debug_bounds("native-cloud-tools-reconnect")
                .expect("the strip has a Reconnect button");
            visual.simulate_click(button.center(), gpui::Modifiers::default());
        }
        fixture.pump(cx);
        assert_eq!(
            fixture.identity_calls(),
            calls + 1,
            "Reconnect tried at once"
        );
        assert!(fixture.connected(cx));
        assert!(!fixture.strip(cx), "the strip goes once connected");
        fixture.close(cx);
    }
    /// A computer's runtime updates in place (an older computer moving to a
    /// newer runtime): the open tools connection sees a new identity and
    /// stops for good. A new connection is opened instead of leaving the
    /// tools disconnected behind a strip.
    #[gpui::test]
    fn a_runtime_updated_in_place_gets_a_new_tools_connection(cx: &mut TestAppContext) {
        let fixture = ToolsFixture::open(cx, 0);
        assert!(fixture.connected(cx));
        let first = fixture
            .view
            .read_with(cx, |view, cx| view.state.read(cx).engine().unwrap().clone());
        *fixture.service.capabilities.lock().unwrap() = vec!["runtime-0.27"];
        {
            let _entered = fixture.runtime.enter();
            fixture
                .service
                .environment
                .send_replace(json!({"state":"ready","epoch":2}));
        }
        fixture.pump(cx);
        fixture.wait(cx, 2);
        assert!(
            first.connection_stopped(),
            "the old connection refused the new identity"
        );
        assert!(fixture.connected(cx), "a new connection replaced it");
        fixture.view.read_with(cx, |view, cx| {
            let engine = view.state.read(cx).engine().unwrap();
            assert!(!engine.connection_stopped());
            assert!(engine.engine_info().supports("runtime-0.27"));
            assert!(!view.tools_offline(cx));
        });
        assert!(fixture.passive());
        fixture.close(cx);
    }
    /// A cloud chat closed while it is not in front and opened again gets a
    /// new conversation and state. Its file tab, still open beside it,
    /// follows the new state, so the file loads once that state's tools
    /// connect instead of waiting on the old state forever.
    #[gpui::test]
    fn a_reopened_cloud_chats_file_tab_follows_its_new_state(cx: &mut TestAppContext) {
        use crate::markdown::render::{LinkAction, LinkActivation, LinkOutcome, LinkTarget};
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.executor().allow_parking();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            initialize(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let target = |workspace: &str, session: &str| CloudTarget {
            controller: controller.clone(),
            subject: "owner".into(),
            organization: None,
            workspace: workspace.into(),
            initial_session: session.into(),
        };
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    account(&mut state);
                    state
                });
                let mut shell = Shell::new(state, boot(directory.path()), cx);
                shell.reduced_motion = true;
                shell
            });
            CloudChromeHost {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        let open = |workspace: &str, session: &str, cx: &mut gpui::VisualTestContext| {
            let target = target(workspace, session);
            shell.update(cx, |shell, cx| {
                shell.open_cloud_conversation(
                    target,
                    json!({"workspaceId":workspace,"initialSessionId":session}),
                    Value::Null,
                    cx,
                );
                let view = shell.cloud_conversation.clone().unwrap();
                let state = view.read(cx).state.clone();
                state.update(cx, |state, cx| {
                    state.local_device_id = Some("cloud-computer".into());
                    state.apply_chats(vec![
                        serde_json::from_value(json!({
                            "id": session, "deviceId": "cloud-computer", "archived": false,
                            "cwd": "/home/zuse/workspace", "spaceId": "space",
                            "checkoutId": "checkout", "createdAt": "2026-11-27T00:00:00Z",
                        }))
                        .unwrap(),
                    ]);
                    cx.notify();
                });
                (view, state)
            })
        };
        let (view, old_state) = open("task", "session", cx);
        let link = view
            .read_with(cx, |view, _| view.transcript_for_test().clone())
            .update(cx, |transcript, cx| transcript.link_ui(cx))
            .expect("a cloud chat's links are routed");
        let activation = LinkActivation {
            target: LinkTarget::new("chart", "/home/zuse/workspace/chart.png"),
            action: LinkAction::Primary,
            source_session: Some("session".into()),
        };
        let outcome = cx.update(|window, cx| (link.handler)(&activation, window, cx));
        assert_eq!(outcome, LinkOutcome::Internal);
        cx.run_until_parked();
        let file = shell.read_with(cx, |shell, cx| match shell.active_main_tab(cx) {
            Some(main_tabs::MainTab::File(id)) => shell.file_surfaces[&id].clone(),
            other => panic!("expected a file tab, got {other:?}"),
        });
        file.read_with(cx, |file, cx| {
            assert_eq!(file.state(), &old_state);
            assert!(file.wake_view(cx).is_some(), "waiting for the computer");
        });

        // Another chat comes to the front, then this one is opened again.
        open("other", "other-session", cx);
        drop(view);
        let (reopened, state) = open("task", "session", cx);
        cx.run_until_parked();
        assert_ne!(state, old_state, "the chat was opened afresh");
        file.read_with(cx, |file, _| assert_eq!(file.state(), &state));
        reopened.read_with(cx, |view, _| {
            assert_eq!(view.state, state);
        });

        // The new state's tools connect: the tab opens its file.
        state.update(cx, |state, cx| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zuse_rpc::memory_client(std::sync::Arc::new(PendingController)),
            ));
            cx.notify();
        });
        cx.run_until_parked();
        file.read_with(cx, |file, cx| assert_eq!(file.wake_view(cx), None));
    }
    #[gpui::test]
    fn cloud_only_chat_paints_header_logo_and_opens_right_sidebar(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            initialize(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    account(&mut state);
                    state
                });
                let mut shell = Shell::new(state, boot(directory.path()), cx);
                shell.reduced_motion = true;
                shell.viewport_width = 1400.;
                shell.viewport_height = 960.;
                shell.settings.sidebar_collapsed = false;
                assert!(shell.main_chrome_visible(cx));
                shell.open_cloud_conversation(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task".into(),
                        initial_session: "session".into(),
                    },
                    json!({"workspaceId":"task","initialSessionId":"session"}),
                    Value::Null,
                    cx,
                );
                let view = shell.cloud_conversation.as_ref().unwrap().clone();
                view.update(cx, |view, cx| {
                    let mut history = retained_history(false);
                    history["head"]["chat"]["sourceContext"] = json!({
                        "checkoutId":"task","repoRoot":"/workspace/repo","cwd":"/workspace/repo",
                        "branch":"task/cloud-chrome","observedAt":"2026-11-01T00:00:00Z"
                    });
                    view.apply_history(history, cx);
                    view.account = None;
                });
                assert!(shell.state.read(cx).selected_chat.is_none());
                assert!(shell.main_chrome_visible(cx));
                assert!(!shell.conversation_tab_row_visible(cx));
                shell
            });
            CloudChromeHost {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("workspace-app-logo").is_some());
        assert!(
            cx.debug_bounds("top-bar-branch").is_none(),
            "a cloud chat's header has no branch picker"
        );
        assert!(cx.debug_bounds("cloud-saved-branch").is_none());
        assert!(
            cx.debug_bounds("cloud-tab-icon-0").is_some(),
            "cloud tabs lead with an icon like local chat tabs"
        );
        assert!(cx.debug_bounds("native-cloud-computer").is_some());
        assert!(cx.debug_bounds("titlebar-conversation-tabs").is_some());
        assert!(cx.debug_bounds("conversation-tab-row").is_none());
        assert!(cx.debug_bounds("cloud-new-tab").is_some());
        let toggle = cx
            .debug_bounds("toggle-changes")
            .expect("cloud chat must expose its right sidebar toggle");
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(shell.read_with(cx, |shell, cx| shell.right_pane_open(cx)));
        assert!(cx.debug_bounds("surface-card-files").is_some());
        assert!(cx.debug_bounds("surface-card-terminal").is_some());
        let toggle = cx.debug_bounds("toggle-changes").unwrap();
        cx.simulate_click(toggle.center(), gpui::Modifiers::default());
        assert!(!shell.read_with(cx, |shell, cx| shell.right_pane_open(cx)));
        let view = shell.read_with(cx, |shell, _| {
            shell.cloud_conversation.as_ref().unwrap().clone()
        });
        view.update(cx, |view, cx| {
            view.config["branch"] = json!("task/private-saved-branch");
            assert!(view.branch_label().is_some());
            view.revoke(cx);
            assert!(
                view.branch_label().is_none(),
                "revocation must clear the fallback header label too"
            );
        });
    }

    struct ConversationHost {
        shell: Entity<Shell>,
        _directory: tempfile::TempDir,
    }
    impl Render for ConversationHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.shell.update(cx, |shell, cx| {
                div()
                    .size_full()
                    .flex()
                    .child(shell.render_main(window, 900., 900., cx))
            })
        }
    }
    #[gpui::test]
    fn closing_cloud_tools_returns_focus_and_typing_to_displayed_native_composer(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            initialize(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    account(&mut state);
                    state
                });
                let mut shell = Shell::new(state, boot(directory.path()), cx);
                shell.open_cloud_conversation(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task".into(),
                        initial_session: "session".into(),
                    },
                    json!({"workspaceId":"task","initialSessionId":"session"}),
                    json!({"firstMessage":"Accepted task","agent":"codex","model":"fixture"}),
                    cx,
                );
                // A focus-only state fixture, not authentication or delivery evidence.
                // Every transport remains in-process and pending; no command can execute.
                shell
                    .cloud_conversation
                    .as_ref()
                    .unwrap()
                    .update(cx, |conversation, cx| {
                        conversation.authorized = true;
                        let pickers = conversation.composer.read(cx).pickers().clone();
                        pickers.update(cx, |pickers, cx| {
                            pickers.seed_cloud_task_catalog_for_test(cx)
                        });
                    });
                shell
            });
            ConversationHost {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| {
                window.focus(&shell.navigation_focus.right, cx);
                shell.restore_right_focus_after_close(window, cx);
                assert!(
                    shell
                        .conversation_composer(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                );
                assert!(!shell.composer.focus_handle(cx).is_focused(window));
            })
        });
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_input("Continue this cloud task");
        shell.read_with(cx, |shell, cx| {
            assert_eq!(
                shell
                    .conversation_composer(cx)
                    .read(cx)
                    .input
                    .read(cx)
                    .text(),
                "Continue this cloud task"
            );
            assert_eq!(shell.composer.read(cx).input.read(cx).text(), "");
        });
    }
    #[gpui::test]
    fn files_dropped_on_a_cloud_chat_attach_to_its_displayed_composer(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(PendingController),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    account(&mut state);
                    state
                });
                let mut shell = Shell::new(state, boot(directory.path()), cx);
                shell.open_cloud_conversation(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task".into(),
                        initial_session: "session".into(),
                    },
                    json!({"workspaceId":"task","initialSessionId":"session"}),
                    json!({"firstMessage":"Accepted task","agent":"codex","model":"fixture"}),
                    cx,
                );
                shell
                    .cloud_conversation
                    .as_ref()
                    .unwrap()
                    .update(cx, |conversation, _| conversation.authorized = true);
                shell
            });
            ConversationHost {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        let files = tempfile::tempdir().unwrap();
        let mut png = std::io::Cursor::new(Vec::new());
        image::RgbImage::new(1, 1)
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        std::fs::write(files.path().join("shot.png"), png.into_inner()).unwrap();
        let to = cx.debug_bounds("chat-dropzone").unwrap().center();
        let paths = gpui::ExternalPaths(vec![files.path().join("shot.png")].into());
        cx.simulate_event(gpui::FileDropEvent::Entered {
            position: to,
            paths,
        });
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_event(gpui::FileDropEvent::Pending { position: to });
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_event(gpui::FileDropEvent::Submit { position: to });
        cx.run_until_parked();
        shell.read_with(cx, |shell, cx| {
            let names = |composer: &Entity<Composer>| {
                composer
                    .read(cx)
                    .staged()
                    .iter()
                    .map(|file| file.name.clone())
                    .collect::<Vec<_>>()
            };
            let shown = names(&shell.conversation_composer(cx));
            assert_eq!(
                shown.len(),
                1,
                "the drop lands in the composer the cloud chat shows"
            );
            assert!(shown[0].ends_with(".png"), "{shown:?}");
            assert!(
                names(&shell.composer).is_empty(),
                "not the hidden local composer"
            );
        });
    }
    struct ProvisionalHistoryController {
        fresh_failure: Option<&'static str>,
        events: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Value>>>,
        all_requests: std::sync::Mutex<Vec<(String, Value)>>,
        cache: tokio::sync::Semaphore,
        fresh: tokio::sync::Semaphore,
        requests: std::sync::Mutex<Vec<Value>>,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for ProvisionalHistoryController {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            use futures::StreamExt as _;
            self.all_requests
                .lock()
                .unwrap()
                .push((method.into(), params.clone()));
            match method {
                methods::CLOUD_ACCOUNT_STATUS => Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::once(async { json!({"user":{"id":"owner"},"orgId":null}) })
                        .chain(futures::stream::pending()),
                ))),
                methods::CLOUD_HISTORY => {
                    self.requests.lock().unwrap().push(params.clone());
                    if params["beforeSequence"].as_i64().is_some() {
                        return Err(match self.fresh_failure {
                            Some("older-permission-denied") => zuse_rpc::RpcError::Failed(
                                "You do not have access to this workspace.".into(),
                            ),
                            Some("older-transport-error") => zuse_rpc::RpcError::Transport(
                                "synthetic older-page network unavailable".into(),
                            ),
                            _ => zuse_rpc::RpcError::Failed("unexpected older request".into()),
                        });
                    }
                    let cached = params["cacheOnly"] == true;
                    if cached {
                        self.cache.acquire().await.unwrap().forget();
                    } else {
                        self.fresh.acquire().await.unwrap().forget();
                    }
                    if !cached {
                        match self.fresh_failure {
                            Some("permission-denied") => {
                                return Err(zuse_rpc::RpcError::Failed(
                                    "You do not have access to this workspace.".into(),
                                ));
                            }
                            Some("session-denied") => return Err(zuse_rpc::RpcError::Failed("Cloud could not verify your session. Reconnect in Settings → General.".into())),
                            Some("workspace-missing") => return Err(zuse_rpc::RpcError::Capability("workspace_not_found".into())),
                            Some("capability-denied") => {
                                return Err(zuse_rpc::RpcError::Capability(
                                    "workspace_access_denied".into(),
                                ));
                            }
                            Some("transport-error") => {
                                return Err(zuse_rpc::RpcError::Transport(
                                    "synthetic network unavailable".into(),
                                ));
                            }
                            _ => {}
                        }
                    }
                    let mut value = retained_history(false);
                    if matches!(
                        self.fresh_failure,
                        Some("older-permission-denied" | "older-transport-error")
                    ) {
                        value["head"]["olderMessageSequence"] = json!(1);
                    }
                    value["head"]["cursor"]["version"] = json!(if cached { 99 } else { 2 });
                    value["head"]["transcript"]["reset"][0]["id"] =
                        json!(if cached { "cached-user" } else { "fresh-user" });
                    if cached {
                        value["provisional"] = json!(true);
                        value["workspace"] = json!({"state":"ready"});
                        value["commandAdmissions"] = json!([{"id":"cached-admission"}]);
                        value["head"]["chat"]["config"] = json!({"harness":"grok","model":"grok-4.7","sandbox":"danger-full-access","modelOptions":{}});
                    }
                    Ok(zuse_rpc::RpcReply::Value(value))
                }
                methods::CLOUD_EVENTS => {
                    let receiver = self.events.lock().unwrap().take().unwrap();
                    Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                        futures::stream::unfold(receiver, |mut receiver| async {
                            receiver.recv().await.map(|value| (value, receiver))
                        }),
                    )))
                }
                methods::CLOUD_ENVIRONMENT => Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                    futures::stream::pending(),
                ))),
                _ => Err(zuse_rpc::RpcError::UnknownMethod(method.into())),
            }
        }
    }
    fn provisional_cache_fixture(cx: &mut TestAppContext, scenario: &str) {
        use std::sync::{Arc, Mutex};
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        });
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let service = Arc::new(ProvisionalHistoryController {
            events: Mutex::new(Some(events_rx)),
            all_requests: Mutex::new(vec![]),
            fresh_failure: match scenario {
                "permission-denied" => Some("permission-denied"),
                "session-denied" => Some("session-denied"),
                "capability-denied" => Some("capability-denied"),
                "workspace-missing" => Some("workspace-missing"),
                "transport-error" => Some("transport-error"),
                "older-permission-denied" => Some("older-permission-denied"),
                "older-transport-error" => Some("older-transport-error"),
                _ => None,
            },
            cache: tokio::sync::Semaphore::new(0),
            fresh: tokio::sync::Semaphore::new(0),
            requests: Mutex::new(vec![]),
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(service.clone()));
        let directory = tempfile::tempdir().unwrap();
        let view=cx.new(|cx|CloudConversation::new(CloudTarget{controller,subject:"owner".into(),organization:None,workspace:"task".into(),initial_session:"session".into()},json!({"workspaceId":"task","agent":"codex","model":"gpt-6.1-sol","state":"paused"}),Value::Null,boot(directory.path()),cx));
        view.update(cx, |view, cx| {
            view.composer.update(cx, |composer, cx| {
                composer
                    .input
                    .update(cx, |input, cx| input.set_text("Retained draft", cx))
            })
        });
        let pump = |cx: &mut TestAppContext| {
            for _ in 0..30 {
                cx.run_until_parked();
                runtime.block_on(async {
                    tokio::task::yield_now().await;
                });
            }
        };
        pump(cx);
        let requests = service.requests.lock().unwrap().clone();
        assert_eq!(
            requests.len(),
            2,
            "account authorization must race cache with exactly one fresh read"
        );
        assert_eq!(
            requests.iter().filter(|p| p["cacheOnly"] == true).count(),
            1
        );
        assert!(requests.iter().all(|p| p["workspaceId"] == "task"));
        if scenario.starts_with("events-") {
            service.cache.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                assert!(
                    view.state
                        .read(cx)
                        .transcript
                        .iter()
                        .any(|row| row.id == "cached-user")
                )
            });
            let identity = view.read_with(cx, |view, _| {
                (
                    view.state.entity_id(),
                    view.composer.entity_id(),
                    view.transcript.entity_id(),
                )
            });
            if scenario == "events-reset" {
                service.fresh.add_permits(1);
                pump(cx);
                for reason in [
                    None,
                    Some(Value::Null),
                    Some(json!("cursor_expired")),
                    Some(json!("consumer_lagged")),
                ] {
                    let mut data = json!({"type":"reset","cacheRevoked":true});
                    if let Some(reason) = reason {
                        data["reason"] = reason;
                    }
                    events_tx
                        .send(json!({"accountId":"owner","organizationId":null,"data":data}))
                        .unwrap();
                    service.fresh.add_permits(1);
                    pump(cx);
                }
                events_tx.send(json!({"accountId":"another-owner","organizationId":null,"data":{"type":"closed","cacheRevoked":true}})).unwrap();
                pump(cx);
                assert_eq!(
                    service.requests.lock().unwrap().len(),
                    6,
                    "only one fresh revalidation per known reset, no stale-scope refresh"
                );
                view.read_with(cx, |view, cx| {
                    assert!(view.authorized && !view.access_revoked);
                    assert_eq!(
                        (
                            view.state.entity_id(),
                            view.composer.entity_id(),
                            view.transcript.entity_id()
                        ),
                        identity
                    );
                    assert!(
                        view.state
                            .read(cx)
                            .transcript
                            .iter()
                            .any(|row| row.id == "fresh-user")
                    );
                    assert_eq!(
                        view.composer.read(cx).input.read(cx).text(),
                        "Retained draft"
                    );
                });
                let calls = service.all_requests.lock().unwrap();
                assert!(calls.iter().all(|(method, _)| matches!(
                    method.as_str(),
                    methods::CLOUD_ACCOUNT_STATUS
                        | methods::CLOUD_EVENTS
                        | methods::CLOUD_HISTORY
                        | methods::CLOUD_ENVIRONMENT
                )));
                let environments = calls
                    .iter()
                    .filter(|(method, _)| method == methods::CLOUD_ENVIRONMENT)
                    .collect::<Vec<_>>();
                assert_eq!(
                    environments.len(),
                    1,
                    "reset must not reopen or wake the execution connection"
                );
                assert_eq!(environments[0].1["interactiveResume"], false);
            } else {
                let data = match scenario {
                    "events-target-revoked" => {
                        json!({"type":"reset","reason":"cursor_expired","cacheRevoked":true,"revokedWorkspaceIds":["task"]})
                    }
                    "events-error" => {
                        json!({"type":"error","code":"cloud_event_access_denied","cacheRevoked":true})
                    }
                    "events-closed" => json!({"type":"closed","cacheRevoked":true}),
                    _ => panic!("unknown event fixture"),
                };
                events_tx
                    .send(json!({"accountId":"owner","organizationId":null,"data":data}))
                    .unwrap();
                pump(cx);
                view.read_with(cx, |view, cx| {
                    assert!(view.access_revoked);
                    assert!(view.state.read(cx).transcript.is_empty());
                    assert!(view.history.head.is_none());
                });
            }
            return;
        }
        if matches!(
            scenario,
            "older-permission-denied" | "older-transport-error"
        ) {
            service.cache.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                assert!(
                    view.state
                        .read(cx)
                        .transcript
                        .iter()
                        .any(|row| row.id == "cached-user")
                )
            });
            service.fresh.add_permits(1);
            pump(cx);
            view.update(cx, |view, cx| view.load_older(cx));
            pump(cx);
            let requests = service.requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            assert_eq!(
                requests.iter().filter(|p| p["beforeSequence"] == 1).count(),
                1
            );
            view.read_with(cx, |view, cx| {
                if scenario == "older-permission-denied" {
                    assert!(
                        view.access_revoked,
                        "actual older-page 403 must erase saved rows"
                    );
                    assert!(view.state.read(cx).transcript.is_empty());
                    assert!(view.history.head.is_none());
                } else {
                    assert!(!view.access_revoked);
                    assert!(
                        view.state
                            .read(cx)
                            .transcript
                            .iter()
                            .any(|row| row.id == "fresh-user")
                    );
                    assert!(view.history.head.is_some());
                    assert!(view.error.as_deref().is_some_and(|error| {
                        error.contains("Earlier history could not be loaded")
                    }));
                }
            });
            return;
        }
        if matches!(
            scenario,
            "permission-denied"
                | "session-denied"
                | "capability-denied"
                | "workspace-missing"
                | "transport-error"
        ) {
            service.cache.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                assert!(
                    view.state
                        .read(cx)
                        .transcript
                        .iter()
                        .any(|row| row.id == "cached-user")
                )
            });
            service.fresh.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                if scenario == "transport-error" {
                    assert!(!view.access_revoked);
                    assert!(
                        view.state
                            .read(cx)
                            .transcript
                            .iter()
                            .any(|row| row.id == "cached-user")
                    );
                    assert!(
                        view.error
                            .as_deref()
                            .is_some_and(|error| error.contains("network unavailable"))
                    );
                } else {
                    assert!(
                        view.access_revoked,
                        "fresh permission denial must revoke already painted saved rows"
                    );
                    assert!(view.state.read(cx).transcript.is_empty());
                    assert!(view.history.head.is_none());
                    assert!(view.history.provisional_read.is_none());
                    if scenario == "workspace-missing" {
                        assert_eq!(view.error.as_deref(), Some("This conversation is no longer available. Choose another conversation or start a new chat."));
                    }
                    assert!(service.all_requests.lock().unwrap().iter().all(|(method, _)| matches!(method.as_str(), methods::CLOUD_ACCOUNT_STATUS | methods::CLOUD_HISTORY | methods::CLOUD_EVENTS | methods::CLOUD_ENVIRONMENT)), "revocation must not admit lifecycle or inference commands");
                }
            });
            return;
        }
        if scenario == "held-fresh" {
            service.cache.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                assert!(
                    view.state
                        .read(cx)
                        .transcript
                        .iter()
                        .any(|row| row.id == "cached-user"),
                    "cached head should paint while fresh API is held"
                );
                assert!(view.state.read(cx).transcript_baseline("session").is_none());
                assert!(view.admissions.is_null());
                assert_eq!(view.config["model"], "gpt-6.1-sol");
                assert_eq!(view.computer.metadata["state"], "paused");
                assert!(view.state.read(cx).engine().is_none());
                assert_eq!(
                    view.composer.read(cx).input.read(cx).text(),
                    "Retained draft"
                );
            });
            service.fresh.add_permits(1);
            pump(cx);
        } else if scenario == "late-cache" {
            service.fresh.add_permits(1);
            pump(cx);
            service.cache.add_permits(1);
            pump(cx);
        } else {
            view.update(cx, |view, cx| view.revoke(cx));
            service.cache.add_permits(1);
            service.fresh.add_permits(1);
            pump(cx);
            view.read_with(cx, |view, cx| {
                assert!(view.access_revoked);
                assert!(view.state.read(cx).transcript.is_empty());
                assert!(view.history.head.is_none());
            });
            return;
        }
        view.read_with(cx, |view, cx| {
            let rows = &view.state.read(cx).transcript;
            assert!(rows.iter().any(|row| row.id == "fresh-user"));
            assert!(
                !rows.iter().any(|row| row.id == "cached-user"),
                "late cache must never replace fresh receipt"
            );
            assert_eq!(view.config["model"], "gpt-6.1-sol");
            assert_eq!(
                view.composer.read(cx).input.read(cx).text(),
                "Retained draft"
            );
        });
    }
    #[gpui::test]
    fn active_native_cached_history_paints_while_fresh_api_is_held(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "held-fresh");
    }
    #[gpui::test]
    fn active_native_late_cache_cannot_replace_fresh_history_or_selection(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "late-cache");
    }
    #[gpui::test]
    fn active_native_revocation_cancels_pending_provisional_history(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "revoke");
    }
    #[gpui::test]
    fn active_native_fresh_permission_denial_clears_already_painted_cache(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "permission-denied");
    }
    #[gpui::test]
    fn active_native_fresh_capability_denial_clears_already_painted_cache(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "capability-denied");
    }
    #[gpui::test]
    fn active_native_transport_failure_keeps_readable_provisional_cache(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "transport-error");
    }
    #[gpui::test]
    fn active_native_older_permission_denial_clears_saved_history(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "older-permission-denied");
    }
    #[gpui::test]
    fn active_native_older_transport_failure_keeps_saved_history(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "older-transport-error");
    }
    #[gpui::test]
    fn active_native_event_resets_revalidate_in_place_without_wake(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "events-reset");
    }
    #[gpui::test]
    fn active_native_explicit_target_revocation_overrides_cursor_reset(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "events-target-revoked");
    }
    #[gpui::test]
    fn active_native_error_event_clears_cached_history(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "events-error");
    }
    #[gpui::test]
    fn active_native_closed_event_clears_cached_history(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "events-closed");
    }
    #[gpui::test]
    fn active_native_fresh_session_denial_clears_already_painted_cache(cx: &mut TestAppContext) {
        provisional_cache_fixture(cx, "session-denied");
    }
    #[gpui::test]
    fn active_native_missing_workspace_clears_provisional_history_without_sign_in(
        cx: &mut TestAppContext,
    ) {
        provisional_cache_fixture(cx, "workspace-missing");
    }
    /// The engine's saved copy answers; the account check and the fresh read
    /// never do (still signing in, offline, or the computer is asleep).
    struct SavedCopyOnly {
        requests: std::sync::Mutex<Vec<Value>>,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for SavedCopyOnly {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            match method {
                methods::CLOUD_ACCOUNT_STATUS | methods::CLOUD_EVENTS => Ok(
                    zuse_rpc::RpcReply::Stream(Box::pin(futures::stream::pending())),
                ),
                methods::CLOUD_HISTORY => {
                    self.requests.lock().unwrap().push(params.clone());
                    if params["cacheOnly"] != true {
                        futures::future::pending::<()>().await;
                    }
                    let mut value = retained_history(false);
                    value["provisional"] = json!(true);
                    value["head"]["transcript"]["reset"][0]["id"] = json!("saved-before-restart");
                    Ok(zuse_rpc::RpcReply::Value(value))
                }
                _ => Err(zuse_rpc::RpcError::UnknownMethod(method.into())),
            }
        }
    }
    /// After a restart nothing is in memory. Opening a cloud chat paints the
    /// saved copy at once, before the account check or any fresh read, and a
    /// reopen in the same session paints from memory without a disk read.
    #[gpui::test]
    fn after_restart_the_saved_copy_paints_before_account_or_fresh_history(
        cx: &mut TestAppContext,
    ) {
        use std::sync::Arc;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        });
        let service = Arc::new(SavedCopyOnly {
            requests: std::sync::Mutex::new(vec![]),
        });
        let directory = tempfile::tempdir().unwrap();
        let open = |cx: &mut TestAppContext| {
            let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
                service.clone(),
            ));
            let boot = boot(directory.path());
            cx.new(|cx| {
                CloudConversation::new(
                    CloudTarget {
                        controller,
                        subject: "owner".into(),
                        organization: None,
                        workspace: "task".into(),
                        initial_session: "session".into(),
                    },
                    json!({"workspaceId":"task","state":"paused"}),
                    Value::Null,
                    boot,
                    cx,
                )
            })
        };
        let pump = |cx: &mut TestAppContext| {
            for _ in 0..30 {
                cx.run_until_parked();
                runtime.block_on(tokio::task::yield_now());
            }
        };
        let shown = |view: &Entity<CloudConversation>, cx: &mut TestAppContext| {
            view.read_with(cx, |view, cx| {
                view.state
                    .read(cx)
                    .transcript
                    .iter()
                    .map(|row| row.id.clone())
                    .collect::<Vec<_>>()
            })
        };

        let view = open(cx);
        pump(cx);
        assert_eq!(shown(&view, cx), ["saved-before-restart"]);
        view.read_with(cx, |view, _| {
            assert!(!view.authorized, "painted before the account check");
            assert!(!view.history.fresh_received, "painted before fresh history");
        });
        let requests = service.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 1, "{requests:?}");
        assert_eq!(requests[0]["cacheOnly"], true);

        // Close and reopen within the session: memory paints, no disk read.
        drop(view);
        cx.update(|_| {});
        pump(cx);
        let view = open(cx);
        assert_eq!(shown(&view, cx), ["saved-before-restart"]);
        pump(cx);
        assert_eq!(service.requests.lock().unwrap().len(), 1);
        drop(view);
    }
    struct AccountChanges {
        statuses: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Value>>>,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for AccountChanges {
        async fn handle(
            &self,
            method: &str,
            _params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            match method {
                methods::CLOUD_ACCOUNT_STATUS => {
                    let mut rx = self
                        .statuses
                        .lock()
                        .unwrap()
                        .take()
                        .expect("one subscription");
                    Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                        futures::stream::poll_fn(move |cx| rx.poll_recv(cx)),
                    )))
                }
                methods::CLOUD_HISTORY => Ok(zuse_rpc::RpcReply::Value(retained_history(false))),
                methods::CLOUD_EVENTS | methods::CLOUD_ENVIRONMENT => Ok(
                    zuse_rpc::RpcReply::Stream(Box::pin(futures::stream::pending())),
                ),
                _ => Err(zuse_rpc::RpcError::UnknownMethod(method.into())),
            }
        }
    }

    #[gpui::test]
    fn a_passing_account_mismatch_does_not_hide_the_composer_for_good(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        cx.update(|cx| {
            initialize(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        });
        let (statuses, rx) = tokio::sync::mpsc::unbounded_channel();
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(AccountChanges {
                statuses: std::sync::Mutex::new(Some(rx)),
            }),
        ));
        let directory = tempfile::tempdir().unwrap();
        let view = cx.new(|cx| {
            CloudConversation::new(
                CloudTarget {
                    controller,
                    subject: "owner".into(),
                    organization: None,
                    workspace: "task".into(),
                    initial_session: "session".into(),
                },
                json!({"workspaceId":"task","agent":"codex","model":"gpt-6.1-sol","runtimeMode":"full-access"}),
                Value::Null,
                boot(directory.path()),
                cx,
            )
        });
        let pump = |cx: &mut TestAppContext| {
            for _ in 0..12 {
                cx.run_until_parked();
                runtime.block_on(tokio::task::yield_now());
            }
        };
        let composer_shown = |view: &Entity<CloudConversation>, cx: &mut TestAppContext| {
            view.read_with(cx, |view, _| {
                view.authorized && view.computer.composer_enabled()
            })
        };
        // Sign-in still loading: no user yet.
        statuses.send(json!({"user":null,"orgId":null})).unwrap();
        pump(cx);
        assert!(!composer_shown(&view, cx));
        // The owner's session arrives: the chat opens.
        statuses
            .send(json!({"user":{"id":"owner"},"orgId":null}))
            .unwrap();
        pump(cx);
        assert!(composer_shown(&view, cx), "the owner can send");
        // Another account takes over: access is withdrawn...
        statuses
            .send(json!({"user":{"id":"someone-else"},"orgId":null}))
            .unwrap();
        pump(cx);
        assert!(!composer_shown(&view, cx));
        view.read_with(cx, |view, _| assert!(view.access_revoked));
        // ...and given back when the owner returns, without reopening the chat.
        statuses
            .send(json!({"user":{"id":"owner"},"orgId":null}))
            .unwrap();
        pump(cx);
        assert!(composer_shown(&view, cx), "the owner can send again");
    }
}
