//! Muse Code driver against a fake `muse serve` speaking the session protocol.
#![cfg(unix)]
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::Value;
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot};
use zuse_protocol::{
    AgentEvent, DoneStatus, ReasoningLevel, RunRequest, SandboxLevel, UserInputAnswer,
    UserInputQuestion,
};
use zuse_providers::{
    CancellationToken, Harness, HarnessError, MuseHarness, RunControls, SteerMessage, TurnControl,
};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/fake-muse.py");

/// A launcher for the fixture with `env` set (never this process's env).
fn launcher(dir: &Path, env: &str) -> PathBuf {
    std::fs::set_permissions(FIXTURE, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = dir.join("muse");
    std::fs::write(
        &path,
        // The fixture logs into its working directory; keep probes (which
        // run in this process's directory) inside the test's own.
        format!(
            "#!/bin/sh\ncd '{}' || exit 1\n{env} exec python3 '{FIXTURE}' \"$@\"\n",
            dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn harness(dir: &Path, env: &str) -> MuseHarness {
    MuseHarness::new()
        .with_executable(launcher(dir, env))
        .with_graces(Duration::from_millis(500), Duration::from_millis(200))
}

fn request(cwd: &Path, prompt: &str) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
    }
}

type Asked = Arc<Mutex<Vec<UserInputQuestion>>>;

/// Controls whose input bridge answers every question with `answer`.
fn controls(
    answer: &'static str,
) -> (
    RunControls,
    mpsc::Sender<SteerMessage>,
    CancellationToken,
    TurnControl,
    Asked,
) {
    let (tx, rx) = mpsc::channel(8);
    let token = CancellationToken::new();
    let turn = TurnControl::default();
    let asked: Asked = Default::default();
    let seen = asked.clone();
    (
        RunControls {
            realtime: None,
            execution_lease: None,
            steering: rx,
            interrupt: token.clone(),
            request_input: Box::new(move |questions: Vec<UserInputQuestion>| {
                let answers = questions
                    .iter()
                    .map(|q| UserInputAnswer {
                        question_id: q.id.clone(),
                        labels: vec![answer.into()],
                    })
                    .collect();
                seen.lock().unwrap().extend(questions);
                let (tx, rx) = oneshot::channel();
                let _ = tx.send(answers);
                rx
            }),
            turn: turn.clone(),
        },
        tx,
        token,
        turn,
        asked,
    )
}

type Stream = BoxStream<'static, Result<AgentEvent, HarnessError>>;

async fn next(stream: &mut Stream) -> AgentEvent {
    tokio::time::timeout(Duration::from_secs(10), stream.next())
        .await
        .expect("event in time")
        .expect("stream open")
        .expect("event")
}

/// Events up to and including the next Done.
async fn until_done(stream: &mut Stream) -> Vec<AgentEvent> {
    let mut events = vec![];
    loop {
        let event = next(stream).await;
        let done = matches!(event, AgentEvent::Done { .. });
        events.push(event);
        if done {
            return events;
        }
    }
}

async fn collect(
    harness: &MuseHarness,
    request: RunRequest,
    answer: &'static str,
) -> Vec<AgentEvent> {
    let (controls, tx, _, _, _) = controls(answer);
    drop(tx);
    let stream = harness.run(request, controls).await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(10),
        stream.map(Result::unwrap).collect(),
    )
    .await
    .expect("stream ends")
}

fn text(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn dones(events: &[AgentEvent]) -> Vec<(DoneStatus, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done { status, error, .. } => Some((*status, error.clone())),
            _ => None,
        })
        .collect()
}

fn wire(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join("commands.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn sent<'a>(wire: &'a [Value], method: &str) -> Vec<&'a Value> {
    wire.iter().filter(|f| f["method"] == method).collect()
}

#[tokio::test]
async fn model_list_keeps_meta_rows_once_with_variant_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let harness = harness(dir.path(), "");
    let catalog = harness.model_catalog(true).await.unwrap();
    assert_eq!(catalog.source, "live");
    let ids: Vec<_> = catalog.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["muse-test", "muse-lite"]);
    assert_eq!(catalog.models[0].label, "Muse Test");
    assert_eq!(
        catalog.models[0].reasoning_levels,
        [ReasoningLevel::High, ReasoningLevel::Max]
    );
    assert_eq!(
        catalog.models[1].reasoning_levels,
        [
            ReasoningLevel::Low,
            ReasoningLevel::Medium,
            ReasoningLevel::High,
            ReasoningLevel::XHigh,
            ReasoningLevel::Max
        ]
    );
}

