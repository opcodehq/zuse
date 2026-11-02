//! Native workflow runs: a script the agent launches in the background that
//! fans work out to many agents, grouped into phases.
//!
//! A run stays an ordinary [`ToolCall::Unknown`] named [`WORKFLOW_TOOL`] — a
//! new `ToolCall` variant would break every older reader, since the enum has
//! no serde fallback. Its latest phase/agent snapshot rides the call's input
//! under [`WORKFLOW_PROGRESS_KEY`], so each update replaces the previous one
//! in place instead of appending transcript rows.

use serde::{Deserialize, Serialize};

use crate::ToolCall;

/// The tool name a workflow run decodes to.
pub const WORKFLOW_TOOL: &str = "Workflow";
/// Input key carrying the run's latest [`WorkflowProgress`].
pub const WORKFLOW_PROGRESS_KEY: &str = "workflow_progress";
/// Input key carrying the run's name as the harness reported it.
pub const WORKFLOW_NAME_KEY: &str = "workflow_name";

/// Where one workflow agent stands, normalized from the harness's states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkflowAgentState {
    /// Waiting for a concurrency slot.
    Queued,
    Running,
    Done,
    Failed,
}

/// One phase a run declared or entered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPhase {
    pub index: u32,
    pub title: String,
}

/// One agent a run started, as last reported. Volatile counters (tokens,
/// heartbeats) are deliberately absent so unchanged runs compare equal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAgentProgress {
    pub index: u32,
    pub label: String,
    pub state: WorkflowAgentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Epoch milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Latest phase and agent snapshot of a workflow run, both sorted by index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowProgress {
    #[serde(default)]
    pub phases: Vec<WorkflowPhase>,
    #[serde(default)]
    pub agents: Vec<WorkflowAgentProgress>,
}

impl ToolCall {
    /// A native workflow run (see the module docs).
    pub fn is_workflow_run(&self) -> bool {
        matches!(self, ToolCall::Unknown { name, .. } if name == WORKFLOW_TOOL)
    }

    fn workflow_input(&self) -> Option<&serde_json::Value> {
        match self {
            ToolCall::Unknown { name, input } if name == WORKFLOW_TOOL => input.as_ref(),
            _ => None,
        }
    }

    /// The latest snapshot attached to a workflow run, if any decodes.
    pub fn workflow_progress(&self) -> Option<WorkflowProgress> {
        let value = self.workflow_input()?.get(WORKFLOW_PROGRESS_KEY)?;
        serde_json::from_value(value.clone()).ok()
    }

    /// The run's display name: the harness-reported name, else the name the
    /// launch input gave it.
    pub fn workflow_name(&self) -> Option<&str> {
        let input = self.workflow_input()?;
        [WORKFLOW_NAME_KEY, "name"]
            .iter()
            .find_map(|key| input.get(key).and_then(serde_json::Value::as_str))
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }

    /// A workflow run call: `input` (the launch input, if any) with the
    /// harness-reported name and snapshot layered on top.
    pub fn workflow_run(
        input: Option<&serde_json::Value>,
        name: Option<&str>,
        progress: Option<&WorkflowProgress>,
    ) -> ToolCall {
        let mut map = match input {
            Some(serde_json::Value::Object(map)) => map.clone(),
            _ => serde_json::Map::new(),
        };
        if let Some(name) = name.map(str::trim).filter(|n| !n.is_empty()) {
            map.insert(WORKFLOW_NAME_KEY.into(), name.into());
        }
        if let Some(progress) = progress
            && let Ok(value) = serde_json::to_value(progress)
        {
            map.insert(WORKFLOW_PROGRESS_KEY.into(), value);
        }
        ToolCall::Unknown {
            name: WORKFLOW_TOOL.into(),
            input: (!map.is_empty()).then_some(serde_json::Value::Object(map)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentEvent;

    fn progress() -> WorkflowProgress {
        WorkflowProgress {
            phases: vec![WorkflowPhase {
                index: 1,
                title: "Map".into(),
            }],
            agents: vec![WorkflowAgentProgress {
                index: 1,
                label: "map:files".into(),
                state: WorkflowAgentState::Running,
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

    #[test]
    fn workflow_progress_round_trips_through_the_call_and_the_event() {
        let input = serde_json::json!({ "script": "export const meta = {}", "name": "draft" });
        let call = ToolCall::workflow_run(Some(&input), Some("audit"), Some(&progress()));
        assert!(call.is_workflow_run());
        assert_eq!(call.workflow_name(), Some("audit"));
        assert_eq!(call.workflow_progress(), Some(progress()));

        // Wire shape: camelCase fields, states as lowercase words.
        let json = serde_json::to_value(progress()).unwrap();
        assert_eq!(json["agents"][0]["state"], "running");
        assert_eq!(json["agents"][0]["phaseIndex"], 1);
        assert!(json["agents"][0].get("durationMs").is_none());

        let event = AgentEvent::WorkflowProgress {
            tool_use_id: "workflow-1".into(),
            call: call.clone(),
        };
        let text = serde_json::to_string(&event).unwrap();
        assert!(text.contains("\"type\":\"workflowProgress\""));
        assert!(text.contains("\"toolUseId\":\"workflow-1\""));
        assert_eq!(serde_json::from_str::<AgentEvent>(&text).unwrap(), event);

        // The call itself still reads as an ordinary Unknown tool call.
        let call_json = serde_json::to_value(&call).unwrap();
        assert_eq!(call_json["kind"], "unknown");
        assert_eq!(call_json["name"], WORKFLOW_TOOL);
    }

    #[test]
    fn only_workflow_calls_answer() {
        let launch =
            ToolCall::workflow_run(Some(&serde_json::json!({ "name": " audit " })), None, None);
        assert_eq!(launch.workflow_name(), Some("audit"));
        assert_eq!(launch.workflow_progress(), None);
        let other = ToolCall::Unknown {
            name: "Agent: scan".into(),
            input: Some(serde_json::json!({ WORKFLOW_PROGRESS_KEY: {"phases": [], "agents": []} })),
        };
        assert!(!other.is_workflow_run());
        assert_eq!(other.workflow_progress(), None);
        assert_eq!(other.workflow_name(), None);
        // A malformed snapshot decodes to nothing rather than a partial board.
        let malformed = ToolCall::workflow_run(
            Some(&serde_json::json!({ WORKFLOW_PROGRESS_KEY: {"agents": [{"index": "x"}]} })),
            None,
            None,
        );
        assert_eq!(malformed.workflow_progress(), None);
    }
}
