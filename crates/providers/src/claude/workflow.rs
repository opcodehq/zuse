//! Reduce the CLI's `task_progress.workflow_progress` array to the phases
//! and agents a transcript shows.

use serde_json::Value;
use zuse_protocol::{WorkflowAgentProgress, WorkflowAgentState, WorkflowPhase, WorkflowProgress};

fn text(entry: &Value, key: &str) -> Option<String> {
    entry
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}

/// Any finite JSON number (the CLI is JavaScript: integers may arrive as
/// floats).
fn number(entry: &Value, key: &str) -> Option<f64> {
    entry
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
}

fn count(entry: &Value, key: &str) -> Option<u32> {
    number(entry, key)
        .filter(|n| *n >= 0.0 && *n <= f64::from(u32::MAX))
        .map(|n| n as u32)
}

fn agent_state(entry: &Value) -> WorkflowAgentState {
    match entry.get("state").and_then(Value::as_str) {
        Some("done") => WorkflowAgentState::Done,
        Some("error") => WorkflowAgentState::Failed,
        Some("progress") => WorkflowAgentState::Running,
        // The CLI reports "start" both while an agent waits for a
        // concurrency slot and once it runs; only a running agent has a start.
        _ if number(entry, "startedAt").is_some() => WorkflowAgentState::Running,
        _ => WorkflowAgentState::Queued,
    }
}

/// The snapshot a `workflow_progress` array describes, or `None` when it
/// names no phase or agent. Volatile counters (tokens, last tool, heartbeat
/// times) and log lines are dropped so an unchanged run reduces to an
/// identical snapshot — the caller dedupes on equality.
pub(crate) fn reduce_progress(raw: &Value) -> Option<WorkflowProgress> {
    let entries = raw.as_array()?;
    let mut progress = WorkflowProgress::default();
    for entry in entries.iter().filter(|e| e.is_object()) {
        let Some(index) = count(entry, "index") else {
            continue;
        };
        match entry.get("type").and_then(Value::as_str) {
            Some("workflow_phase") => {
                if let Some(title) = text(entry, "title") {
                    progress.phases.push(WorkflowPhase { index, title });
                }
            }
            Some("workflow_agent") => {
                let Some(label) = text(entry, "label") else {
                    continue;
                };
                progress.agents.push(WorkflowAgentProgress {
                    index,
                    label,
                    state: agent_state(entry),
                    phase_index: count(entry, "phaseIndex"),
                    phase_title: text(entry, "phaseTitle"),
                    model: text(entry, "model"),
                    started_at: number(entry, "startedAt").map(|n| n as i64),
                    duration_ms: number(entry, "durationMs")
                        .filter(|n| *n >= 0.0)
                        .map(|n| n as u64),
                    prompt_preview: text(entry, "promptPreview"),
                    result_preview: text(entry, "resultPreview"),
                    error: text(entry, "error"),
                });
            }
            _ => {}
        }
    }
    if progress.phases.is_empty() && progress.agents.is_empty() {
        return None;
    }
    progress.phases.sort_by_key(|p| p.index);
    progress.agents.sort_by_key(|a| a.index);
    Some(progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn states_normalize_and_volatile_fields_drop() {
        let snapshot = |tokens: u64| {
            reduce_progress(&json!([
                {"type": "workflow_agent", "index": 3, "label": "c", "state": "start"},
                {"type": "workflow_agent", "index": 2.0, "label": "b", "state": "start",
                 "startedAt": 10, "tokens": tokens, "lastProgressAt": tokens},
                {"type": "workflow_agent", "index": 1, "label": "a", "state": "error",
                 "error": "boom", "durationMs": 4},
                {"type": "workflow_agent", "index": 4, "label": "d", "state": "done"},
                {"type": "workflow_agent", "index": 5, "label": "e", "state": "progress"},
                {"type": "workflow_phase", "index": 2, "title": "Reduce"},
                {"type": "workflow_phase", "index": 1, "title": "Map"},
                {"type": "workflow_log", "index": 9, "message": "ignored"},
                {"type": "workflow_agent", "label": "no index"},
                {"type": "workflow_agent", "index": 6, "label": " "},
                "garbage"
            ]))
            .unwrap()
        };
        let progress = snapshot(1);
        assert_eq!(progress, snapshot(99), "counters never change a snapshot");
        let states: Vec<_> = progress
            .agents
            .iter()
            .map(|a| (a.label.as_str(), a.state))
            .collect();
        assert_eq!(
            states,
            vec![
                ("a", WorkflowAgentState::Failed),
                ("b", WorkflowAgentState::Running),
                ("c", WorkflowAgentState::Queued),
                ("d", WorkflowAgentState::Done),
                ("e", WorkflowAgentState::Running),
            ]
        );
        assert_eq!(progress.agents[0].error.as_deref(), Some("boom"));
        assert_eq!(progress.agents[0].duration_ms, Some(4));
        let phases: Vec<_> = progress.phases.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(phases, ["Map", "Reduce"]);
        assert_eq!(reduce_progress(&json!([{"type": "workflow_log"}])), None);
        assert_eq!(reduce_progress(&json!({"not": "an array"})), None);
    }
}
