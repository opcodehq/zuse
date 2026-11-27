//! A computer command behaves like any other tool call: `run_on_computer`
//! returns once the owner has decided and the command has finished (or was
//! denied, stopped, or its outcome became unknown), so the agent continues in
//! the same turn with the output.
use std::future::Future;
use std::time::Duration;

use serde_json::{Value, json};
use zuse_protocol::computer_tasks::{DEFAULT_TIMEOUT_SECS, UNKNOWN_AFTER_SECS};

/// How often a waiting call reads the request's state.
pub(crate) const POLL: Duration = Duration::from_millis(1500);
/// Failed reads in a row before the call returns the request ID to resume with.
const READ_ATTEMPTS: u32 = 3;
/// How long the owner has to decide before the call returns still waiting.
pub(crate) const APPROVAL_WAIT: Duration = Duration::from_secs(600);

/// What the agent is told when no computer can take the command.
pub(crate) const NOT_CONNECTED: &str = "The user's computer isn't connected to this chat. Ask the user to turn on \"Use this computer from cloud chats\" in Settings → Cloud computer → Your computers on the computer they want to use, then send their next message from it.";

/// The longest a call waits for `timeout_seconds` (owner decision, then run).
pub(crate) fn budget(timeout_seconds: Option<u64>) -> Duration {
    let run = timeout_seconds.unwrap_or(u64::from(DEFAULT_TIMEOUT_SECS));
    APPROVAL_WAIT + Duration::from_secs(run + UNKNOWN_AFTER_SECS)
}

/// A request that will not change any more.
fn settled(state: &str) -> bool {
    matches!(state, "finished" | "denied" | "cancelled" | "unknown")
}

/// The agent-facing outcome of one request row.
fn outcome(row: &Value, waited: bool) -> Value {
    let state = row["state"].as_str().unwrap_or("unknown");
    let note = match state {
        "finished" => None,
        "denied" => Some("The user denied this command. Don't run it again unless they ask."),
        "cancelled" => Some("The user stopped this command."),
        "unknown" => Some(
            "The computer stopped reporting; the command may or may not have run. Never resubmit it.",
        ),
        "pending" if waited => Some(
            "Still waiting for the user's approval. Call computer_task_results with this request_id to keep waiting; don't resubmit it.",
        ),
        _ if waited => Some(
            "Still running. Call computer_task_results with this request_id to keep waiting; don't resubmit it.",
        ),
        _ => None,
    };
    let mut out = json!({
        "requestId": row["id"],
        "computer": row["computer"],
        "command": row["command"],
        "state": state,
        "status": row["result"]["status"],
        "output": row["result"]["output"],
    });
    if let Some(note) = note {
        out["note"] = json!(note);
    }
    out
}

/// Wait for request `id` to settle, reading this chat's requests with
/// `results` every `poll`, for at most `budget`. Returns its outcome, or its
/// latest state (with a note to keep waiting) when the budget runs out.
pub(crate) async fn wait<F, Fut>(
    id: &str,
    mut results: F,
    poll: Duration,
    budget: Duration,
) -> anyhow::Result<Value>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<Value>>,
{
    let deadline = tokio::time::Instant::now() + budget;
    let mut last = None;
    let mut failures = 0;
    loop {
        let rows = match results().await {
            Ok(rows) => {
                failures = 0;
                rows
            }
            // The request was accepted: never lose its ID to a failed read.
            Err(error) => {
                failures += 1;
                if failures >= READ_ATTEMPTS || tokio::time::Instant::now() + poll > deadline {
                    let mut out = match &last {
                        Some(row) => outcome(row, false),
                        None => json!({"requestId":id,"state":"unknown"}),
                    };
                    out["note"] = json!(format!(
                        "Couldn't read this request's result ({error:#}). Call computer_task_results with this request_id to keep waiting; never resubmit it."
                    ));
                    return Ok(out);
                }
                tokio::time::sleep(poll).await;
                continue;
            }
        };
        let row = rows
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"].as_str() == Some(id)))
            .cloned();
        match row {
            Some(row) if settled(row["state"].as_str().unwrap_or("unknown")) => {
                return Ok(outcome(&row, false));
            }
            Some(row) => last = Some(row),
            // Not listed (yet, or no longer among the recent ones).
            None => {}
        }
        if tokio::time::Instant::now() + poll > deadline {
            return Ok(match last {
                Some(row) => outcome(&row, true),
                None => json!({"requestId":id,"state":"unknown",
                    "note":"This request is no longer listed. Never resubmit it."}),
            });
        }
        tokio::time::sleep(poll).await;
    }
}