#[tokio::test]
async fn initialize_timeout_and_wrong_schema_fail_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let hang =
        harness(dir.path(), "FAKE_MUSE_HANG=1").with_init_timeout(Duration::from_millis(400));
    let error = hang.model_catalog(true).await.unwrap_err().to_string();
    assert!(error.contains("did not finish initializing"), "{error}");
    let events = collect(&hang, request(dir.path(), "hello"), "Yes").await;
    let done = dones(&events);
    assert_eq!(done.len(), 1, "{events:?}");
    assert_eq!(done[0].0, DoneStatus::Errored);
    assert!(
        done[0]
            .1
            .as_ref()
            .unwrap()
            .contains("did not finish initializing")
    );

    let other = tempfile::tempdir().unwrap();
    let schema = harness(other.path(), "FAKE_MUSE_SCHEMA=2");
    let events = collect(&schema, request(other.path(), "hello"), "Yes").await;
    let done = dones(&events);
    assert_eq!(done[0].0, DoneStatus::Errored);
    assert!(
        done[0].1.as_ref().unwrap().contains("protocol version 2"),
        "{done:?}"
    );
    // No session is opened on a host that speaks another protocol.
    assert!(sent(&wire(other.path()), "session/start").is_empty());
}

#[tokio::test]
async fn a_streamed_turn_carries_text_usage_context_and_the_session_contract() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), "hello");
    req.reasoning = Some(ReasoningLevel::High);
    req.model = Some("muse-test".into());
    let events = collect(&harness(dir.path(), ""), req, "Yes").await;
    assert!(matches!(
        &events[0],
        AgentEvent::SessionStarted { session_id, model, .. }
            if session_id == "native-muse-session" && model == "muse-test"
    ));
    assert_eq!(text(&events), "Hello 🌍");
    assert!(events.contains(&AgentEvent::Usage {
        input_tokens: 12,
        output_tokens: 3
    }));
    assert!(events.contains(&AgentEvent::ContextUsage {
        tokens: Some(15),
        window: Some(1000)
    }));
    assert_eq!(dones(&events), [(DoneStatus::Completed, None)]);

    let wire = wire(dir.path());
    let init = sent(&wire, "initialize")[0];
    assert_eq!(
        init["params"]["capabilities"]["requestedCapabilities"],
        serde_json::json!(["sessionMcp"])
    );
    assert!(sent(&wire, "initialized").len() == 1);
    let start = &sent(&wire, "session/start")[0]["params"];
    assert_eq!(start["permissionProfile"], ":ask-me");
    assert_eq!(start["approvalMode"], "promptUnmatched");
    assert_eq!(start["providerId"], "meta");
    assert_eq!(start["modelId"], "muse-test");
    assert!(start["commandId"].as_str().is_some_and(|id| id.len() == 36));
    let turn = &sent(&wire, "turn/start")[0]["params"];
    assert_eq!(turn["reasoningEffort"], "high");
    assert_eq!(turn["input"][0]["text"], "hello");
    assert_eq!(turn["sessionId"], "native-muse-session");
    let root = dir.path().canonicalize().unwrap().display().to_string();
    assert_eq!(turn["workspaceRoots"], serde_json::json!([root]));
    assert_ne!(turn["commandId"], start["commandId"]);
}

#[tokio::test]
async fn approvals_round_trip_through_the_user_and_only_offered_choices() {
    for (answer, choice, output) in [("Yes", "yes", "file.txt"), ("No", "no", "denied by user")] {
        let dir = tempfile::tempdir().unwrap();
        let (controls, tx, _, _, asked) = controls(answer);
        drop(tx);
        let stream = harness(dir.path(), "")
            .run(request(dir.path(), "approve"), controls)
            .await
            .unwrap();
        let events: Vec<_> = tokio::time::timeout(
            Duration::from_secs(10),
            stream.map(Result::unwrap).collect::<Vec<_>>(),
        )
        .await
        .unwrap();
        let asked = asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].question.contains("`ls`"), "{asked:?}");
        let decide = wire(dir.path());
        let decide = sent(&decide, "approval/decide");
        assert_eq!(decide.len(), 1);
        assert_eq!(decide[0]["params"]["choiceId"], choice);
        assert_eq!(
            decide[0]["params"]["requirementId"],
            serde_json::json!({"approvalId": "approval", "sourceIndex": 0})
        );
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolResult { id, output: Some(o), .. } if id == "tool" && o == output
        )));
        assert_eq!(dones(&events), [(DoneStatus::Completed, None)]);
    }

    // Full access with auto-approve settles without asking.
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), "approve");
    req.sandbox = SandboxLevel::DangerFullAccess;
    req.auto_approve = true;
    let (controls, tx, _, _, asked) = controls("No");
    drop(tx);
    let stream = harness(dir.path(), "").run(req, controls).await.unwrap();
    let _: Vec<_> = tokio::time::timeout(Duration::from_secs(10), stream.collect::<Vec<_>>())
        .await
        .unwrap();
    assert!(asked.lock().unwrap().is_empty());
    assert_eq!(
        sent(&wire(dir.path()), "approval/decide")[0]["params"]["choiceId"],
        "yes"
    );
}

