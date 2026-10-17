use super::host::lock;
use super::*;
use futures::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, mpsc, oneshot};
use zuse_harness::{Call, Message, Role, Usage};
use zuse_protocol::{DoneStatus, McpServer, SandboxLevel};
use zuse_providers::{SteerMessage, TurnControl};
#[derive(Clone)]
struct Seen {
    root: String,
    agent: String,
    model: String,
    tools: Vec<String>,
    messages: Vec<Message>,
}
struct ModelFixture {
    replies: Mutex<VecDeque<ModelOutput>>,
    seen: Mutex<Vec<Seen>>,
    block_root: AtomicBool,
    block_child: AtomicBool,
    blocked: Notify,
    release: Notify,
    removed: Notify,
}
fn output(text: &str, calls: Vec<Call>) -> ModelOutput {
    let mut parts = vec![json!({"type":"text","text":text})];
    parts.extend(calls.iter().map(|call| json!({"type":"tool-call","toolCallId":call.id,"toolName":call.name,"input":call.input})));
    ModelOutput {
        messages: vec![Message {
            role: Role::Assistant,
            content: Value::Array(parts),
            metadata: BTreeMap::new(),
        }],
        calls,
        text: text.into(),
        usage: Usage {
            input_tokens: 20,
            output_tokens: 5,
            ..Usage::default()
        },
    }
}
fn call(id: &str, name: &str, input: Value) -> Call {
    Call {
        id: id.into(),
        name: name.into(),
        input,
    }
}
impl ModelFixture {
    fn new(replies: Vec<ModelOutput>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into()),
            seen: Mutex::new(vec![]),
            block_root: AtomicBool::new(false),
            block_child: AtomicBool::new(false),
            blocked: Notify::new(),
            release: Notify::new(),
            removed: Notify::new(),
        })
    }
}
#[async_trait]
impl Models for ModelFixture {
    async fn inventory(&self, _: bool, _: &CancellationToken) -> Result<Vec<HarnessModel>, Error> {
        Ok(["chatgpt/fixture", "chatgpt/next"]
            .into_iter()
            .map(|id| HarnessModel {
                id: id.into(),
                label: id.into(),
                context_window_tokens: Some(100000),
                auto_compact_token_limit: None,
                effective_context_window_percent: Some(95.5),
            })
            .collect())
    }
    async fn request(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        let child = input.root_id != input.agent_id;
        lock(&self.seen).push(Seen {
            root: input.root_id.clone(),
            agent: input.agent_id.clone(),
            model: input.model.clone(),
            tools: input.tools.keys().cloned().collect(),
            messages: input.messages.clone(),
        });
        if self.block_root.load(Ordering::Acquire) && !child
            || self.block_child.load(Ordering::Acquire) && child
        {
            (input.text)("working".into());
            self.blocked.notify_one();
            self.release.notified().await;
        }
        let output = if child {
            output("child completed", vec![])
        } else {
            lock(&self.replies)
                .pop_front()
                .unwrap_or_else(|| output("continued", vec![]))
        };
        (input.text)(output.text.clone());
        Ok(output)
    }
    fn remove_root(&self, _: &str) {
        self.removed.notify_one();
    }
}
fn harness(root: &std::path::Path, fixture: Arc<ModelFixture>) -> NativeHarness {
    NativeHarness {
        profile: root.join("profile"),
        device_id: "device".into(),
        models: fixture,
        command_host: std::env::current_exe().unwrap(),
        uploads: crate::Uploads::from_root(&root.join("uploads")),
    }
}
fn request(root: &std::path::Path) -> RunRequest {
    RunRequest {
        prompt: "Implement this".into(),
        harness: Some(HarnessId::Zuse),
        model: Some("chatgpt/fixture".into()),
        reasoning: None,
        model_options: Default::default(),
        cwd: root.to_string_lossy().into_owned(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
    }
}
fn run_controls() -> (
    RunControls,
    mpsc::Sender<SteerMessage>,
    Arc<Mutex<Vec<zuse_protocol::UserInputQuestion>>>,
) {
    let (tx, rx) = mpsc::channel(32);
    let questions = Arc::new(Mutex::new(vec![]));
    let captured = questions.clone();
    (
        RunControls {
            realtime: None,
            execution_lease: None,
            request_input: Box::new(move |questions| {
                lock(&captured).extend(questions.clone());
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(
                    questions
                        .into_iter()
                        .map(|q| zuse_protocol::UserInputAnswer {
                            question_id: q.id,
                            labels: vec!["Allow once".into()],
                        })
                        .collect(),
                );
                rx
            }),
            steering: rx,
            interrupt: CancellationToken::new(),
            turn: TurnControl::default(),
        },
        tx,
        questions,
    )
}
async fn turn(
    stream: &mut BoxStream<'static, Result<AgentEvent, HarnessError>>,
) -> Vec<AgentEvent> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = vec![];
        loop {
            let event = stream
                .next()
                .await
                .expect("session stream ended")
                .expect("session failed");
            let done = matches!(event, AgentEvent::Done { .. });
            events.push(event);
            if done {
                return events;
            }
        }
    })
    .await
    .unwrap()
}
async fn retired(fixture: &ModelFixture) {
    tokio::time::timeout(Duration::from_secs(5), fixture.removed.notified())
        .await
        .unwrap();
}
#[tokio::test]
async fn native_session_streams_real_file_tools_children_attachments_steering_and_resume() {
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("seed.txt"), "before").unwrap();
    let hash = format!("{:x}", Sha256::digest(b"before"));
    let fixture = ModelFixture::new(vec![
        output(
            "inspect",
            vec![call("read", "read_file", json!({"path":"seed.txt"}))],
        ),
        output(
            "edit",
            vec![call(
                "write",
                "write_file",
                json!({"path":"seed.txt","content":"after","expected_hash":hash}),
            )],
        ),
        output(
            "delegate",
            vec![
                call(
                    "spawn-a",
                    "spawn_agent",
                    json!({"name":"a","message":"Inspect A","inherit":false}),
                ),
                call(
                    "spawn-b",
                    "spawn_agent",
                    json!({"name":"b","message":"Inspect B","inherit":false}),
                ),
            ],
        ),
        output("wait", vec![]),
        output("finished", vec![]),
    ]);
    let harness = harness(root.path(), fixture.clone());
    let mut request = request(root.path());
    let image = root.path().join("image.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nfixture").unwrap();
    request
        .attachments
        .push(image.to_string_lossy().into_owned());
    request.mcp = Some(McpServer {
        name: "zuse".into(),
        env: [
            ("ZUSE_IPC_PORT".into(), "9".into()),
            ("ZUSE_CHAT_ID".into(), "parent-chat".into()),
            ("ZUSE_DEVICE_ID".into(), "device".into()),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    });
    let (controls, steering, questions) = run_controls();
    let interrupt = controls.interrupt.clone();
    let mut stream = harness.run(request.clone(), controls).await.unwrap();
    let events = turn(&mut stream).await;
    assert_eq!(
        std::fs::read_to_string(root.path().join("seed.txt")).unwrap(),
        "after"
    );
    assert_eq!(lock(&questions).len(), 1);
    assert!(lock(&questions)[0].question.contains("write_file"));
    let session = events
        .iter()
        .find_map(|event| {
            if let AgentEvent::SessionStarted {
                harness: HarnessId::Zuse,
                session_id,
                ..
            } = event
            {
                Some(session_id.clone())
            } else {
                None
            }
        })
        .unwrap();
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolCall {
            call: zuse_protocol::ToolCall::WriteFile { .. },
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::AssistantMessageCompleted { .. }))
    );
    for spawn in ["spawn-a", "spawn-b"] {
        assert!(events.iter().any(|event| matches!(event, AgentEvent::Subagent { parent_tool_use_id, event } if parent_tool_use_id == spawn && matches!(event.as_ref(), AgentEvent::SessionStarted { .. }))), "missing {spawn}: {events:?}");
        assert!(events.iter().any(|event| matches!(event, AgentEvent::Subagent { parent_tool_use_id, event } if parent_tool_use_id == spawn && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. }))));
    }
    let seen = lock(&fixture.seen).clone();
    assert!(
        seen.iter()
            .all(|input| input.tools.contains(&"zuse_plugins_call".into()))
    );
    assert!(seen.iter().any(|input| input.root != input.agent));
    assert!(
        seen[0]
            .messages
            .iter()
            .any(
                |message| message.content.as_array().is_some_and(|parts| parts
                    .iter()
                    .any(|part| part["type"] == "image" && part["mediaType"] == "image/png"))
            )
    );
    steering
        .send(SteerMessage {
            prompt: "Next constraint".into(),
            message_id: Some("delivery".into()),
            attachments: vec![],
            config: None,
        })
        .await
        .unwrap();
    let next = turn(&mut stream).await;
    assert_eq!(
        next.iter()
            .filter(|event| matches!(event, AgentEvent::Steered { .. }))
            .count(),
        1
    );
    interrupt.cancel();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .is_none(),
        "retiring an idle runtime must not publish another turn completion"
    );
    drop(stream);
    retired(&fixture).await;
    request.resume = Some(session);
    request.attachments.clear();
    let (controls, _steering, _) = run_controls();
    let mut resumed = harness.run(request, controls).await.unwrap();
    assert!(matches!(
        turn(&mut resumed).await.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
    assert!(
        lock(&fixture.seen)
            .last()
            .unwrap()
            .messages
            .iter()
            .any(|message| message.content == "Next constraint")
    );
    drop(resumed);
    retired(&fixture).await;
}
#[tokio::test]
async fn session_stop_keeps_child_work_and_adopts_the_next_prompt_configuration() {
    let root = tempfile::tempdir().unwrap();
    let fixture = ModelFixture::new(vec![
        output(
            "delegate",
            vec![call(
                "spawn",
                "spawn_agent",
                json!({"name":"worker","message":"continue","inherit":false}),
            )],
        ),
        output("waiting", vec![]),
    ]);
    fixture.block_child.store(true, Ordering::Release);
    let harness = harness(root.path(), fixture.clone());
    let request = request(root.path());
    let (controls, steering, _) = run_controls();
    let turn_control = controls.turn.clone();
    let mut stream = harness.run(request.clone(), controls).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), fixture.blocked.notified())
        .await
        .unwrap();
    turn_control.stop_turn();
    let stopped = turn(&mut stream).await;
    assert!(matches!(
        stopped.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            ..
        })
    ));
    assert!(turn_control.background_live());
    let config = RunRequest {
        model: Some("chatgpt/next".into()),
        reasoning: Some(ReasoningLevel::High),
        ..request
    };
    steering
        .send(SteerMessage {
            prompt: "Continue root".into(),
            message_id: Some("next".into()),
            config: Some(Box::new(config)),
            attachments: vec![],
        })
        .await
        .unwrap();
    fixture.release.notify_one();
    let next = turn(&mut stream).await;
    assert!(matches!(
        next.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
    assert!(
        next.iter()
            .any(|event| matches!(event, AgentEvent::Steered { .. }))
    );
    assert!(
        lock(&fixture.seen)
            .iter()
            .any(|input| input.model == "chatgpt/next")
    );
    drop(stream);
    retired(&fixture).await;
}
#[tokio::test]
async fn resume_rejects_missing_journals_and_foreign_provider_cursors() {
    let root = tempfile::tempdir().unwrap();
    let fixture = ModelFixture::new(vec![]);
    let harness = harness(root.path(), fixture);
    for cursor in [
        "foreign-session".into(),
        format!("zuse:{}", uuid::Uuid::new_v4()),
    ] {
        let request = RunRequest {
            resume: Some(cursor),
            ..request(root.path())
        };
        let (controls, _, _) = run_controls();
        assert!(harness.run(request, controls).await.is_err());
    }
}
#[tokio::test]
async fn application_tools_retain_host_identity_and_read_only_policy() {
    let root = tempfile::tempdir().unwrap();
    let mut request = request(root.path());
    request.mcp = Some(McpServer {
        name: "zuse".into(),
        env: [
            ("ZUSE_IPC_PORT".into(), "9".into()),
            ("ZUSE_CHAT_ID".into(), "parent-chat".into()),
            ("ZUSE_DEVICE_ID".into(), "device".into()),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    });
    assert!(host::tools(&request, "other-device").is_err());
    let (mut controls, _, questions) = run_controls();
    let (tx, _) = mpsc::channel(10);
    let app = host::Application {
        models: ModelFixture::new(vec![]),
        events: tx,
        stop: CancellationToken::new(),
        overflow: AtomicBool::new(false),
        callbacks: host::callbacks(&mut controls),
        mode: Arc::new(Mutex::new(
            zuse_harness::native_tools::permission::RuntimeMode::FullAccess,
        )),
        tools: host::tools(&request, "device").unwrap(),
        cwd: root.path().into(),
        uploads: crate::Uploads::from_root(&root.path().join("uploads")),
    };
    use zuse_harness::Host;
    let context = zuse_harness::ToolContext {
        agent_id: "child".into(),
        call_id: "call".into(),
        cancel: CancellationToken::new(),
        permission_mode: zuse_harness::PermissionMode::Default,
        read_only: true,
    };
    let result = app
        .execute(
            "zuse_plugins_call",
            json!({"chatId":"override","address":"mail/send","arguments":{}}),
            context,
        )
        .await;
    assert!(matches!(result, Err(Error::Tool(message)) if message.contains("read-only")));
    assert!(lock(&questions).is_empty());
    let path = app.uploads.dir().join("notes.txt");
    std::fs::create_dir_all(app.uploads.dir()).unwrap();
    std::fs::write(&path, "uploaded notes").unwrap();
    let context = zuse_harness::ToolContext {
        agent_id: "child".into(),
        call_id: "upload".into(),
        cancel: CancellationToken::new(),
        permission_mode: zuse_harness::PermissionMode::Plan,
        read_only: true,
    };
    let result = app
        .execute("zuse_read_upload", json!({"path":path}), context.clone())
        .await
        .unwrap();
    assert!(
        matches!(result, zuse_harness::ToolOutput::Text(text) if text.contains("uploaded notes"))
    );
    std::fs::write(root.path().join("outside.txt"), "outside").unwrap();
    assert!(
        app.execute(
            "zuse_read_upload",
            json!({"path":root.path().join("outside.txt")}),
            context
        )
        .await
        .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_before_turn_admission_cannot_start_model_work_and_the_runtime_can_continue() {
    let root = tempfile::tempdir().unwrap();
    let fixture = ModelFixture::new(vec![]);
    let harness = harness(root.path(), fixture.clone());
    let (controls, steering, _) = run_controls();
    controls.turn.stop_turn();
    let mut stream = harness.run(request(root.path()), controls).await.unwrap();
    let stopped = turn(&mut stream).await;
    assert!(matches!(
        stopped.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            ..
        })
    ));
    assert!(lock(&fixture.seen).is_empty());
    steering
        .send(SteerMessage::text("Continue deliberately"))
        .await
        .unwrap();
    let next = turn(&mut stream).await;
    assert!(matches!(
        next.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
    drop(stream);
    retired(&fixture).await;
}

#[tokio::test]
async fn late_cancelled_delivery_cannot_change_configuration_or_reappear_after_stop() {
    let root = tempfile::tempdir().unwrap();
    let fixture = ModelFixture::new(vec![]);
    fixture.block_root.store(true, Ordering::Release);
    let harness = harness(root.path(), fixture.clone());
    let initial = request(root.path());
    let (controls, steering, _) = run_controls();
    let control = controls.turn.clone();
    let mut stream = harness.run(initial.clone(), controls).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), fixture.blocked.notified())
        .await
        .unwrap();
    control.register_steer("old");
    control.stop_turn();
    let stopped = turn(&mut stream).await;
    assert!(matches!(
        stopped.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            ..
        })
    ));
    fixture.block_root.store(false, Ordering::Release);
    steering
        .send(SteerMessage {
            prompt: "cancelled constraint".into(),
            message_id: Some("old".into()),
            config: Some(Box::new(RunRequest {
                model: Some("chatgpt/next".into()),
                ..initial
            })),
            attachments: vec![],
        })
        .await
        .unwrap();
    control.register_steer("new");
    steering
        .send(SteerMessage {
            prompt: "deliberate continuation".into(),
            message_id: Some("new".into()),
            config: None,
            attachments: vec![],
        })
        .await
        .unwrap();
    let events = turn(&mut stream).await;
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, AgentEvent::Steered { .. }))
            .count(),
        1
    );
    {
        let seen = lock(&fixture.seen);
        let last = seen.last().unwrap();
        assert_eq!(last.model, "chatgpt/fixture");
        let messages = serde_json::to_string(&last.messages).unwrap();
        assert!(messages.contains("deliberate continuation"));
        assert!(!messages.contains("cancelled constraint"));
    }
    drop(stream);
    retired(&fixture).await;
}

