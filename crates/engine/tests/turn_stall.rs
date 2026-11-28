//! Model reconnects and stalled turns, mocked against a production cloud
//! chat (2026-11-24): after "make pr", Codex lost its model stream and
//! retried on its own ("Reconnecting... 2/5"). That retry notice landed in
//! the transcript as a red Error card above "Codex is working 6m 52s", which
//! read as a dead turn.
//!
//! Under test:
//! - a transient notice rides the session row (the working row's word) and
//!   never reaches the transcript; the next streamed event clears it;
//! - when retries run out the turn ends Errored with the failure shown once,
//!   and the session leaves Working so the composer takes the next message;
//! - a driver that names a stall window has its turn ended with a visible
//!   error after that long with no event and nothing in flight — while an
//!   open tool call or a stream of reconnect notices holds it off.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::{Mutex, mpsc};

use zuse_doc::{MessagePart, MessageRole, SessionMessageEntry};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel, Session,
    SessionStatus, SteeringMode, ToolCall,
};
use zuse_providers::{Harness, HarnessError, RunControls};

const CHAT: &str = "chat-stall";
/// The feed harness's stall window.
const STALL_MS: u64 = 400;

fn run_request(prompt: &str) -> RunRequest {
    RunRequest {
        plan: false,
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

fn session_started() -> AgentEvent {
    AgentEvent::SessionStarted {
        harness: HarnessId::Mock,
        model: "mock-1".into(),
        tools: vec![],
        cwd: "/tmp".into(),
        session_id: "hs-s".into(),
        assistant_message_id: "a-s".into(),
    }
}

fn text(t: &str) -> AgentEvent {
    AgentEvent::TextDelta { text: t.into() }
}

fn notice(t: &str) -> AgentEvent {
    AgentEvent::Notice {
        message: Some(t.into()),
    }
}

/// Feed-by-hand harness with a deterministic turn end (like the native Codex
/// driver) and a short stall window. The feed is served only to the test's
/// own dispatch (matched by prompt); the auto-titler gets an instantly
/// completed empty stream.
struct FeedHarness {
    main_prompt: String,
    feed: Mutex<Option<mpsc::UnboundedReceiver<AgentEvent>>>,
}

#[async_trait]
impl Harness for FeedHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Feed"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    fn stall_timeout(&self) -> Option<Duration> {
        Some(Duration::from_millis(STALL_MS))
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        if request.prompt != self.main_prompt {
            let events = vec![Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            })];
            return Ok(futures::stream::iter(events).boxed());
        }
        let feed = self
            .feed
            .lock()
            .await
            .take()
            .expect("FeedHarness serves the main dispatch once per test");
        Ok(futures::stream::unfold(feed, |mut rx| async move {
            rx.recv().await.map(|event| (Ok(event), rx))
        })
        .boxed())
    }
}

struct Rig {
    core: EngineCore,
    feed: mpsc::UnboundedSender<AgentEvent>,
    _dir: tempfile::TempDir,
}

async fn start(prompt: &str) -> Rig {
    let (feed, rx) = mpsc::unbounded_channel();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(FeedHarness {
        main_prompt: prompt.into(),
        feed: Mutex::new(Some(rx)),
    }));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    core.sessions
        .dispatch(CHAT, HarnessId::Mock, run_request(prompt), None)
        .await
        .expect("dispatch");
    Rig {
        core,
        feed,
        _dir: dir,
    }
}

fn session(core: &EngineCore) -> Option<Session> {
    core.sessions.session_status(CHAT)
}

fn status(core: &EngineCore) -> Option<SessionStatus> {
    session(core).map(|s| s.status)
}

fn notice_of(core: &EngineCore) -> Option<String> {
    session(core).and_then(|s| s.notice)
}

/// Tolerant read: a snapshot mid-segment-write may not deserialize yet.
fn entries(core: &EngineCore) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
}