#[tokio::test]
async fn questions_are_answered_by_label() {
    let dir = tempfile::tempdir().unwrap();
    let events = collect(
        &harness(dir.path(), ""),
        request(dir.path(), "question"),
        "Two",
    )
    .await;
    assert_eq!(dones(&events), [(DoneStatus::Completed, None)]);
    let wire = wire(dir.path());
    let answer = &sent(&wire, "userInput/answer")[0]["params"];
    assert_eq!(answer["userInputId"], "question");
    assert_eq!(
        answer["answers"],
        serde_json::json!([{"questionId": "q", "selectedLabel": "Two"}])
    );
}

#[tokio::test]
async fn stopping_a_turn_keeps_the_runtime_for_steers_and_interrupt_ends_it() {
    let dir = tempfile::tempdir().unwrap();
    let (controls, tx, token, turn, _) = controls("Yes");
    let mut stream: Stream = harness(dir.path(), "")
        .run(request(dir.path(), "wait"), controls)
        .await
        .unwrap();
    assert!(matches!(
        next(&mut stream).await,
        AgentEvent::SessionStarted { .. }
    ));
    turn.stop_turn();
    let events = until_done(&mut stream).await;
    assert_eq!(dones(&events), [(DoneStatus::Interrupted, None)]);
    let interrupts = sent(&wire(dir.path()), "turn/interrupt")
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(interrupts.len(), 1);
    let started = wire(dir.path());
    assert_eq!(
        interrupts[0]["params"]["turnId"],
        sent(&started, "turn/start")[0]["params"]["commandId"]
    );

    // The same runtime takes the next prompt as a new turn.
    tx.send(SteerMessage::text("hello")).await.unwrap();
    let events = until_done(&mut stream).await;
    assert!(
        matches!(events[0], AgentEvent::Steered { .. }),
        "{events:?}"
    );
    assert_eq!(text(&events), "Hello 🌍");
    assert_eq!(dones(&events), [(DoneStatus::Completed, None)]);

    // A steer into a running turn is confirmed when its message joins.
    tx.send(SteerMessage::text("wait")).await.unwrap();
    assert!(matches!(
        next(&mut stream).await,
        AgentEvent::Steered { .. }
    ));
    tx.send(SteerMessage::text("more please")).await.unwrap();
    let events = until_done(&mut stream).await;
    assert!(
        matches!(events[0], AgentEvent::Steered { .. }),
        "{events:?}"
    );
    assert_eq!(text(&events), "Steered reply");
    let wire_now = wire(dir.path());
    assert_eq!(
        sent(&wire_now, "turn/steer")[0]["params"]["input"][0]["text"],
        "more please"
    );

    // Interrupting the runtime mid-turn ends the stream with Interrupted.
    tx.send(SteerMessage::text("wait")).await.unwrap();
    assert!(matches!(
        next(&mut stream).await,
        AgentEvent::Steered { .. }
    ));
    token.cancel();
    let rest: Vec<_> = tokio::time::timeout(
        Duration::from_secs(10),
        stream.map(Result::unwrap).collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    assert_eq!(dones(&rest), [(DoneStatus::Interrupted, None)]);
}

#[tokio::test]
async fn resume_reopens_the_session_and_a_lost_one_starts_fresh_with_a_notice() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), "hello");
    req.resume = Some("saved-session".into());
    req.model = Some("muse-test".into());
    let events = collect(&harness(dir.path(), ""), req, "Yes").await;
    assert!(matches!(
        &events[0],
        AgentEvent::SessionStarted { session_id, .. } if session_id == "saved-session"
    ));
    assert_eq!(dones(&events), [(DoneStatus::Completed, None)]);
    let wire_log = wire(dir.path());
    let resume = &sent(&wire_log, "session/resume")[0]["params"];
    assert_eq!(resume["sessionId"], "saved-session");
    assert_eq!(resume["excludeItems"], true);
    assert!(sent(&wire_log, "session/start").is_empty());
    assert_eq!(
        sent(&wire_log, "session/setApprovalMode")[0]["params"]["mode"],
        "promptUnmatched"
    );
    assert_eq!(
        sent(&wire_log, "session/setModel")[0]["params"]["model"],
        serde_json::json!({"modelId": "muse-test", "providerId": "meta"})
    );

    let lost = tempfile::tempdir().unwrap();
    let mut req = request(lost.path(), "hello");
    req.resume = Some("missing".into());
    let events = collect(&harness(lost.path(), ""), req, "Yes").await;
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::Error { message } if message.contains("could not restore session missing")
    )));
    assert_eq!(dones(&events), [(DoneStatus::Completed, None)]);
    assert_eq!(sent(&wire(lost.path()), "session/start").len(), 1);
}

