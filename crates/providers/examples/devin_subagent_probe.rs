//! Live probe: drive the real `devin acp` through AcpHarness and print the
//! event stream for the Fusion sidekick + mid-turn prompt paths (Devin CLI
//! 3000.14.30821). Needs a logged-in `devin` on PATH. Costs a few cents of
//! Devin credits — run deliberately, at most twice.
//!
//!     cargo run -p zuse-providers --example devin_subagent_probe -- /tmp/devin-probe/repo [1|2|all]
//!
//! Scenario 1 hands work to the Fusion sidekick (blocking); scenario 2
//! steers a running exec turn mid-flight. The optional second argument
//! selects one scenario (`1`, `2`) or both (default `all`). Each stops at
//! its first untagged Done (120s bound) and prints a summary.

use std::collections::BTreeMap;
use std::time::Duration;

use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use zuse_protocol::{AgentEvent, DoneStatus, RunRequest, SandboxLevel, ToolCall, UserInputAnswer};
use zuse_providers::{AcpHarness, CancellationToken, Harness, RunControls, SteerMessage};

const MODEL: &str = "fusion-gpt-6-2-sol-high-sidekick-swe-2-medium";
const SCENARIO_1: &str = "Protocol test, keep it minimal. Use your sidekick tool with \
     block=true: have the sidekick run `ls` in the current directory and report back. \
     Then reply with one short line. Do nothing else.";
const SCENARIO_2: &str = "Run the shell command `sleep 8; echo first-done` (one exec call), \
     then reply with exactly: DONE1";
const STEER_TEXT: &str = "Also, at the very end of your reply add the word BANANA.";

fn controls(steering: mpsc::Receiver<SteerMessage>) -> RunControls {
    RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |questions| {
            let (tx, rx) = oneshot::channel();
            let answers: Vec<UserInputAnswer> = questions
                .iter()
                .map(|q| UserInputAnswer {
                    question_id: q.id.clone(),
                    labels: q.options.first().cloned().into_iter().collect(),
                })
                .collect();
            let _ = tx.send(answers);
            rx
        }),
        steering,
        interrupt: CancellationToken::new(),
        turn: Default::default(),
    }
}

/// One scenario: run `prompt`, printing every event, steering once when the
/// first exec ToolCall arrives (scenario 2 only), and stop at the first
/// untagged Done or the 120s bound.
async fn run_scenario(
    harness: &AcpHarness,
    cwd: &str,
    prompt: &str,
    steer_on_exec: bool,
) -> Vec<AgentEvent> {
    let (steer_tx, steering) = mpsc::channel(8);
    let mut steer_tx = steer_on_exec.then_some(steer_tx);
    let request = RunRequest {
        plan: false,
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: Some(MODEL.into()),
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: cwd.into(),
        // DangerFullAccess + auto_approve selects Devin's "bypass" mode —
        // the posture the mid-turn frames were verified under. WorkspaceWrite
        // can only map to Devin's "ask" mode, which blocks exec and the
        // sidekick tool entirely.
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    };
    let mut stream = harness
        .run(request, controls(steering))
        .await
        .expect("run starts");
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let ev = match tokio::time::timeout_at(deadline, stream.next()).await {
            Ok(Some(ev)) => ev,
            Ok(None) => break,
            Err(_) => {
                eprintln!("--- timed out after 120s");
                break;
            }
        };
        match ev {
            Ok(AgentEvent::Subagent {
                parent_tool_use_id,
                event,
            }) => {
                println!("[sub {parent_tool_use_id}] {event:?}");
                events.push(AgentEvent::Subagent {
                    parent_tool_use_id,
                    event,
                });
            }
            Ok(ev) => {
                if steer_tx.is_some()
                    && matches!(
                        &ev,
                        AgentEvent::ToolCall {
                            call: ToolCall::Exec { .. },
                            ..
                        }
                    )
                {
                    let _ = steer_tx
                        .take()
                        .expect("checked")
                        .send(SteerMessage::text(STEER_TEXT))
                        .await;
                }
                let done = matches!(ev, AgentEvent::Done { .. });
                println!("{ev:?}");
                events.push(ev);
                if done {
                    break;
                }
            }
            Err(e) => println!("ERR {e}"),
        }
    }
    events
}

fn summarize(label: &str, events: &[AgentEvent], check_banana: bool) {
    let chips: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCall { id, call } if call.is_subagent_spawn() => Some(id.as_str()),
            _ => None,
        })
        .collect();
    let mut per_parent: BTreeMap<&str, usize> = BTreeMap::new();
    let mut bad_parent = false;
    for e in events {
        if let AgentEvent::Subagent {
            parent_tool_use_id, ..
        } = e
        {
            *per_parent.entry(parent_tool_use_id.as_str()).or_default() += 1;
            if matches!(parent_tool_use_id.as_str(), "root" | "sidekick") {
                bad_parent = true;
            }
        }
    }
    let dones: Vec<DoneStatus> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Done { status, .. } => Some(*status),
            _ => None,
        })
        .collect();
    println!("--- {label} summary ---");
    println!("chips opened: {chips:?}");
    println!("tagged events per parent: {per_parent:?}");
    println!("dones: {dones:?}");
    println!("any Subagent parent was root/sidekick: {bad_parent}");
    if check_banana {
        // Steered must precede the first text containing BANANA.
        let mut text = String::new();
        let mut steered_at = None;
        let mut banana_at = None;
        for (i, e) in events.iter().enumerate() {
            match e {
                AgentEvent::SteerBoundary { .. } if steered_at.is_none() => steered_at = Some(i),
                AgentEvent::TextDelta { text: delta } => {
                    text.push_str(delta);
                    if banana_at.is_none() && text.contains("BANANA") {
                        banana_at = Some(i);
                    }
                }
                _ => {}
            }
        }
        let preceded = match (steered_at, banana_at) {
            (Some(s), Some(b)) => s < b,
            _ => false,
        };
        println!(
            "BANANA seen: {}; Steered preceded BANANA: {preceded}",
            banana_at.is_some()
        );
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let cwd = args
        .next()
        .unwrap_or_else(|| "/tmp/devin-probe/repo".into());
    let scenario = args.next().unwrap_or_else(|| "all".into());
    // Validate the selector before creating the directory or starting a run.
    if !matches!(scenario.as_str(), "1" | "2" | "all") {
        eprintln!("usage: devin_subagent_probe [cwd] [1|2|all]");
        std::process::exit(2);
    }
    std::fs::create_dir_all(&cwd).unwrap();
    let harness = AcpHarness::devin();

    if scenario != "2" {
        println!("=== scenario 1: fusion sidekick (blocking) ===");
        let events = run_scenario(&harness, &cwd, SCENARIO_1, false).await;
        summarize("scenario-1", &events, false);
    }
    if scenario != "1" {
        println!("=== scenario 2: mid-turn steer ===");
        let events = run_scenario(&harness, &cwd, SCENARIO_2, true).await;
        summarize("scenario-2", &events, true);
    }
}