fn transcript_errors(core: &EngineCore) -> Vec<String> {
    entries(core)
        .into_iter()
        .filter(|e| e.role == MessageRole::Assistant)
        .flat_map(|e| e.parts)
        .filter_map(|p| match p {
            MessagePart::Error { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

fn transcript_text(core: &EngineCore) -> String {
    entries(core)
        .into_iter()
        .filter(|e| e.role == MessageRole::Assistant)
        .flat_map(|e| e.parts)
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

async fn wait_for<F>(mut predicate: F, what: &str)
where
    F: FnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_reconnect_notice_rides_the_working_row_and_clears_when_streaming_resumes() {
    let rig = start("make pr").await;
    rig.feed.send(session_started()).unwrap();
    rig.feed.send(text("Opening the pull request.")).unwrap();
    rig.feed
        .send(notice("Reconnecting to the model… 2/5"))
        .unwrap();
    wait_for(
        || notice_of(&rig.core).as_deref() == Some("Reconnecting to the model… 2/5"),
        "the notice on the session row",
    )
    .await;
    assert_eq!(status(&rig.core), Some(SessionStatus::Working));

    // The stream resumes: the notice clears and the turn carries on.
    rig.feed.send(text(" Done: #1.")).unwrap();
    wait_for(|| notice_of(&rig.core).is_none(), "the notice to clear").await;
    rig.feed
        .send(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some("hs-s".into()),
        })
        .unwrap();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Idle),
        "clean turn end",
    )
    .await;
    wait_for(
        || transcript_text(&rig.core).contains("Done: #1."),
        "the reply in the transcript",
    )
    .await;
    assert!(
        transcript_errors(&rig.core).is_empty(),
        "a reconnect is never a transcript error: {:?}",
        transcript_errors(&rig.core)
    );
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn exhausted_retries_end_the_turn_with_one_error_and_free_the_composer() {
    let rig = start("make pr").await;
    rig.feed.send(session_started()).unwrap();
    for attempt in 1..=5 {
        rig.feed
            .send(notice(&format!("Reconnecting to the model… {attempt}/5")))
            .unwrap();
    }
    wait_for(
        || notice_of(&rig.core).as_deref() == Some("Reconnecting to the model… 5/5"),
        "the last retry notice",
    )
    .await;
    // Codex reports the final failure, then ends the turn with the same text.
    let failure = "stream disconnected before completion: idle timeout";
    rig.feed
        .send(AgentEvent::Error {
            message: failure.into(),
        })
        .unwrap();
    rig.feed
        .send(AgentEvent::Done {
            status: DoneStatus::Errored,
            result: None,
            error: Some(failure.into()),
            session_id: Some("hs-s".into()),
        })
        .unwrap();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Errored),
        "the turn to end Errored",
    )
    .await;
    assert_eq!(notice_of(&rig.core), None, "no notice outlives its turn");
    wait_for(
        || !transcript_errors(&rig.core).is_empty(),
        "the failure in the transcript",
    )
    .await;
    assert_eq!(transcript_errors(&rig.core), vec![failure.to_owned()]);
    // Nothing is running: a new message starts a new run.
    wait_for(|| !rig.core.sessions.has_run(CHAT), "the run to be gone").await;
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_silent_turn_ends_with_a_visible_error_after_the_stall_window() {
    let rig = start("make pr").await;
    rig.feed.send(session_started()).unwrap();
    rig.feed.send(text("Thinking about it.")).unwrap();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Working),
        "the turn to start",
    )
    .await;
    // Reconnect notices are the agent talking: they hold the watchdog off.
    for _ in 0..5 {
        tokio::time::sleep(Duration::from_millis(STALL_MS / 2)).await;
        rig.feed.send(notice("Reconnecting to the model…")).unwrap();
    }
    assert_eq!(status(&rig.core), Some(SessionStatus::Working));

    // Then total silence: the turn ends Errored with a plain message.
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Errored),
        "the stalled turn to end",
    )
    .await;
    assert_eq!(notice_of(&rig.core), None);
    wait_for(
        || {
            transcript_errors(&rig.core)
                .iter()
                .any(|e| e.starts_with("Feed stopped responding"))
        },
        "the stall error in the transcript",
    )
    .await;
    assert!(transcript_text(&rig.core).contains("Thinking about it."));
    wait_for(|| !rig.core.sessions.has_run(CHAT), "the run to be gone").await;
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn an_open_tool_call_never_stalls() {
    let rig = start("run the slow build").await;
    rig.feed.send(session_started()).unwrap();
    rig.feed
        .send(AgentEvent::ToolCall {
            id: "tool-slow".into(),
            call: ToolCall::Exec {
                command: "cargo build --release".into(),
            },
        })
        .unwrap();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Working),
        "the turn to start",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(STALL_MS * 4)).await;
    assert_eq!(
        status(&rig.core),
        Some(SessionStatus::Working),
        "a long tool run is legitimate silence"
    );
    rig.feed
        .send(AgentEvent::ToolResult {
            id: "tool-slow".into(),
            is_error: false,
            output: None,
            diff: None,
        })
        .unwrap();
    rig.feed
        .send(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some("hs-s".into()),
        })
        .unwrap();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Idle),
        "clean turn end",
    )
    .await;
    assert!(transcript_errors(&rig.core).is_empty());
    rig.core.sessions.shutdown().await;
}

/// A steer boundary resets the transcript segment while a command keeps
/// running; the command still holds the watchdog off, and its late result
/// re-arms it.
#[tokio::test]
async fn a_tool_call_open_across_a_steer_boundary_never_stalls() {
    let rig = start("run the slow build").await;
    rig.feed.send(session_started()).unwrap();
    rig.feed
        .send(AgentEvent::ToolCall {
            id: "tool-slow".into(),
            call: ToolCall::Exec {
                command: "cargo build --release".into(),
            },
        })
        .unwrap();
    rig.feed
        .send(AgentEvent::Steered {
            assistant_message_id: None,
            next_assistant_message_id: None,
            message_id: None,
        })
        .unwrap();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Working),
        "the turn to start",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(STALL_MS * 4)).await;
    assert_eq!(
        status(&rig.core),
        Some(SessionStatus::Working),
        "the command outlives the segment, and so does its hold"
    );
    // The result lands (a stale echo for the new segment): the next silence
    // is a stall again.
    rig.feed
        .send(AgentEvent::ToolResult {
            id: "tool-slow".into(),
            is_error: false,
            output: None,
            diff: None,
        })
        .unwrap();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Errored),
        "the silent turn to end after the command finished",
    )
    .await;
    rig.core.sessions.shutdown().await;
}