#[tokio::test]
async fn compaction_gap_recovery_crashes_and_profile_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), "/compact");
    req.resume = Some("saved-session".into());
    let events = collect(&harness(dir.path(), ""), req, "Yes").await;
    assert_eq!(text(&events), "Context compacted.");
    assert_eq!(dones(&events), [(DoneStatus::Completed, None)]);
    assert_eq!(sent(&wire(dir.path()), "session/compact").len(), 1);
    assert!(
        harness(dir.path(), "")
            .run(request(dir.path(), "/compact"), controls("Yes").0)
            .await
            .is_err()
    );

    let gap = tempfile::tempdir().unwrap();
    let events = collect(&harness(gap.path(), ""), request(gap.path(), "gap"), "Yes").await;
    assert_eq!(text(&events), "Recovered reply");
    assert_eq!(dones(&events), [(DoneStatus::Completed, None)]);
    assert_eq!(sent(&wire(gap.path()), "view/page").len(), 1);

    let crash = tempfile::tempdir().unwrap();
    let events = collect(
        &harness(crash.path(), ""),
        request(crash.path(), "crash"),
        "Yes",
    )
    .await;
    let done = dones(&events);
    assert_eq!(done[0].0, DoneStatus::Errored);
    assert!(
        done[0].1.as_ref().unwrap().contains("exit code 7"),
        "{done:?}"
    );

    let profile = tempfile::tempdir().unwrap();
    let events = collect(
        &harness(profile.path(), "FAKE_MUSE_PROFILE=:unrestricted"),
        request(profile.path(), "hello"),
        "Yes",
    )
    .await;
    let done = dones(&events);
    assert_eq!(done[0].0, DoneStatus::Errored);
    assert!(done[0].1.as_ref().unwrap().contains("ask-me"));
    assert!(sent(&wire(profile.path()), "turn/start").is_empty());
}

#[tokio::test]
async fn zuse_mcp_needs_the_session_mcp_capability() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), "hello");
    req.mcp = Some(zuse_protocol::McpServer {
        name: "zuse".into(),
        command: "/bin/true".into(),
        args: vec![],
        env: Default::default(),
    });
    let events = collect(&harness(dir.path(), ""), req.clone(), "Yes").await;
    assert_eq!(dones(&events), [(DoneStatus::Completed, None)]);
    assert_eq!(
        sent(&wire(dir.path()), "session/start")[0]["params"]["config"]["mcpServers"]["zuse"]["transport"],
        "stdio"
    );
    let old = tempfile::tempdir().unwrap();
    req.cwd = old.path().display().to_string();
    let events = collect(&harness(old.path(), "FAKE_MUSE_NO_MCP=1"), req, "Yes").await;
    let done = dones(&events);
    assert_eq!(done[0].0, DoneStatus::Errored);
    assert!(done[0].1.as_ref().unwrap().contains("session MCP"));
}

/// Real Muse Code, when installed and signed in:
/// `cargo test -p zuse-providers --test muse_rpc -- --ignored`.
#[tokio::test]
#[ignore = "needs an installed, signed-in Muse Code CLI"]
async fn live_muse_lists_models_and_answers() {
    let harness = MuseHarness::new();
    if !harness.installed() {
        eprintln!("Muse Code is not installed; skipping");
        return;
    }
    let models = harness.models().await.expect("Muse Code model list");
    assert!(!models.is_empty());
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), "Reply with the single word: pong");
    req.sandbox = SandboxLevel::ReadOnly;
    req.model = Some(models[0].id.clone());
    let (controls, tx, _, _, _) = controls("No");
    drop(tx);
    let stream = harness.run(req, controls).await.unwrap();
    let events: Vec<_> = tokio::time::timeout(
        Duration::from_secs(180),
        stream.map(Result::unwrap).collect::<Vec<_>>(),
    )
    .await
    .unwrap();
    assert_eq!(dones(&events)[0].0, DoneStatus::Completed, "{events:?}");
    assert!(text(&events).to_lowercase().contains("pong"), "{events:?}");
}