async fn doc_queue_stop_case(hard_stop: bool) {
    use sha2::{Digest, Sha256};
    use zuse_doc::SessionCommandPayload;
    let root = tempfile::tempdir().unwrap();
    let fixture = ModelFixture::new(vec![]);
    fixture.block_root.store(true, Ordering::Release);
    let mut native = harness(root.path(), fixture.clone());
    let profile = native.profile.clone();
    let registry = Arc::new(crate::HarnessRegistry::new());
    let core = crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(&root.path().join("engine")).unwrap(),
        registry.clone(),
        HarnessId::Zuse,
        None,
    )
    .unwrap();
    native.device_id = core.device_id.clone();
    registry.register(Arc::new(native));
    core.workspace
        .create_chat(
            "chat",
            None,
            Some(&core.device_id),
            None,
            Some(root.path().to_string_lossy().into_owned()),
        )
        .unwrap();
    let mut request = request(root.path());
    request.auto_approve = true;
    core.doc_host
        .queue_command(
            "chat",
            SessionCommandPayload::Run {
                request: request.clone(),
                message_id: "first".into(),
            },
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), fixture.blocked.notified())
        .await
        .unwrap();
    let id = core
        .doc_host
        .queue_message_with_behavior("chat", "cancelled constraint", vec![], true)
        .unwrap();
    assert!(core.doc_host.steer_queued_now("chat", &id).await.unwrap());
    let root_id = lock(&fixture.seen)[0].root.clone();
    let journal = profile
        .join("harness")
        .join(format!("{:x}", Sha256::digest(root_id.as_bytes())))
        .join("checkpoint.json");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state: zuse_harness::Checkpoint =
                serde_json::from_slice(&tokio::fs::read(&journal).await.unwrap()).unwrap();
            if state
                .agents
                .iter()
                .any(|agent| agent.mailbox.iter().any(|mail| mail.id == id))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(if hard_stop {
        core.sessions.interrupt("chat").await.unwrap()
    } else {
        core.sessions.stop_turn("chat").await.unwrap()
    });
    let state: zuse_harness::Checkpoint =
        serde_json::from_slice(&tokio::fs::read(&journal).await.unwrap()).unwrap();
    assert!(state.cancelled_notifications.contains(&id));
    assert!(
        state
            .agents
            .iter()
            .all(|agent| agent.mailbox.iter().all(|mail| mail.id != id))
    );
    fixture.block_root.store(false, Ordering::Release);
    request.prompt = "deliberate continuation".into();
    core.doc_host
        .queue_command(
            "chat",
            SessionCommandPayload::Run {
                request,
                message_id: "second".into(),
            },
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if lock(&fixture.seen).len() >= 2
                && core
                    .sessions
                    .session_status("chat")
                    .is_some_and(|s| s.status == zuse_protocol::SessionStatus::Idle)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    {
        let seen = lock(&fixture.seen);
        let last = seen.last().unwrap();
        assert_eq!(last.root, root_id);
        let messages = serde_json::to_string(&last.messages).unwrap();
        assert!(messages.contains("deliberate continuation"));
        assert!(!messages.contains("cancelled constraint"));
    }
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_queue_steering_and_stop_share_the_native_durable_delivery_boundary() {
    doc_queue_stop_case(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hard_stop_persists_steering_cancellation_before_native_runtime_retirement() {
    doc_queue_stop_case(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switching_from_an_opaque_provider_cursor_starts_a_native_session_with_transcript_context()
{
    use zuse_doc::{MessagePart, MessageRole, SessionMessageEntry};
    let root = tempfile::tempdir().unwrap();
    let fixture = ModelFixture::new(vec![]);
    let mut native = harness(root.path(), fixture.clone());
    let registry = Arc::new(crate::HarnessRegistry::new());
    let core = crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(&root.path().join("engine")).unwrap(),
        registry.clone(),
        HarnessId::Zuse,
        None,
    )
    .unwrap();
    native.device_id = core.device_id.clone();
    registry.register(Arc::new(native));
    core.workspace
        .create_chat(
            "migration",
            None,
            Some(&core.device_id),
            None,
            Some(root.path().to_string_lossy().into_owned()),
        )
        .unwrap();
    core.workspace.set_chat_harness_session(
        "migration",
        "foreign-provider-cursor",
        &root.path().to_string_lossy(),
    );
    let handle = core.doc_host.open("migration").unwrap();
    for (id, role, text) in [
        (
            "u1",
            MessageRole::User,
            "Original request: preserve the existing file.",
        ),
        (
            "a1",
            MessageRole::Assistant,
            "The original provider already inspected it.",
        ),
        ("current", MessageRole::User, "Continue with Zuse"),
        ("future", MessageRole::User, "future pending instruction"),
    ] {
        let mut parts = vec![MessagePart::Text {
            id: format!("{id}-text"),
            text: text.into(),
        }];
        if id == "a1" {
            parts.push(MessagePart::Image {
                id: "previous-image".into(),
                path: "/uploads/old-evidence.png".into(),
                name: "evidence.png".into(),
                mime_type: "image/png".into(),
            });
            parts.push(serde_json::from_value(json!({"kind":"tool","id":"old-tool","call": zuse_protocol::ToolCall::Unknown {name:"inspected_file".into(),input:Some(json!({"path":"existing.txt"}))},"resolved":true,"isError":false,"output":"already inspected","outputRef":"migration/old-tool"})).unwrap());
            parts.push(MessagePart::Error {
                id: "old-error".into(),
                message: "Historical failure preserved".into(),
            });
        }
        handle
            .doc()
            .push_message(&SessionMessageEntry {
                id: id.into(),
                role,
                parts,
                created_at: 0,
                device_id: core.device_id.clone(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
    }
    let mut request = request(root.path());
    request.prompt = "Continue with Zuse".into();
    request.auto_approve = true;
    core.sessions
        .dispatch(
            "migration",
            HarnessId::Zuse,
            request,
            Some("current".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if core
                .sessions
                .session_status("migration")
                .is_some_and(|s| s.status == zuse_protocol::SessionStatus::Idle)
                && !lock(&fixture.seen).is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    {
        let seen = lock(&fixture.seen);
        let first = seen.first().unwrap();
        assert!(first.root.starts_with("zuse:"));
        let history = serde_json::to_string(&first.messages).unwrap();
        assert!(history.contains("Original request: preserve the existing file."));
        assert!(history.contains("The original provider already inspected it."));
        assert_eq!(history.matches("Continue with Zuse").count(), 1);
        assert!(!history.contains("future pending instruction"));
        assert!(history.contains("old-evidence.png"));
        assert!(history.contains("resolved=true"));
        assert!(history.contains("migration/old-tool"));
        assert!(history.contains("Historical failure preserved"));
    }
    assert!(
        core.workspace
            .chat_harness_session("migration")
            .unwrap()
            .0
            .starts_with("zuse:")
    );
    core.shutdown().await;
}

#[tokio::test]
async fn consumed_delivery_replay_only_acknowledges_in_live_and_reopened_runtimes() {
    for reopen in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let fixture = ModelFixture::new(vec![]);
        let harness = harness(root.path(), fixture.clone());
        let initial = request(root.path());
        let (controls, mut steering, _) = run_controls();
        let mut stream = harness.run(initial.clone(), controls).await.unwrap();
        let events = turn(&mut stream).await;
        let session = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::SessionStarted { session_id, .. } => Some(session_id.clone()),
                _ => None,
            })
            .unwrap();
        steering
            .send(SteerMessage {
                prompt: "durable constraint".into(),
                message_id: Some("durable-delivery".into()),
                attachments: vec![],
                config: None,
            })
            .await
            .unwrap();
        let consumed = turn(&mut stream).await;
        assert!(
            consumed
                .iter()
                .any(|event| matches!(event, AgentEvent::Steered { .. }))
        );
        if reopen {
            drop(stream);
            retired(&fixture).await;
            let (controls, sender, _) = run_controls();
            steering = sender;
            stream = harness
                .run(
                    RunRequest {
                        resume: Some(session),
                        prompt: "Resume deliberately".into(),
                        ..initial.clone()
                    },
                    controls,
                )
                .await
                .unwrap();
            turn(&mut stream).await;
        }
        let requests_before = lock(&fixture.seen).len();
        // A replay must not even resolve stale attachments or apply its old
        // configuration. Only the persisted consumption acknowledgement repeats.
        steering
            .send(SteerMessage {
                prompt: "must never enter history".into(),
                message_id: Some("durable-delivery".into()),
                attachments: vec![
                    root.path()
                        .join("removed-image.png")
                        .to_string_lossy()
                        .into(),
                ],
                config: Some(Box::new(RunRequest {
                    model: Some("chatgpt/next".into()),
                    ..initial
                })),
            })
            .await
            .unwrap();
        let acknowledged = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            matches!(acknowledged, AgentEvent::DeliveryAcknowledged { ref message_id } if message_id == "durable-delivery"),
            "{acknowledged:?}"
        );
        assert_eq!(lock(&fixture.seen).len(), requests_before);
        steering
            .send(SteerMessage::text("A new deliberate turn"))
            .await
            .unwrap();
        let next = turn(&mut stream).await;
        assert!(matches!(
            next.last(),
            Some(AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            })
        ));
        {
            let seen = lock(&fixture.seen);
            assert_eq!(seen.len(), requests_before + 1);
            assert_eq!(seen.last().unwrap().model, "chatgpt/fixture");
            let messages = serde_json::to_string(&seen.last().unwrap().messages).unwrap();
            assert_eq!(messages.matches("durable constraint").count(), 1);
            assert!(!messages.contains("must never enter history"));
            assert!(messages.contains("A new deliberate turn"));
        }
        drop(stream);
        retired(&fixture).await;
    }
}

#[test]
fn replay_receipt_preserves_the_streamed_assistant_segment() {
    use events::Mapper;
    let mut mapper = Mapper::new("root".into(), "/workspace".into(), "session".into());
    let route = zuse_harness::AgentRoute {
        parents: vec![],
        model: "model".into(),
    };
    mapper.translate(
        zuse_harness::Event::Text {
            agent_id: "root".into(),
            item_id: "streaming-item".into(),
            text: "In progress".into(),
        },
        &route,
        false,
    );
    mapper.replays.insert("old".into());
    let receipt = mapper.translate(
        zuse_harness::Event::MailConsumed {
            agent_id: "root".into(),
            delivery_id: "old".into(),
        },
        &route,
        false,
    );
    assert!(
        matches!(&receipt[..], [AgentEvent::DeliveryAcknowledged { message_id }] if message_id == "old")
    );
    mapper.steers.insert("new".into());
    let boundary = mapper.translate(
        zuse_harness::Event::MailConsumed {
            agent_id: "root".into(),
            delivery_id: "new".into(),
        },
        &route,
        false,
    );
    assert!(
        matches!(&boundary[..], [AgentEvent::Steered { assistant_message_id: Some(id), .. }] if id == "streaming-item")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn replay_receipt_retires_the_host_ledger_without_changing_its_idle_route() {
    let root = tempfile::tempdir().unwrap();
    let fixture = ModelFixture::new(vec![]);
    let mut native = harness(root.path(), fixture.clone());
    let registry = Arc::new(crate::HarnessRegistry::new());
    let core = crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(&root.path().join("engine")).unwrap(),
        registry.clone(),
        HarnessId::Zuse,
        None,
    )
    .unwrap();
    native.device_id = core.device_id.clone();
    registry.register(Arc::new(native));
    core.workspace
        .create_chat(
            "chat",
            None,
            Some(&core.device_id),
            None,
            Some(root.path().to_string_lossy().into_owned()),
        )
        .unwrap();
    let initial = request(root.path());
    core.sessions
        .dispatch(
            "chat",
            HarnessId::Zuse,
            initial.clone(),
            Some("first".into()),
        )
        .await
        .unwrap();
    async fn idle(core: &crate::EngineCore, fixture: &ModelFixture, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if lock(&fixture.seen).len() == count
                    && core
                        .sessions
                        .session_status("chat")
                        .is_some_and(|s| s.status == zuse_protocol::SessionStatus::Idle)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    idle(&core, &fixture, 1).await;
    let next = RunRequest {
        prompt: "durable constraint".into(),
        ..initial.clone()
    };
    core.sessions
        .dispatch(
            "chat",
            HarnessId::Zuse,
            next.clone(),
            Some("durable-delivery".into()),
        )
        .await
        .unwrap();
    idle(&core, &fixture, 2).await;
    core.sessions
        .dispatch(
            "chat",
            HarnessId::Zuse,
            RunRequest {
                model: Some("chatgpt/next".into()),
                attachments: vec![root.path().join("removed.png").to_string_lossy().into()],
                ..next
            },
            Some("durable-delivery".into()),
        )
        .await
        .unwrap();
    idle(&core, &fixture, 2).await;
    assert_eq!(
        core.sessions.last_request("chat").unwrap().model,
        initial.model
    );
    core.sessions
        .dispatch(
            "chat",
            HarnessId::Zuse,
            RunRequest {
                prompt: "A new deliberate turn".into(),
                ..initial
            },
            Some("next".into()),
        )
        .await
        .unwrap();
    idle(&core, &fixture, 3).await;
    assert_eq!(lock(&fixture.seen).last().unwrap().model, "chatgpt/fixture");
    core.shutdown().await;
}
