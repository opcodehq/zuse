//! Live Oh My Pi contract: drives the real `omp acp` server end to end.
//!
//! OMP_EXECUTABLE=$(command -v omp) cargo test -p zuse-providers --test omp_live \
//!     -- --ignored --nocapture
//!
//! `OMP_MODEL` may pick a model the account can actually reach; omp's default
//! comes from ~/.omp and may route through a local proxy that is not running.

#![cfg(unix)]

use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};

use zuse_protocol::{
    AgentEvent, DoneStatus, RunRequest, SandboxLevel, SteeringMode, UserInputAnswer,
};
use zuse_providers::{AcpHarness, CancellationToken, Harness, RunControls};

fn controls() -> (RunControls, mpsc::Sender<zuse_providers::SteerMessage>) {
    let (steer_tx, steering) = mpsc::channel(8);
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |questions| {
            let (tx, rx) = oneshot::channel();
            let answers: Vec<UserInputAnswer> = questions
                .iter()
                .map(|q| UserInputAnswer {
                    question_id: q.id.clone(),
                    labels: vec!["Yes".into()],
                })
                .collect();
            let _ = tx.send(answers);
            rx
        }),
        steering,
        interrupt: CancellationToken::new(),
        turn: Default::default(),
    };
    (controls, steer_tx)
}

async fn run_to_end(omp: &AcpHarness, request: RunRequest) -> Vec<AgentEvent> {
    let (controls, _steer) = controls();
    let started = Instant::now();
    let stream = omp.run(request, controls).await.expect("run starts");
    let mut events = Vec::new();
    let mut stream = Box::pin(stream);
    // The ACP session stays alive for the mailbox after `Done`; the engine
    // owns teardown, so a test reads exactly one turn.
    let collect = async {
        while let Some(event) = stream.next().await {
            let event = event.expect("stream event");
            eprintln!("+{:?} {event:?}", started.elapsed());
            let done = matches!(event, AgentEvent::Done { .. });
            events.push(event);
            if done {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(180), collect)
        .await
        .expect("run finished in time");
    events
}

fn request(prompt: &str, resume: Option<String>) -> RunRequest {
    RunRequest {
        plan: false,
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: std::env::var("OMP_MODEL").ok(),
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume,
    }
}

/// Needs no reachable model: proves detection, the ACP handshake and the
/// session's model catalog through the real binary.
#[tokio::test]
#[ignore = "needs an installed Oh My Pi"]
async fn omp_acp_detects_and_lists_models() {
    let omp = AcpHarness::omp();
    assert!(
        omp.installed(),
        "omp must be installed or pinned by OMP_EXECUTABLE"
    );

    let started = Instant::now();
    let models = omp.models().await.expect("model discovery");
    eprintln!(
        "initialize+models: {:?}, {} models",
        started.elapsed(),
        models.len()
    );
    assert!(!models.is_empty(), "a session advertises its model catalog");
}

#[tokio::test]
#[ignore = "needs an installed, signed-in Oh My Pi"]
async fn omp_acp_runs_a_turn_and_resumes() {
    let omp = AcpHarness::omp();
    assert_eq!(omp.steering_mode(), SteeringMode::StepBoundary);
    assert!(
        omp.installed(),
        "omp must be installed or pinned by OMP_EXECUTABLE"
    );

    let events = run_to_end(&omp, request("Reply with exactly OMP-OK.", None)).await;
    for event in &events {
        if let AgentEvent::Error { message }
        | AgentEvent::Done {
            error: Some(message),
            ..
        } = event
        {
            eprintln!("omp error: {message}");
        }
    }
    let done = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::Done { status, .. } => Some(*status),
            _ => None,
        })
        .expect("the turn settles");
    assert_eq!(done, DoneStatus::Completed, "{events:?}");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AgentEvent::TextDelta { .. })),
        "a successful turn streams text: {events:?}"
    );

    // Reopening the chat must restore through `session/load`, not start over.
    let resumed = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::SessionStarted { session_id, .. } => Some(session_id.clone()),
            _ => None,
        })
        .expect("the turn starts a session");
    let replay = run_to_end(
        &omp,
        request("What did you just reply? One word.", Some(resumed.clone())),
    )
    .await;
    assert!(
        !replay.iter().any(|event| matches!(
            event,
            AgentEvent::Error { message } if message.contains("could not restore session")
        )),
        "omp advertises loadSession: {replay:?}"
    );
    assert!(replay.iter().any(|event| matches!(
        event,
        AgentEvent::SessionStarted { session_id, .. } if *session_id == resumed
    )));
}