/// Turn "nothing can take it" refusals into the hint the agent relays.
pub(crate) fn explain(error: anyhow::Error) -> anyhow::Error {
    let text = format!("{error:#}");
    if [
        "no_primary_computer",
        "computer_offline",
        "computer_not_found",
    ]
    .iter()
    .any(|code| text.contains(code))
    {
        anyhow::anyhow!("{NOT_CONNECTED} ({text})")
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn row(state: &str, status: &str, output: &str) -> Value {
        json!([{"id":"other","state":"pending"},
            {"id":"req","computer":"Mac","command":"ls","state":state,
             "result":{"status":status,"output":output}}])
    }

    /// Replays `states` (the last repeats) and counts reads.
    fn replay(
        states: Vec<Value>,
    ) -> (
        impl FnMut() -> std::future::Ready<anyhow::Result<Value>>,
        Arc<Mutex<usize>>,
    ) {
        let reads = Arc::new(Mutex::new(0));
        let counter = reads.clone();
        let next = move || {
            let mut n = counter.lock().unwrap();
            let value = states[(*n).min(states.len() - 1)].clone();
            *n += 1;
            std::future::ready(Ok(value))
        };
        (next, reads)
    }

    #[tokio::test(start_paused = true)]
    async fn approved_command_returns_its_output_in_the_same_call() {
        let (results, reads) = replay(vec![
            row("pending", "Waiting for approval", ""),
            row("approved", "Approved", ""),
            row("running", "Running", "a"),
            row("finished", "exit status: 0", "a\nb\n"),
        ]);
        let out = wait("req", results, POLL, budget(None)).await.unwrap();
        assert_eq!(out["state"], "finished");
        assert_eq!(out["output"], "a\nb\n");
        assert_eq!(out["status"], "exit status: 0");
        assert!(out.get("note").is_none());
        assert_eq!(*reads.lock().unwrap(), 4);
    }

    #[tokio::test(start_paused = true)]
    async fn denied_and_stopped_commands_say_so() {
        for (state, status, says) in [
            ("denied", "Denied", "denied"),
            ("cancelled", "Cancelled", "stopped"),
            ("unknown", "Outcome unknown", "Never resubmit"),
        ] {
            let (results, _) = replay(vec![row("pending", "", ""), row(state, status, "")]);
            let out = wait("req", results, POLL, budget(None)).await.unwrap();
            assert_eq!(out["state"], state);
            assert!(out["note"].as_str().unwrap().contains(says), "{out}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_wait_past_its_budget_returns_the_latest_state_to_keep_waiting() {
        let (results, reads) = replay(vec![row("running", "Running", "partial")]);
        let started = tokio::time::Instant::now();
        let out = wait("req", results, POLL, Duration::from_secs(10))
            .await
            .unwrap();
        assert!(started.elapsed() <= Duration::from_secs(10));
        assert_eq!(out["state"], "running");
        assert_eq!(out["output"], "partial");
        assert!(out["note"].as_str().unwrap().contains("keep waiting"));
        assert!(*reads.lock().unwrap() >= 6);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_reads_return_the_request_id_to_resume_with() {
        let reads = Arc::new(Mutex::new(0));
        let counter = reads.clone();
        let results = move || {
            let mut n = counter.lock().unwrap();
            *n += 1;
            std::future::ready(if *n == 1 {
                Ok(row("pending", "Waiting for approval", ""))
            } else {
                Err(anyhow::anyhow!("engine closed"))
            })
        };
        let out = wait("req", results, POLL, budget(None)).await.unwrap();
        assert_eq!(out["requestId"], "req");
        assert_eq!(out["state"], "pending");
        let note = out["note"].as_str().unwrap();
        assert!(
            note.contains("request_id") && note.contains("never resubmit"),
            "{note}"
        );
        assert_eq!(*reads.lock().unwrap(), 1 + READ_ATTEMPTS as usize);
    }

    #[test]
    fn the_wait_covers_the_approval_and_the_command_deadline() {
        assert_eq!(
            budget(Some(60)),
            APPROVAL_WAIT + Duration::from_secs(60 + UNKNOWN_AFTER_SECS)
        );
        assert!(budget(None) > Duration::from_secs(u64::from(DEFAULT_TIMEOUT_SECS)));
    }

    #[test]
    fn offline_refusals_tell_the_agent_how_to_connect() {
        let hint = explain(anyhow::anyhow!("ComputerTasks: 409 computer_offline")).to_string();
        assert!(hint.contains("Use this computer from cloud chats"));
        let other = explain(anyhow::anyhow!("boom")).to_string();
        assert_eq!(other, "boom");
    }
}
