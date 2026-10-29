//! A background workflow run reports progress after its turn has parked:
//! the snapshot must land on the run's chip in the finished entry without
//! un-parking the chat, and the run settles through a tagged Done.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::{Mutex, mpsc};

use zuse_doc::{MessagePart, SessionMessageEntry, SubagentStatus};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode, ToolCall, WorkflowAgentProgress, WorkflowAgentState,
    WorkflowPhase, WorkflowProgress,
};
use zuse_providers::{Harness, HarnessError, RunControls};

const CHAT: &str = "chat-workflow-progress";
const RUN: &str = "toolu_workflow";

fn run_request(prompt: &str) -> RunRequest {
    RunRequest {
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

fn done(status: DoneStatus) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: None,
        session_id: Some("hs-r".into()),
    }
}

fn tagged(event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: RUN.into(),
        event: Box::new(event),
    }
}

/// Feed-by-hand harness (see turn_quiesce.rs): the test pushes events through
/// a channel. The stream ends when the engine cancels the run, like a reaped
/// child's stdout closing.
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
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        if request.prompt != self.main_prompt {
            let events = vec![Ok(done(DoneStatus::Completed))];
            return Ok(futures::stream::iter(events).boxed());
        }
        let mut feed = self
            .feed
            .lock()
            .await
            .take()
            .expect("FeedHarness serves the main dispatch once per test");
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        let cancel = controls.interrupt.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    event = feed.recv() => match event {
                        Some(event) => {
                            if tx.send(Ok(event)).await.is_err() {
                                return;
                            }
                        }
                        None => return,
                    },
                }
            }
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}

fn entries(core: &EngineCore) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
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

fn chip(core: &EngineCore) -> Option<(ToolCall, Option<SubagentStatus>)> {
    entries(core)
        .iter()
        .flat_map(|e| &e.parts)
        .find_map(|p| match p {
            MessagePart::Tool {
                id,
                call,
                subagent_status,
                ..
            } if id == RUN => Some((call.clone(), *subagent_status)),
            _ => None,
        })
}

fn snapshot(state: WorkflowAgentState) -> WorkflowProgress {
    WorkflowProgress {
        phases: vec![WorkflowPhase {
            index: 1,
            title: "Map".into(),
        }],
        agents: vec![WorkflowAgentProgress {
            index: 1,
            label: "map:files".into(),
            state,
            phase_index: Some(1),
            phase_title: Some("Map".into()),
            model: None,
            started_at: Some(100),
            duration_ms: None,
            prompt_preview: Some("List files".into()),
            result_preview: None,
            error: None,
        }],
    }
}

fn refresh(state: WorkflowAgentState) -> AgentEvent {
    AgentEvent::WorkflowProgress {
        tool_use_id: RUN.into(),
        call: ToolCall::workflow_run(
            Some(&serde_json::json!({ "script": "export const meta = {}" })),
            Some("audit"),
            Some(&snapshot(state)),
        ),
    }
}

#[tokio::test]
async fn parked_workflow_progress_updates_its_chip_and_settles() {
    let (feed, rx) = mpsc::unbounded_channel();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(FeedHarness {
        main_prompt: "run the audit".into(),
        feed: Mutex::new(Some(rx)),
    }));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    core.sessions
        .dispatch(CHAT, HarnessId::Mock, run_request("run the audit"), None)
        .await
        .expect("dispatch");
    feed.send(AgentEvent::SessionStarted {
        harness: HarnessId::Mock,
        model: "mock-1".into(),
        tools: vec![],
        cwd: "/tmp".into(),
        session_id: "hs-w".into(),
        assistant_message_id: "a-w".into(),
    })
    .unwrap();
    feed.send(AgentEvent::ToolCall {
        id: RUN.into(),
        call: ToolCall::workflow_run(
            Some(&serde_json::json!({ "script": "export const meta = {}" })),
            None,
            None,
        ),
    })
    .unwrap();
    feed.send(AgentEvent::ToolResult {
        id: RUN.into(),
        is_error: false,
        output: Some("Workflow started in the background".into()),
        diff: None,
    })
    .unwrap();
    feed.send(done(DoneStatus::Completed)).unwrap();
    wait_for(
        || {
            core.sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == SessionStatus::Idle)
        },
        "park after Done",
    )
    .await;
    let entry_count = entries(&core).len();

    // Progress after the park: the finished entry's chip refreshes in place.
    feed.send(refresh(WorkflowAgentState::Running)).unwrap();
    wait_for(
        || chip(&core).is_some_and(|(call, _)| call.workflow_progress().is_some()),
        "progress on the parked chip",
    )
    .await;
    let (call, status) = chip(&core).unwrap();
    assert_eq!(call.workflow_name(), Some("audit"));
    assert_eq!(
        call.workflow_progress(),
        Some(snapshot(WorkflowAgentState::Running))
    );
    assert_eq!(status, Some(SubagentStatus::Running));
    // The script stays in the run journal, never the doc.
    assert!(
        !serde_json::to_string(&call)
            .unwrap()
            .contains("export const meta")
    );
    // Background progress neither re-opens the turn nor adds an entry.
    assert_eq!(
        core.sessions.session_status(CHAT).map(|s| s.status),
        Some(SessionStatus::Idle)
    );
    assert_eq!(entries(&core).len(), entry_count);

    // The final snapshot lands with the settle, not a window later.
    feed.send(refresh(WorkflowAgentState::Done)).unwrap();
    feed.send(tagged(done(DoneStatus::Completed))).unwrap();
    wait_for(
        || chip(&core).is_some_and(|(_, status)| status == Some(SubagentStatus::Done)),
        "workflow chip to settle",
    )
    .await;
    let (call, _) = chip(&core).unwrap();
    assert_eq!(
        call.workflow_progress(),
        Some(snapshot(WorkflowAgentState::Done))
    );
    assert_eq!(entries(&core).len(), entry_count);

    core.sessions.shutdown().await;
}
