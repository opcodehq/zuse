//! Frame → [`AgentEvent`] normalization (init dedupe, subagent tagging, tool
//! decoding, error-code mapping).

use serde_json::Value;
use zuse_protocol::{AgentEvent, DoneStatus, HarnessId, TodoItem, TodoStatus, ToolCall, ToolDiff};

use super::wire::{ContentBlock, Frame, SystemFrame};

/// Human-readable text for the CLI's assistant-level error codes. These arrive
/// as a terse `error` field on an `assistant` frame — usually with NO text
/// content and NOT as a `result` error — so a usage-limited or otherwise failed
/// turn looks like the agent simply never replied unless we surface it.
fn assistant_error_text(code: &str) -> String {
    match code {
        "authentication_failed" => "Authentication failed — sign in to Claude again.".into(),
        "oauth_org_not_allowed" => "This organization isn't allowed to use Claude here.".into(),
        "billing_error" => "Billing error — check your Claude plan or payment method.".into(),
        "rate_limit" => "Claude usage limit reached — try again after the limit resets.".into(),
        "overloaded" => "Claude is overloaded right now — try again shortly.".into(),
        "invalid_request" => "The request was rejected as invalid.".into(),
        "model_not_found" => "The selected model isn't available.".into(),
        "server_error" => "Claude had a server error — try again.".into(),
        "max_output_tokens" => "The reply hit the maximum output length.".into(),
        "unknown" => "Claude returned an unspecified error.".into(),
        other => format!("Claude error: {other}"),
    }
}

/// Which claude.ai usage window a `rate_limit_event` refers to.
fn rate_window_label(kind: &str) -> &'static str {
    match kind {
        "five_hour" => "5-hour",
        "seven_day" | "seven_day_overage_included" => "weekly",
        "seven_day_opus" => "weekly (Opus)",
        "seven_day_sonnet" => "weekly (Sonnet)",
        "overage" => "overage",
        _ => "usage",
    }
}

/// Fallback wording for a `result` error whose `errors` array is empty, so the
/// turn never ends with a blank (and therefore invisible) error.
fn result_error_text(subtype: &str) -> &'static str {
    match subtype {
        "error_max_turns" => "The run hit the maximum number of turns.",
        "error_max_budget_usd" => "The run hit its cost budget.",
        "error_max_structured_output_retries" => "The run exhausted its structured-output retries.",
        _ => "The run ended with an error.",
    }
}

/// The CLI seeds `result.errors` with internal `[ede_diagnostic]` breadcrumbs
/// for its error_during_execution telemetry ("turn aborted (…) stop_reason=…",
/// "result_type=… last_content_type=… stop_reason=…"). They're diagnostics
/// about the CLI's own turn accounting, not user-relevant errors — surfacing
/// them verbatim put raw `[ede_diagnostic] result_type=user …` boxes in the
/// transcript. They're debug-logged and dropped instead.
fn is_internal_diagnostic(message: &str) -> bool {
    message.contains("[ede_diagnostic]")
}

fn str_field(input: &Value, key: &str) -> String {
    input.get(key).and_then(Value::as_str).unwrap_or("").into()
}

fn opt_str_field(input: &Value, key: &str) -> Option<String> {
    input.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// The diff a Write/Edit call's input describes: a Write is the whole new
/// file, an Edit its replaced span. Capped like the other harnesses' diffs.
fn file_diff(name: &str, input: &Value) -> Option<ToolDiff> {
    const LIMIT: usize = 64 * 1024;
    let cap = |text: &str| match text.char_indices().nth(LIMIT) {
        Some((at, _)) => text[..at].to_owned(),
        None => text.to_owned(),
    };
    let path = str_field(input, "file_path");
    if path.is_empty() {
        return None;
    }
    match name {
        "Write" => Some(ToolDiff {
            path,
            old_text: None,
            new_text: cap(input.get("content")?.as_str()?),
        }),
        "Edit" => Some(ToolDiff {
            path,
            old_text: Some(cap(input.get("old_string")?.as_str()?)),
            new_text: cap(input.get("new_string")?.as_str()?),
        }),
        _ => None,
    }
}

/// Decode a Claude `tool_use` block (name + input) into a typed [`ToolCall`].
pub(crate) fn decode_tool_use(name: &str, input: &Value) -> ToolCall {
    match name {
        "Bash" => ToolCall::Exec {
            command: str_field(input, "command"),
        },
        "Read" => ToolCall::ReadFile {
            path: str_field(input, "file_path"),
        },
        "Write" => ToolCall::WriteFile {
            path: str_field(input, "file_path"),
            content: opt_str_field(input, "content"),
        },
        "Edit" => ToolCall::EditFile {
            path: str_field(input, "file_path"),
            old_string: opt_str_field(input, "old_string"),
            new_string: opt_str_field(input, "new_string"),
        },
        "Grep" => ToolCall::Search {
            pattern: str_field(input, "pattern"),
            path: opt_str_field(input, "path"),
        },
        "Glob" => ToolCall::Glob {
            pattern: str_field(input, "pattern"),
        },
        "WebFetch" => ToolCall::WebFetch {
            url: str_field(input, "url"),
            prompt: opt_str_field(input, "prompt"),
        },
        "WebSearch" => ToolCall::WebSearch {
            query: str_field(input, "query"),
        },
        "TodoWrite" => ToolCall::Todo {
            items: input
                .get("todos")
                .and_then(Value::as_array)
                .map(|a| a.as_slice())
                .unwrap_or_default()
                .iter()
                .map(|t| {
                    TodoItem::new(
                        str_field(t, "content"),
                        TodoStatus::parse(t.get("status").and_then(Value::as_str).unwrap_or("")),
                    )
                })
                .collect(),
        },
        // The subagent spawn: name the chip — and the tab it opens — after
        // the TASK, not the bare tool ("Agent" alone says nothing in a tab
        // strip; the tool's `description` is where the work is named).
        "Agent" | "Task" => {
            let description = str_field(input, "description");
            ToolCall::Unknown {
                name: if description.is_empty() {
                    "Agent".into()
                } else {
                    format!("Agent: {description}")
                },
                input: (!input.is_null()).then(|| input.clone()),
            }
        }
        // MCP tools arrive as `mcp__<server>__<tool>`.
        _ => match name.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
            Some((server, tool)) => ToolCall::Mcp {
                server: server.into(),
                tool: tool.into(),
                input: (!input.is_null()).then(|| input.clone()),
            },
            None => ToolCall::Unknown {
                name: name.into(),
                input: (!input.is_null()).then(|| input.clone()),
            },
        },
    }
}

/// The CLI tool that launches a native workflow run (always in the background).
const WORKFLOW_TOOL_USE: &str = "Workflow";

/// A launched workflow run: its reported name and the last snapshot sent,
/// so an unchanged `task_progress` emits nothing.
#[derive(Default)]
struct WorkflowRun {
    name: Option<String>,
    last: Option<zuse_protocol::WorkflowProgress>,
}

/// A background task's terminal `task_notification` status, or `None` for a
/// non-terminal notification shape.
fn task_done_status(status: Option<&str>) -> Option<DoneStatus> {
    match status.unwrap_or("") {
        "completed" | "complete" | "succeeded" | "success" => Some(DoneStatus::Completed),
        "failed" | "errored" | "error" => Some(DoneStatus::Errored),
        "killed" | "cancelled" | "canceled" | "stopped" | "interrupted" => {
            Some(DoneStatus::Interrupted)
        }
        _ => None,
    }
}

fn settled(status: DoneStatus) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: None,
        session_id: None,
    }
}

fn new_message_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Wrap an event as subagent-attributed traffic.
fn tag(parent: &str, event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: parent.to_owned(),
        event: Box::new(event),
    }
}

/// CLI-synthesized text that rides a user frame but is NOT conversation:
/// `<system-reminder>` context injections and the interruption marker the CLI
/// stamps into the transcript when a turn (or a subagent) is stopped.
///
/// On a TAGGED frame the distinction is load-bearing, not cosmetic. A tagged
/// user message means "the parent steered its subagent", which announces more
/// work and is therefore the one event allowed to resurrect a settled spawn
/// chip. The CLI emits `[Request interrupted by user]` on the child feed
/// immediately AFTER the subagent's `done{interrupted}` — read as a steer it
/// un-settled a chip that nothing would ever settle again, and the spinner ran
/// forever (2026-08-21: "orchestrator killed them but spinner doesn't stop").
/// It announces the opposite of more work.
///
/// Prefix-matched: the CLI ships at least two spellings of the marker
/// (`…by user]` and `…by user for tool use]`).
fn is_synthetic_user_text(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("<system-reminder>") || text.starts_with("[Request interrupted")
}

/// Per-run normalization state.
///
/// `saw_init` dedupes `system:init` — the CLI re-emits it every time the model
/// is re-invoked WITHIN one session (a background-subagent wake turn, a
/// scheduled wakeup), not just at start (live-verified against 2.1.228: a
/// background subagent finishing produces a second init with the SAME session
/// id, then a second `result`). Downstream, `SessionStarted` is the fold's run
/// boundary (it resets accumulated parts), so one run ⇒ one `SessionStarted`;
/// the wake turn's own frames flow through and the engine's parked-session
/// resume turns them into the done→Working→done wake.
pub(crate) struct Normalizer {
    saw_init: bool,
    last_model: Option<String>,
    /// Background-agent ids (`task_started.task_id`) → the spawning Agent
    /// tool_use id. `SendMessage` steers address the AGENT id; this map
    /// re-keys them onto the spawn chip's feed (the wire never echoes the
    /// steer on the child feed — live-verified 2.1.228).
    agent_tasks: std::collections::HashMap<String, String>,
    /// Later tool calls that wake the same task → its original spawn.
    agent_tool_spawns: std::collections::HashMap<String, String>,
    /// tool_use ids of Agent/Task spawn calls, recorded from their own
    /// assistant frames (plus `task_started`'s agent-task pairing). Gates
    /// `task_notification`: background SHELL tasks settle through the same
    /// subtype carrying their Bash call's id, and tagging that Done as
    /// subagent traffic stamped a spawn ref onto an ordinary Run chip —
    /// which then opened as an empty, never-created subagent doc (user
    /// report 2026-08-20).
    agent_spawn_tools: std::collections::HashSet<String>,
    /// Native workflow launches (`Workflow` tool_use id → launch input),
    /// recorded from their own assistant frames. Each progress refresh
    /// re-emits the input so the chip keeps its source.
    workflow_inputs: std::collections::HashMap<String, Value>,
    /// Workflow task ids (`task_started.task_id`) → launching tool_use id.
    workflow_tasks: std::collections::HashMap<String, String>,
    /// Started workflow runs by launching tool_use id.
    workflow_runs: std::collections::HashMap<String, WorkflowRun>,
    /// Write/Edit tool_use id → the diff its input describes. The wire's
    /// tool_result carries only a one-line confirmation, so the chip's diff
    /// is rebuilt from the call and attached when the result lands.
    file_diffs: std::collections::HashMap<String, ToolDiff>,
    /// Rotates at each assistant-frame close and at each steer; SessionStarted
    /// carries the first value so folds can attribute deltas from the start.
    assistant_message_id: String,
    /// Last session id seen (init or result) — used for synthetic Dones.
    pub session_id: Option<String>,
}

impl Normalizer {
    pub fn new() -> Self {
        Self {
            saw_init: false,
            last_model: None,
            agent_tasks: std::collections::HashMap::new(),
            agent_tool_spawns: std::collections::HashMap::new(),
            agent_spawn_tools: std::collections::HashSet::new(),
            workflow_inputs: std::collections::HashMap::new(),
            workflow_tasks: std::collections::HashMap::new(),
            workflow_runs: std::collections::HashMap::new(),
            file_diffs: std::collections::HashMap::new(),
            assistant_message_id: new_message_id(),
            session_id: None,
        }
    }

    /// Restore only spawn identity from Claude's native history. The CLI
    /// does not replay task_started when --resume starts a new process.
    /// Session ids are global; search project directories rather than
    /// duplicating the CLI's cwd encoding (including long-path hashing).
    /// Long sessions' histories run to hundreds of MB: the scan runs off the
    /// runtime and only parses lines that can name a spawn or its agent.
    pub async fn for_resume(config_root: &std::path::Path, session_id: &str) -> Self {
        let config_root = config_root.to_owned();
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || Self::restore_spawns(&config_root, &session_id))
            .await
            .unwrap_or_else(|_| Self::new())
    }

    fn restore_spawns(config_root: &std::path::Path, session_id: &str) -> Self {
        use std::io::BufRead as _;
        let mut norm = Self::new();
        if session_id.is_empty()
            || !session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return norm;
        }
        let Ok(projects) = std::fs::read_dir(config_root.join("projects")) else {
            return norm;
        };
        let Some(file) = projects.flatten().find_map(|project| {
            std::fs::File::open(project.path().join(format!("{session_id}.jsonl"))).ok()
        }) else {
            return norm;
        };
        let mut reader = std::io::BufReader::with_capacity(1 << 16, file);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            // Torn lines and older/newer unknown shapes are harmless.
            let Ok(text) = std::str::from_utf8(&line) else {
                continue;
            };
            if !(text.contains("\"Agent\"")
                || text.contains("\"Task\"")
                || text.contains("\"Workflow\"")
                || text.contains("agentId"))
            {
                continue;
            }
            let Ok(record) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            // A subagent's own nested spawns are not chips on this chat.
            if record.get("isSidechain").and_then(Value::as_bool) == Some(true)
                || !record.get("parent_tool_use_id").is_none_or(Value::is_null)
            {
                continue;
            }
            let Some(blocks) = record.pointer("/message/content").and_then(Value::as_array) else {
                continue;
            };
            match record.get("type").and_then(Value::as_str) {
                Some("assistant") => {
                    for block in blocks {
                        if block.get("type").and_then(Value::as_str) == Some("tool_use")
                            && matches!(
                                block.get("name").and_then(Value::as_str),
                                Some("Agent" | "Task")
                            )
                            && let Some(id) = block
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|id| !id.is_empty())
                        {
                            norm.agent_spawn_tools.insert(id.to_owned());
                        }
                        if block.get("type").and_then(Value::as_str) == Some("tool_use")
                            && block.get("name").and_then(Value::as_str) == Some(WORKFLOW_TOOL_USE)
                            && let Some(id) = block
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|id| !id.is_empty())
                        {
                            norm.workflow_inputs.insert(
                                id.to_owned(),
                                block.get("input").cloned().unwrap_or(Value::Null),
                            );
                        }
                    }
                }
                Some("user") => {
                    if let Some(agent) = record
                        .pointer("/toolUseResult/agentId")
                        .and_then(Value::as_str)
                        .filter(|id| !id.is_empty())
                        && let Some(spawn) = blocks
                            .iter()
                            .filter(|b| {
                                b.get("type").and_then(Value::as_str) == Some("tool_result")
                            })
                            .filter_map(|b| b.get("tool_use_id").and_then(Value::as_str))
                            .find(|id| norm.agent_spawn_tools.contains(*id))
                    {
                        norm.agent_tasks
                            .entry(agent.to_owned())
                            .or_insert_with(|| spawn.to_owned());
                    }
                }
                _ => {}
            }
        }
        norm
    }

    /// The launching tool_use id a workflow task frame refers to, if it is
    /// one: by task id once `task_started` paired them, else by the frame's
    /// own tool id when that names a known launch.
    fn workflow_tool(&self, f: &SystemFrame) -> Option<String> {
        f.task_id
            .as_deref()
            .and_then(|task| self.workflow_tasks.get(task))
            .cloned()
            .or_else(|| {
                f.tool_use_id
                    .as_deref()
                    .filter(|tool| {
                        self.workflow_runs.contains_key(*tool)
                            || self.workflow_inputs.contains_key(*tool)
                    })
                    .map(str::to_owned)
            })
    }

    /// The refreshed chip call for a workflow run.
    fn workflow_refresh(&self, tool: &str) -> AgentEvent {
        let run = self.workflow_runs.get(tool);
        AgentEvent::WorkflowProgress {
            tool_use_id: tool.to_owned(),
            call: ToolCall::workflow_run(
                self.workflow_inputs.get(tool),
                run.and_then(|r| r.name.as_deref()),
                run.and_then(|r| r.last.as_ref()),
            ),
        }
    }

    /// Native workflow task frames. A run launches through the `Workflow`
    /// tool (always in the background), reports its phases and agents on
    /// `task_progress`, and settles on `task_notification` — the run's chip
    /// is refreshed in place ([`AgentEvent::WorkflowProgress`]) and settled
    /// like a background subagent (a tagged `Done`). `None` when the frame
    /// is not a workflow's.
    fn normalize_workflow(&mut self, f: &SystemFrame) -> Option<Vec<AgentEvent>> {
        match f.subtype.as_str() {
            "task_started" => {
                let tool = f.tool_use_id.as_deref().filter(|t| !t.is_empty());
                let is_workflow = f.task_type.as_deref() == Some("local_workflow")
                    || tool.is_some_and(|t| self.workflow_inputs.contains_key(t));
                if !is_workflow {
                    return None;
                }
                let Some(tool) = tool.map(str::to_owned) else {
                    return Some(Vec::new());
                };
                if let Some(task) = f.task_id.as_deref().filter(|t| !t.is_empty()) {
                    self.workflow_tasks.insert(task.to_owned(), tool.clone());
                }
                let run = self.workflow_runs.entry(tool.clone()).or_default();
                if let Some(name) = f.workflow_name.as_deref().filter(|n| !n.trim().is_empty()) {
                    run.name = Some(name.to_owned());
                }
                Some(vec![self.workflow_refresh(&tool)])
            }
            "task_progress" => {
                let tool = self.workflow_tool(f)?;
                let Some(progress) = f
                    .workflow_progress
                    .as_ref()
                    .and_then(super::workflow::reduce_progress)
                else {
                    return Some(Vec::new());
                };
                let run = self.workflow_runs.entry(tool.clone()).or_default();
                if run.last.as_ref() == Some(&progress) {
                    return Some(Vec::new());
                }
                run.last = Some(progress);
                Some(vec![self.workflow_refresh(&tool)])
            }
            "task_notification" => {
                let tool = self.workflow_tool(f)?;
                let Some(status) = task_done_status(f.status.as_deref()) else {
                    return Some(Vec::new());
                };
                self.workflow_inputs.remove(&tool);
                self.workflow_runs.remove(&tool);
                self.workflow_tasks.retain(|_, t| *t != tool);
                Some(vec![tag(&tool, settled(status))])
            }
            _ => None,
        }
    }

    /// Rotate the assistant message id for a steer boundary; returns
    /// (previous, next) for the `Steered` event.
    pub fn rotate_for_steer(&mut self) -> (String, String) {
        let prev = std::mem::replace(&mut self.assistant_message_id, new_message_id());
        (prev, self.assistant_message_id.clone())
    }

    /// Normalize one stdout frame into 0+ unified events. `interrupted` folds
    /// a post-interrupt `result` into `Done { status: Interrupted }`.
    pub fn normalize(&mut self, frame: Frame, interrupted: bool) -> Vec<AgentEvent> {
        match frame {
            Frame::System(f) => {
                if let Some(events) = self.normalize_workflow(&f) {
                    return events;
                }
                // A background subagent's completion arrives as an UNTAGGED
                // `task_notification` carrying the spawning tool's id — the
                // wire's only terminal signal for it (live-verified 2.1.228:
                // no tagged frame follows; the subagent's stream just stops).
                // Surface it as the subagent's tagged Done so the chip flips
                // done/failed and the transcript freezes.
                if f.subtype == "task_notification" {
                    // A resumed task's notification may carry SendMessage's
                    // id, or omit the tool id entirely. Its task id is stable.
                    let parent = f
                        .task_id
                        .as_deref()
                        .and_then(|task| self.agent_tasks.get(task).map(String::as_str))
                        .or_else(|| f.tool_use_id.as_deref().filter(|t| !t.is_empty()));
                    let Some(parent) = parent else {
                        return Vec::new();
                    };
                    // Only a KNOWN spawn settles as a subagent. Background
                    // SHELL tasks (`Bash` with `run_in_background`) settle
                    // through this same subtype carrying the Bash call's id —
                    // tagging that Done would bind a subagent ref onto an
                    // ordinary Run chip. Spawn ids are learned from live
                    // calls/task starts or restored from the native history.
                    if !self.agent_spawn_tools.contains(parent) {
                        return Vec::new();
                    }
                    // Non-terminal notification shapes: nothing to close.
                    let Some(status) = task_done_status(f.status.as_deref()) else {
                        return Vec::new();
                    };
                    return vec![tag(parent, settled(status))];
                }
                // An AGENT task starting (subagent_type present — subagent-
                // owned shell tasks carry the same subtype without it):
                // record agentId → spawn id for SendMessage steer re-keying.
                if f.subtype == "task_started"
                    && f.subagent_type.is_some()
                    && let (Some(task), Some(tool)) = (
                        f.task_id.as_deref().filter(|t| !t.is_empty()),
                        f.tool_use_id.as_deref().filter(|t| !t.is_empty()),
                    )
                {
                    let spawn = self
                        .agent_tasks
                        .entry(task.to_owned())
                        .or_insert_with(|| tool.to_owned());
                    self.agent_spawn_tools.insert(spawn.clone());
                    self.agent_tool_spawns
                        .insert(tool.to_owned(), spawn.clone());
                    return Vec::new();
                }
                if f.subtype != "init" || self.saw_init {
                    return Vec::new();
                }
                self.saw_init = true;
                self.session_id = Some(f.session_id.clone());
                vec![AgentEvent::SessionStarted {
                    harness: HarnessId::ClaudeCode,
                    model: f.model,
                    tools: f.tools,
                    cwd: f.cwd,
                    session_id: f.session_id,
                    assistant_message_id: self.assistant_message_id.clone(),
                }]
            }

            // Frames with `parent_tool_use_id` set belong to a SUBAGENT's
            // nested transcript: a background subagent runs concurrently with
            // the parent's own stream, so folding them into the parent feed
            // would split a contiguous text block around phantom tool calls
            // (and a subagent's message boundaries must never rotate the
            // parent's assistant message id). They are wrapped in
            // `AgentEvent::Subagent` instead — the engine routes them to the
            // subagent's own doc.
            Frame::StreamEvent(f) => {
                if f.event.kind != "content_block_delta" {
                    return Vec::new();
                }
                if let Some(parent) = &f.parent_tool_use_id {
                    let parent = self.agent_tool_spawns.get(parent).unwrap_or(parent);
                    return match f.event.delta.kind.as_str() {
                        "text_delta" => vec![tag(
                            parent,
                            AgentEvent::TextDelta {
                                text: f.event.delta.text,
                            },
                        )],
                        "thinking_delta" if !f.event.delta.thinking.is_empty() => vec![tag(
                            parent,
                            AgentEvent::ReasoningDelta {
                                text: f.event.delta.thinking,
                            },
                        )],
                        // Subagent liveness heartbeats have no consumer; the
                        // parent turn is already done (eager-done policy).
                        _ => Vec::new(),
                    };
                }
                match f.event.delta.kind.as_str() {
                    "text_delta" => vec![AgentEvent::TextDelta {
                        text: f.event.delta.text,
                    }],
                    "thinking_delta" => vec![AgentEvent::ReasoningDelta {
                        text: f.event.delta.thinking,
                    }],
                    // A big tool input (a 90-line Write) streams as a long run
                    // of input_json_delta frames with nothing else — minutes of
                    // apparent silence that reads as a stalled run. Surface
                    // them as empty reasoning deltas: the engine treats those
                    // as pure liveness heartbeats (never journaled/rendered).
                    "input_json_delta" => vec![AgentEvent::ReasoningDelta {
                        text: String::new(),
                    }],
                    _ => Vec::new(),
                }
            }

            Frame::Assistant(f) => {
                for b in f.message.blocks() {
                    if b.kind == "tool_use"
                        && let Some(diff) = file_diff(&b.name, &b.input)
                    {
                        self.file_diffs.insert(b.id.clone(), diff);
                    }
                }
                if let Some(parent) = &f.parent_tool_use_id {
                    let parent = self.agent_tool_spawns.get(parent).unwrap_or(parent);
                    // Subagent content, attributed. The 2.1.x wire streams NO
                    // tagged partial deltas (live-verified): a subagent's text
                    // arrives only as full text blocks on its tagged
                    // assistant frames — emit them, in block order with the
                    // tool calls, or subagent transcripts are tool-chips-only.
                    let mut out: Vec<AgentEvent> = f
                        .message
                        .blocks()
                        .filter_map(|b: ContentBlock| match b.kind.as_str() {
                            "text" if !b.text.is_empty() => Some(tag(
                                parent,
                                AgentEvent::TextDelta {
                                    text: format!("{}\n\n", b.text.trim_end()),
                                },
                            )),
                            "tool_use" => Some(tag(
                                parent,
                                AgentEvent::ToolCall {
                                    id: b.id.clone(),
                                    call: decode_tool_use(&b.name, &b.input),
                                },
                            )),
                            _ => None,
                        })
                        .collect();
                    if let Some(code) = &f.error {
                        out.push(tag(
                            parent,
                            AgentEvent::Error {
                                message: assistant_error_text(code),
                            },
                        ));
                    }
                    return out;
                }
                // Record spawn tool ids up front: `task_notification` keys on
                // them; a task may finish before a `task_started` arrives.
                for b in f.message.blocks() {
                    if b.kind == "tool_use" && matches!(b.name.as_str(), "Agent" | "Task") {
                        self.agent_spawn_tools.insert(b.id.clone());
                    }
                    if b.kind == "tool_use" && b.name == WORKFLOW_TOOL_USE {
                        self.workflow_inputs.insert(b.id.clone(), b.input.clone());
                    }
                }
                let mut out: Vec<AgentEvent> = f
                    .message
                    .blocks()
                    .filter(|b: &ContentBlock| b.kind == "tool_use")
                    .flat_map(|b| {
                        let call = AgentEvent::ToolCall {
                            id: b.id.clone(),
                            call: decode_tool_use(&b.name, &b.input),
                        };
                        // A spawn's `prompt` is the subagent's opening user
                        // message — the wire never echoes it on the child
                        // feed (child user frames carry tool results and
                        // steers only), so seed it here and the subagent
                        // transcript starts the way every chat does.
                        let opening = matches!(b.name.as_str(), "Agent" | "Task")
                            .then(|| b.input.get("prompt"))
                            .flatten()
                            .and_then(Value::as_str)
                            .filter(|p| !p.trim().is_empty())
                            .map(|prompt| {
                                tag(
                                    &b.id,
                                    AgentEvent::UserMessage {
                                        text: prompt.to_owned(),
                                    },
                                )
                            });
                        // A SendMessage steer never echoes on the child feed
                        // (live-verified) — surface it from the parent's own
                        // call, re-keyed onto the spawn it addresses.
                        let steer = (b.name == "SendMessage")
                            .then(|| {
                                let to = ["to", "recipient"]
                                    .iter()
                                    .find_map(|k| b.input.get(*k))
                                    .and_then(Value::as_str)?;
                                let spawn = self.agent_tasks.get(to)?;
                                let text = ["message", "content"]
                                    .iter()
                                    .find_map(|k| b.input.get(*k))
                                    .and_then(Value::as_str)
                                    .filter(|m| !m.trim().is_empty())?;
                                Some(tag(
                                    spawn,
                                    AgentEvent::UserMessage {
                                        text: text.to_owned(),
                                    },
                                ))
                            })
                            .flatten();
                        std::iter::once(call).chain(opening).chain(steer)
                    })
                    .collect();
                self.last_model = f.message.model.clone().or(self.last_model.take());
                if let Some(usage) = &f.message.usage {
                    let fields = [
                        "input_tokens",
                        "cache_read_input_tokens",
                        "cache_creation_input_tokens",
                    ];
                    if fields
                        .iter()
                        .any(|key| usage.get(*key).and_then(Value::as_u64).is_some())
                    {
                        let tokens = fields
                            .iter()
                            .filter_map(|key| usage.get(*key).and_then(Value::as_u64))
                            .fold(0u64, u64::saturating_add);
                        out.push(AgentEvent::ContextUsage {
                            tokens: Some(tokens),
                            window: None,
                        });
                    }
                }
                // A failed turn (usage limit, billing, auth, overloaded, …)
                // carries a terse `error` code here — often with empty content
                // and no `result` error — so surface it visibly.
                if let Some(code) = &f.error {
                    out.push(AgentEvent::Error {
                        message: assistant_error_text(code),
                    });
                }
                // The enclosing assistant frame closes the streamed message
                // item; rotate so post-boundary deltas get a fresh id.
                let (prev, _next) = self.rotate_for_steer();
                out.push(AgentEvent::AssistantMessageCompleted {
                    assistant_message_id: prev,
                });
                out
            }

            Frame::User(f) => {
                // A settled Write/Edit takes the diff its call described;
                // a failed one changed nothing.
                let mut diffs = std::collections::HashMap::new();
                for b in f.message.blocks() {
                    if b.kind == "tool_result"
                        && let Some(diff) = self.file_diffs.remove(&b.tool_use_id)
                        && !b.is_error.unwrap_or(false)
                    {
                        diffs.insert(b.tool_use_id.clone(), diff);
                    }
                }
                if let Some(parent) = &f.parent_tool_use_id {
                    let parent = self.agent_tool_spawns.get(parent).unwrap_or(parent);
                    // A subagent's tool results echo on the main channel too;
                    // they belong to its transcript, attributed like its calls.
                    let mut out: Vec<AgentEvent> = f
                        .message
                        .blocks()
                        .filter(|b: &ContentBlock| b.kind == "tool_result")
                        .map(|b| {
                            tag(
                                parent,
                                AgentEvent::ToolResult {
                                    id: b.tool_use_id.clone(),
                                    is_error: b.is_error.unwrap_or(false),
                                    output: crate::tool_output::text(&b.content),
                                    diff: diffs.remove(&b.tool_use_id),
                                },
                            )
                        })
                        .collect();
                    // A tagged user frame's TEXT blocks are the parent
                    // steering its subagent (SendMessage-style follow-ups —
                    // tool results ride their own blocks, filtered above).
                    // Synthetic harness injections are not conversation, and
                    // must not read as a steer — see [`is_synthetic_user_text`].
                    out.extend(
                        f.message
                            .blocks()
                            .filter(|b: &ContentBlock| {
                                b.kind == "text"
                                    && !b.text.trim().is_empty()
                                    && !is_synthetic_user_text(&b.text)
                            })
                            .map(|b| tag(parent, AgentEvent::UserMessage { text: b.text })),
                    );
                    return out;
                }
                let mut out = Vec::new();
                for b in f
                    .message
                    .blocks()
                    .filter(|b: &ContentBlock| b.kind == "tool_result")
                {
                    let is_error = b.is_error.unwrap_or(false);
                    out.push(AgentEvent::ToolResult {
                        id: b.tool_use_id.clone(),
                        is_error,
                        output: crate::tool_output::text(&b.content),
                        diff: diffs.remove(&b.tool_use_id),
                    });
                    // A rejected workflow launch never starts a task, so no
                    // notification will settle it: settle it here.
                    if is_error
                        && !self.workflow_runs.contains_key(&b.tool_use_id)
                        && self.workflow_inputs.remove(&b.tool_use_id).is_some()
                    {
                        out.push(tag(&b.tool_use_id, settled(DoneStatus::Errored)));
                    }
                }
                out
            }

            // A claude.ai plan window was hit. A hard `rejected` blocks the
            // turn — make it visible; allowed/allowed_warning stay quiet, and
            // so does a reject the account's overage carries past.
            Frame::RateLimit(f) => {
                if !f.rate_limit_info.blocks() {
                    return Vec::new();
                }
                let window =
                    rate_window_label(f.rate_limit_info.rate_limit_type.as_deref().unwrap_or(""));
                vec![AgentEvent::Error {
                    message: format!(
                        "Claude {window} limit reached — the turn was blocked. Try again after it resets."
                    ),
                }]
            }

            Frame::Result(f) => {
                let model_usage = self
                    .last_model
                    .as_ref()
                    .and_then(|model| {
                        f.model_usage.get(model).or_else(|| {
                            f.model_usage.values().find(|entry| {
                                entry.get("canonicalModel").and_then(Value::as_str)
                                    == Some(model.as_str())
                            })
                        })
                    })
                    .or_else(|| {
                        (f.model_usage.len() == 1)
                            .then(|| f.model_usage.values().next())
                            .flatten()
                    });
                let window = model_usage
                    .and_then(|entry| entry.get("contextWindow"))
                    .and_then(Value::as_u64)
                    .filter(|n| *n > 0);
                if let Some(id) = &f.session_id {
                    self.session_id = Some(id.clone());
                }
                let usage = AgentEvent::Usage {
                    input_tokens: f.usage.input_tokens,
                    output_tokens: f.usage.output_tokens,
                };
                let done = if f.subtype == "success" {
                    AgentEvent::Done {
                        status: if interrupted {
                            DoneStatus::Interrupted
                        } else {
                            DoneStatus::Completed
                        },
                        result: f.result,
                        error: None,
                        session_id: f.session_id,
                    }
                } else {
                    // Split the CLI's internal `[ede_diagnostic]` breadcrumbs
                    // off the real errors: diagnostics are debug-logged, never
                    // surfaced as transcript error parts.
                    let (diagnostics, errors): (Vec<String>, Vec<String>) = f
                        .errors
                        .iter()
                        .map(|e| match e {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        })
                        .partition(|m| is_internal_diagnostic(m));
                    for diagnostic in &diagnostics {
                        tracing::debug!(
                            target: "zuse_providers::claude",
                            "internal CLI diagnostic (not surfaced): {diagnostic}"
                        );
                    }
                    let error = if !errors.is_empty() {
                        // Real user-relevant errors — surface verbatim.
                        Some(errors.join("; "))
                    } else {
                        match f.subtype.as_str() {
                            // Known run-failure subtypes stay visible with
                            // their mapped human wording (never blank — a
                            // blank error folds to no part and the failed
                            // turn reads as a silent non-reply).
                            "error_max_turns"
                            | "error_max_budget_usd"
                            | "error_max_structured_output_retries" => {
                                Some(result_error_text(&f.subtype).to_owned())
                            }
                            // Diagnostic-only ends (the CLI's turn-accounting
                            // telemetry, typically `error_during_execution`
                            // after an abort): nothing user-relevant to show.
                            _ if !diagnostics.is_empty() => None,
                            _ => Some(result_error_text(&f.subtype).to_owned()),
                        }
                    };
                    AgentEvent::Done {
                        status: if interrupted {
                            DoneStatus::Interrupted
                        } else {
                            DoneStatus::Errored
                        },
                        result: None,
                        error,
                        session_id: f.session_id,
                    }
                };
                let mut out = Vec::new();
                if let Some(window) = window {
                    out.push(AgentEvent::ContextUsage {
                        tokens: None,
                        window: Some(window),
                    });
                }
                out.extend([usage, done]);
                out
            }

            // Control frames are handled by the run loop, not normalized.
            Frame::ControlRequest(_)
            | Frame::ControlResponse(_)
            | Frame::CommandLifecycle(_)
            | Frame::Other => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_typed_tools() {
        assert_eq!(
            decode_tool_use("Bash", &json!({"command": "ls -la"})),
            ToolCall::Exec {
                command: "ls -la".into()
            }
        );
        assert_eq!(
            decode_tool_use(
                "Edit",
                &json!({"file_path": "/a", "old_string": "x", "new_string": "y"})
            ),
            ToolCall::EditFile {
                path: "/a".into(),
                old_string: Some("x".into()),
                new_string: Some("y".into())
            }
        );
        assert_eq!(
            decode_tool_use(
                "TodoWrite",
                &json!({"todos": [{"content": "t", "status": "completed"}]})
            ),
            ToolCall::Todo {
                items: vec![TodoItem::new("t", TodoStatus::Completed)]
            }
        );
        // Claude's in-progress state survives; unknown/missing reads as pending.
        assert_eq!(
            decode_tool_use(
                "TodoWrite",
                &json!({"todos": [
                    {"content": "a", "status": "completed"},
                    {"content": "b", "status": "in_progress", "activeForm": "Doing b"},
                    {"content": "c", "status": "pending"},
                    {"content": "d"},
                ]})
            ),
            ToolCall::Todo {
                items: vec![
                    TodoItem::new("a", TodoStatus::Completed),
                    TodoItem::new("b", TodoStatus::InProgress),
                    TodoItem::new("c", TodoStatus::Pending),
                    TodoItem::new("d", TodoStatus::Pending),
                ]
            }
        );
        assert_eq!(
            decode_tool_use("mcp__linear__search", &json!({"q": "bug"})),
            ToolCall::Mcp {
                server: "linear".into(),
                tool: "search".into(),
                input: Some(json!({"q": "bug"}))
            }
        );
        assert!(matches!(
            decode_tool_use("Mystery", &json!({})),
            ToolCall::Unknown { .. }
        ));
    }

    fn normalize_one(raw: &str) -> Vec<AgentEvent> {
        let frame = crate::claude::wire::parse_frame(raw).expect("frame parses");
        Normalizer::new().normalize(frame, false)
    }

    #[test]
    fn a_rejected_window_carried_by_overage_is_not_an_error() {
        let blocked = normalize_one(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","overageStatus":"rejected"}}"#,
        );
        assert!(matches!(blocked.as_slice(), [AgentEvent::Error { .. }]));
        for overage in [
            r#""overageStatus":"allowed""#,
            r#""overageStatus":"allowed_warning""#,
            r#""isUsingOverage":true"#,
        ] {
            let raw = format!(
                r#"{{"type":"rate_limit_event","rate_limit_info":{{"status":"rejected","rateLimitType":"five_hour",{overage}}}}}"#
            );
            assert!(normalize_one(&raw).is_empty(), "{overage}");
        }
    }

    fn result_done(raw: &str) -> AgentEvent {
        let events = normalize_one(raw);
        assert_eq!(events.len(), 2, "usage + done");
        events.into_iter().nth(1).expect("done event")
    }

    #[test]
    fn stream_deltas_map_to_text_reasoning_and_heartbeats() {
        // Real thinking text streams as a reasoning delta.
        let ev = normalize_one(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hmm"}}}"#,
        );
        assert_eq!(ev, vec![AgentEvent::ReasoningDelta { text: "hmm".into() }]);
        // Redacted thinking (estimated_tokens only) yields the empty
        // heartbeat shape the engine filters.
        let ev = normalize_one(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"","estimated_tokens":50}}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::ReasoningDelta {
                text: String::new()
            }]
        );
        // A tool input being generated (input_json_delta) is a liveness
        // heartbeat, not silence — minutes of a big Write must not read as
        // a stalled run.
        let ev = normalize_one(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"file_"}}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::ReasoningDelta {
                text: String::new()
            }]
        );
        // Signature deltas stay dropped.
        let ev = normalize_one(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"signature_delta","signature":"abc"}}}"#,
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn subagent_stream_deltas_arrive_tagged() {
        let ev = normalize_one(
            r#"{"type":"stream_event","parent_tool_use_id":"toolu_sub","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"sub text"}}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::TextDelta {
                    text: "sub text".into()
                }),
            }]
        );
        // Subagent input_json heartbeats are dropped — the parent turn is
        // already done under the eager-done policy, nothing to keep alive.
        let ev = normalize_one(
            r#"{"type":"stream_event","parent_tool_use_id":"toolu_sub","event":{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{"}}}"#,
        );
        assert!(ev.is_empty());
    }

    #[test]
    fn subagent_tool_calls_and_results_arrive_tagged_without_boundary_rotation() {
        let mut norm = Normalizer::new();
        let before = norm.assistant_message_id.clone();
        let frame = crate::claude::wire::parse_frame(
            r#"{"type":"assistant","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
        )
        .expect("parses");
        let ev = norm.normalize(frame, false);
        assert_eq!(
            ev,
            vec![AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::ToolCall {
                    id: "t1".into(),
                    call: ToolCall::Exec {
                        command: "ls".into()
                    },
                }),
            }]
        );
        // A subagent's assistant frame must NOT rotate the parent's message
        // id (it would split the parent's contiguous text mid-stream).
        assert_eq!(norm.assistant_message_id, before);

        let frame = crate::claude::wire::parse_frame(
            r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false}]}}"#,
        )
        .expect("parses");
        let ev = norm.normalize(frame, false);
        assert_eq!(
            ev,
            vec![AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::ToolResult {
                    id: "t1".into(),
                    is_error: false,
                    output: None,
                    diff: None,
                }),
            }]
        );
    }

    #[test]
    fn spawn_prompt_seeds_the_subagent_opening_user_message() {
        // The wire never echoes a Task's prompt on the child feed, so the
        // spawn itself seeds the subagent's opening user entry.
        let ev = normalize_one(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_sub","name":"Task","input":{"description":"probe","prompt":"scan the fold path"}}]}}"#,
        );
        assert!(matches!(
            &ev[..],
            [
                AgentEvent::ToolCall { id, .. },
                AgentEvent::Subagent { parent_tool_use_id, event },
                AgentEvent::AssistantMessageCompleted { .. },
            ] if id == "toolu_sub"
                && parent_tool_use_id == "toolu_sub"
                && matches!(event.as_ref(), AgentEvent::UserMessage { text } if text == "scan the fold path")
        ));
        // No prompt → no synthetic opening; ordinary tools never spawn one.
        for frame in [
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Task","input":{"description":"probe"}}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"ls","prompt":"red herring"}}]}}"#,
        ] {
            let ev = normalize_one(frame);
            assert!(
                !ev.iter().any(|e| matches!(e, AgentEvent::Subagent { .. })),
                "{frame}: {ev:?}"
            );
        }
    }

    /// Killing a subagent puts `done{interrupted}` on the child feed and then
    /// an interruption MARKER as a tagged user frame. Read as a steer, that
    /// marker resurrects the spawn chip the `done` just settled — and nothing
    /// ever settles it again, so the chip spins forever. It is CLI
    /// bookkeeping, filtered like a `<system-reminder>`; a real steer on the
    /// same frame shape still gets through.
    #[test]
    fn the_interruption_marker_is_not_a_steer() {
        for marker in [
            "[Request interrupted by user]",
            "[Request interrupted by user for tool use]",
        ] {
            let frame = format!(
                r#"{{"type":"user","parent_tool_use_id":"toolu_spawn","message":{{"content":[{{"type":"text","text":"{marker}"}}]}}}}"#
            );
            assert!(
                !normalize_one(&frame)
                    .iter()
                    .any(|e| matches!(e, AgentEvent::Subagent { .. })),
                "{marker} leaked as a steer"
            );
        }
        // A genuine steer on the very same frame shape still arrives.
        let real = r#"{"type":"user","parent_tool_use_id":"toolu_spawn","message":{"content":[{"type":"text","text":"Keep going."}]}}"#;
        assert!(
            normalize_one(real).iter().any(|e| matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if parent_tool_use_id == "toolu_spawn"
                        && matches!(event.as_ref(), AgentEvent::UserMessage { text } if text == "Keep going.")
            )),
            "a real steer must still reach the subagent"
        );
    }

    #[test]
    fn send_message_steers_rekey_onto_the_spawn_feed() {
        // Live 2.1.228: the steer NEVER echoes on the child feed; the only
        // wire evidence is the parent's SendMessage call addressed to the
        // agent id that task_started paired with the spawn tool id.
        let mut norm = Normalizer::new();
        let started = crate::claude::wire::parse_frame(
            r#"{"type":"system","subtype":"task_started","task_id":"a20b2336","tool_use_id":"toolu_spawn","subagent_type":"general-purpose","prompt":"p","description":"d"}"#,
        )
        .expect("parses");
        assert!(norm.normalize(started, false).is_empty());
        let send = crate::claude::wire::parse_frame(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_send","name":"SendMessage","input":{"to":"a20b2336","message":"Also read the rebuild.","summary":"s"}}]}}"#,
        )
        .expect("parses");
        let ev = norm.normalize(send, false);
        assert!(
            ev.iter().any(|e| matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if parent_tool_use_id == "toolu_spawn"
                        && matches!(event.as_ref(), AgentEvent::UserMessage { text } if text == "Also read the rebuild.")
            )),
            "{ev:?}"
        );
        // Unknown recipient (no task_started seen) or a subagent-owned shell
        // task's task_started: no steer synthesized.
        let mut norm = Normalizer::new();
        let shell_task = crate::claude::wire::parse_frame(
            r#"{"type":"system","subtype":"task_started","task_id":"bg1","tool_use_id":"toolu_bash","task_type":"local_bash"}"#,
        )
        .expect("parses");
        assert!(norm.normalize(shell_task, false).is_empty());
        let send = crate::claude::wire::parse_frame(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"SendMessage","input":{"to":"bg1","message":"x"}}]}}"#,
        )
        .expect("parses");
        assert!(
            !norm
                .normalize(send, false)
                .iter()
                .any(|e| matches!(e, AgentEvent::Subagent { .. }))
        );
    }

    #[test]
    fn tagged_user_text_becomes_a_subagent_steer() {
        // A tagged user frame's TEXT block is the parent steering its
        // subagent — forwarded as a tagged UserMessage so the subagent doc
        // grows a user entry.
        let ev = normalize_one(
            r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"text","text":"Also check the rebuild path."}]}}"#,
        );
        assert_eq!(
            ev,
            vec![AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::UserMessage {
                    text: "Also check the rebuild path.".into(),
                }),
            }]
        );
        // Synthetic harness injections are not conversation, and blank text
        // is noise; an UNTAGGED user text frame is not a steer at all (the
        // parent chat's user messages come from doc commands).
        for frame in [
            r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"text","text":"<system-reminder>tick</system-reminder>"}]}}"#,
            r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"text","text":"   "}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"text","text":"typed into the parent"}]}}"#,
        ] {
            assert_eq!(normalize_one(frame), Vec::new(), "frame: {frame}");
        }
        // Mixed frames keep both: the tool result AND the steer text.
        let ev = normalize_one(
            r#"{"type":"user","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false},{"type":"text","text":"Keep going."}]}}"#,
        );
        assert!(matches!(
            &ev[..],
            [
                AgentEvent::Subagent { event: first, .. },
                AgentEvent::Subagent { event: second, .. },
            ] if matches!(first.as_ref(), AgentEvent::ToolResult { .. })
                && matches!(second.as_ref(), AgentEvent::UserMessage { text } if text == "Keep going.")
        ));
    }

    #[test]
    fn task_notification_settles_the_subagent_with_a_tagged_done() {
        // The wire's ONLY terminal signal for a background subagent
        // (live-verified 2.1.228): an untagged system frame carrying the
        // spawning tool's id. Shape from the captured fixture. The spawn's
        // own tool_use frame always precedes it — that's what marks the id
        // as an AGENT task (shell tasks share the subtype).
        let spawn = |norm: &mut Normalizer| {
            let frame = crate::claude::wire::parse_frame(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_agent","name":"Agent","input":{"description":"probe"}}]}}"#,
            )
            .expect("parses");
            norm.normalize(frame, false);
        };
        let notify = |norm: &mut Normalizer, raw: &str| {
            let frame = crate::claude::wire::parse_frame(raw).expect("parses");
            norm.normalize(frame, false)
        };
        let mut norm = Normalizer::new();
        spawn(&mut norm);
        let ev = notify(
            &mut norm,
            r#"{"type":"system","subtype":"task_notification","task_id":"t1","tool_use_id":"toolu_agent","status":"completed","summary":"DONE."}"#,
        );
        assert!(matches!(
            &ev[..],
            [AgentEvent::Subagent { parent_tool_use_id, event }]
                if parent_tool_use_id == "toolu_agent"
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })
        ));
        let mut norm = Normalizer::new();
        spawn(&mut norm);
        let ev = notify(
            &mut norm,
            r#"{"type":"system","subtype":"task_notification","tool_use_id":"toolu_agent","status":"failed"}"#,
        );
        assert!(matches!(
            &ev[..],
            [AgentEvent::Subagent { event, .. }]
                if matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Errored, .. })
        ));
        // Non-terminal or id-less notifications close nothing.
        let mut norm = Normalizer::new();
        spawn(&mut norm);
        assert!(notify(
            &mut norm,
            r#"{"type":"system","subtype":"task_notification","tool_use_id":"toolu_agent","status":"running"}"#,
        )
        .is_empty());
        assert!(
            normalize_one(
                r#"{"type":"system","subtype":"task_notification","status":"completed"}"#,
            )
            .is_empty()
        );
    }

    #[test]
    fn resumed_task_notifications_settle_the_original_spawn() {
        for notification in [
            r#"{"type":"system","subtype":"task_notification","task_id":"a1","tool_use_id":"toolu_send","status":"completed"}"#,
            r#"{"type":"system","subtype":"task_notification","task_id":"a1","status":"stopped"}"#,
        ] {
            let mut norm = Normalizer::new();
            for raw in [
                r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_spawn","subagent_type":"general-purpose"}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_send","name":"SendMessage","input":{"to":"a1","message":"Keep going."}}]}}"#,
                r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_send","subagent_type":"general-purpose"}"#,
            ] {
                norm.normalize(crate::claude::wire::parse_frame(raw).unwrap(), false);
            }
            let events = norm.normalize(
                crate::claude::wire::parse_frame(notification).unwrap(),
                false,
            );
            let expected = if notification.contains("completed") {
                DoneStatus::Completed
            } else {
                DoneStatus::Interrupted
            };
            assert_eq!(
                events,
                vec![tag(
                    "toolu_spawn",
                    AgentEvent::Done {
                        status: expected,
                        result: None,
                        error: None,
                        session_id: None,
                    }
                )]
            );
            // The next steer must still use the original transcript too.
            let events = norm.normalize(crate::claude::wire::parse_frame(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_send2","name":"SendMessage","input":{"to":"a1","message":"One more check."}}]}}"#,
            ).unwrap(), false);
            assert!(
                events.iter().any(|e| matches!(e,
                    AgentEvent::Subagent { parent_tool_use_id, event }
                        if parent_tool_use_id == "toolu_spawn"
                            && matches!(event.as_ref(), AgentEvent::UserMessage { .. })
                )),
                "{events:?}"
            );
        }
    }

    #[test]
    fn resumed_child_frames_use_the_original_transcript() {
        let mut norm = Normalizer::new();
        for raw in [
            r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_spawn","subagent_type":"general-purpose"}"#,
            r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_send","subagent_type":"general-purpose"}"#,
        ] {
            norm.normalize(crate::claude::wire::parse_frame(raw).unwrap(), false);
        }
        for raw in [
            r#"{"type":"stream_event","parent_tool_use_id":"toolu_send","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"working"}}}"#,
            r#"{"type":"assistant","parent_tool_use_id":"toolu_send","message":{"content":[{"type":"text","text":"finished"}]}}"#,
            r#"{"type":"user","parent_tool_use_id":"toolu_send","message":{"content":[{"type":"tool_result","tool_use_id":"child-tool"}]}}"#,
        ] {
            let events = norm.normalize(crate::claude::wire::parse_frame(raw).unwrap(), false);
            assert!(!events.is_empty());
            assert!(events.iter().all(|e| matches!(e,
                AgentEvent::Subagent { parent_tool_use_id, .. } if parent_tool_use_id == "toolu_spawn"
            )), "{events:?}");
        }
    }

    #[tokio::test]
    async fn cold_resume_restores_only_task_ids_without_republishing_native_text() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("projects/encoded-project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("session-1.jsonl"), concat!(
            "not json\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"retired-credential-not-in-current-registry"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_spawn","name":"Agent","input":{"description":"scout"}}]}}"#,
            "\n",
            r#"{"type":"user","toolUseResult":{"agentId":"a1","status":"async_launched"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_spawn"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_send","name":"SendMessage","input":{"to":"a1","message":"continue"}}]}}"#,
            "\n",
            r#"{"type":"user","toolUseResult":{"agentId":"a1"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_send"}]}}"#,
            "\n",
            // A subagent's own nested spawn is not a chip on this chat.
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"tool_use","id":"toolu_nested","name":"Agent","input":{"description":"deeper"}}]}}"#,
            "\n",
            r#"{"type":"user","isSidechain":true,"toolUseResult":{"agentId":"a2"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_nested"}]}}"#,
            "\n{torn tail"
        )).unwrap();
        let mut norm = Normalizer::for_resume(root.path(), "session-1").await;
        assert!(!norm.agent_spawn_tools.contains("toolu_nested"));
        assert!(!norm.agent_tasks.contains_key("a2"));
        // The revived process has no original spawn frame on its wire.
        norm.normalize(crate::claude::wire::parse_frame(
            r#"{"type":"system","subtype":"task_started","task_id":"a1","tool_use_id":"toolu_send","subagent_type":"general-purpose"}"#,
        ).unwrap(), false);
        let events = norm.normalize(crate::claude::wire::parse_frame(
            r#"{"type":"system","subtype":"task_notification","task_id":"a1","tool_use_id":"toolu_send","status":"completed"}"#,
        ).unwrap(), false);
        assert_eq!(
            events,
            vec![tag(
                "toolu_spawn",
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                }
            )]
        );
        assert!(!norm.saw_init, "history must not consume the live init");
        assert!(
            Normalizer::for_resume(root.path(), "missing")
                .await
                .agent_tasks
                .is_empty()
        );
    }

    #[test]
    fn shell_task_notification_never_settles_a_subagent() {
        // `Bash` with `run_in_background` settles through the SAME
        // `task_notification` subtype, carrying the Bash call's own id.
        // Tagging that Done bound a subagent ref onto an ordinary Run chip,
        // which then rendered as a spawn chip opening an empty, never-created
        // subagent doc (user report 2026-08-20).
        let mut norm = Normalizer::new();
        for raw in [
            // The shell task's start is already unmapped (no subagent_type)…
            r#"{"type":"system","subtype":"task_started","task_id":"bg1","tool_use_id":"toolu_bash","task_type":"local_bash"}"#,
            // …and the Bash call itself must not mark the id as a spawn.
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_bash","name":"Bash","input":{"command":"git clone …","run_in_background":true}}]}}"#,
        ] {
            let frame = crate::claude::wire::parse_frame(raw).expect("parses");
            norm.normalize(frame, false);
        }
        let done = crate::claude::wire::parse_frame(
            r#"{"type":"system","subtype":"task_notification","task_id":"bg1","tool_use_id":"toolu_bash","status":"completed"}"#,
        )
        .expect("parses");
        assert!(norm.normalize(done, false).is_empty());
    }

    #[test]
    fn subagent_assistant_text_blocks_emit_tagged_text() {
        // No tagged partial deltas exist on the 2.1.x wire: a subagent's text
        // arrives only as full blocks on its tagged assistant frames.
        let ev = normalize_one(
            r#"{"type":"assistant","parent_tool_use_id":"toolu_sub","message":{"content":[{"type":"text","text":"working on it"},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}"#,
        );
        assert_eq!(ev.len(), 2);
        assert_eq!(
            ev[0],
            AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::TextDelta {
                    text: "working on it\n\n".into()
                }),
            }
        );
        assert!(matches!(&ev[1], AgentEvent::Subagent { event, .. }
            if matches!(event.as_ref(), AgentEvent::ToolCall { .. })));
    }

    #[test]
    fn wake_turn_init_is_deduped_but_second_result_still_emits_done() {
        // Live-verified 2.1.228 shape: background subagent → eager result,
        // then a second init (same session id) + second result on completion.
        let mut norm = Normalizer::new();
        let init = r#"{"type":"system","subtype":"init","model":"m","cwd":"/x","session_id":"s1"}"#;
        let frame = crate::claude::wire::parse_frame(init).unwrap();
        assert_eq!(norm.normalize(frame, false).len(), 1, "first init");
        let frame = crate::claude::wire::parse_frame(init).unwrap();
        assert!(
            norm.normalize(frame, false).is_empty(),
            "wake init deduped — SessionStarted is the fold's run boundary"
        );
        let result = r#"{"type":"result","subtype":"success","session_id":"s1"}"#;
        let frame = crate::claude::wire::parse_frame(result).unwrap();
        let events = norm.normalize(frame, false);
        assert!(
            matches!(
                events.last(),
                Some(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    ..
                })
            ),
            "wake turn settles with its own Done"
        );
    }

    #[test]
    fn ede_diagnostics_never_surface_as_errors() {
        // The CLI's internal turn-accounting breadcrumbs must not become
        // transcript error parts (they showed up as raw red boxes).
        let done = result_done(
            r#"{"type":"result","subtype":"error_during_execution","errors":["[ede_diagnostic] result_type=user last_content_type=n/a stop_reason=null"]}"#,
        );
        match done {
            AgentEvent::Done { status, error, .. } => {
                assert_eq!(status, DoneStatus::Errored);
                assert_eq!(error, None, "diagnostic-only failure surfaces no text");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn real_errors_survive_diagnostic_filtering() {
        let done = result_done(
            r#"{"type":"result","subtype":"error_during_execution","errors":["[ede_diagnostic] turn aborted (x) stop_reason=null","Something real broke"]}"#,
        );
        match done {
            AgentEvent::Done { error, .. } => {
                assert_eq!(error.as_deref(), Some("Something real broke"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn known_failure_subtypes_keep_mapped_wording() {
        // A known run-failure subtype stays visible with human wording even
        // when its errors array is all diagnostics (or empty).
        let done = result_done(
            r#"{"type":"result","subtype":"error_max_turns","errors":["[ede_diagnostic] turn aborted (max) stop_reason=null"]}"#,
        );
        match done {
            AgentEvent::Done { error, .. } => {
                assert_eq!(
                    error.as_deref(),
                    Some("The run hit the maximum number of turns.")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
        let done = result_done(r#"{"type":"result","subtype":"error_max_turns","errors":[]}"#);
        match done {
            AgentEvent::Done { error, .. } => {
                assert_eq!(
                    error.as_deref(),
                    Some("The run hit the maximum number of turns.")
                );
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }
}

#[cfg(test)]
mod context_tests {
    use super::*;
    #[test]
    fn context_counts_cached_prompt_and_matches_primary_model() {
        let mut normalizer = Normalizer::new();
        let events = normalizer.normalize(super::super::wire::parse_frame(r#"{"type":"assistant","message":{"model":"primary","content":[],"usage":{"input_tokens":200,"cache_read_input_tokens":40000,"cache_creation_input_tokens":1800,"output_tokens":100}}}"#).unwrap(), false);
        assert!(events.contains(&AgentEvent::ContextUsage {
            tokens: Some(42000),
            window: None
        }));
        let events = normalizer.normalize(super::super::wire::parse_frame(r#"{"type":"assistant","parent_tool_use_id":"child","message":{"model":"child","content":[],"usage":{"input_tokens":999999}}}"#).unwrap(), false);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ContextUsage { .. }))
        );
        let events = normalizer.normalize(super::super::wire::parse_frame(r#"{"type":"result","subtype":"success","usage":{"input_tokens":999999},"modelUsage":{"primary":{"contextWindow":200000},"child":{"contextWindow":1000000}}}"#).unwrap(), false);
        assert!(events.contains(&AgentEvent::ContextUsage {
            tokens: None,
            window: Some(200000)
        }));
        assert!(!events.iter().any(|e| matches!(
            e,
            AgentEvent::ContextUsage {
                tokens: Some(_),
                ..
            }
        )));
    }

    fn run_frames(norm: &mut Normalizer, frames: &[&str]) -> Vec<AgentEvent> {
        frames
            .iter()
            .flat_map(|raw| {
                let frame = crate::claude::wire::parse_frame(raw).expect("frame parses");
                norm.normalize(frame, false)
            })
            .collect()
    }

    fn workflow_refreshes(events: &[AgentEvent]) -> Vec<(&str, &ToolCall)> {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::WorkflowProgress { tool_use_id, call } => {
                    Some((tool_use_id.as_str(), call))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn write_and_edit_results_carry_the_diff_their_call_described() {
        let events = run_frames(
            &mut Normalizer::new(),
            &[
                r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"w1","name":"Write","input":{"file_path":"/repo/plan.md","content":"Plan\n"}},{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/repo/a.rs","old_string":"old","new_string":"new"}},{"type":"tool_use","id":"e2","name":"Edit","input":{"file_path":"/repo/b.rs","old_string":"x","new_string":"y"}}]}}"#,
                r#"{"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":"w1","content":"File created"},{"type":"tool_result","tool_use_id":"e1","content":"Updated"},{"type":"tool_result","tool_use_id":"e2","content":"String not found","is_error":true}]}}"#,
            ],
        );
        let diff = |want: &str| {
            events.iter().find_map(|event| match event {
                AgentEvent::ToolResult { id, diff, .. } if id == want => Some(diff.clone()),
                _ => None,
            })
        };
        assert_eq!(
            diff("w1"),
            Some(Some(ToolDiff {
                path: "/repo/plan.md".into(),
                old_text: None,
                new_text: "Plan\n".into(),
            }))
        );
        assert_eq!(
            diff("e1"),
            Some(Some(ToolDiff {
                path: "/repo/a.rs".into(),
                old_text: Some("old".into()),
                new_text: "new".into(),
            }))
        );
        // A failed edit changed nothing, so it shows no diff.
        assert_eq!(diff("e2"), Some(None));
    }

    #[test]
    fn rejected_workflow_launch_settles_instead_of_running_forever() {
        let events = run_frames(
            &mut Normalizer::new(),
            &[
                r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"rejected-workflow","name":"Workflow","input":{"name":"audit"}}]}}"#,
                r#"{"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":"rejected-workflow","content":"Invalid workflow script","is_error":true}]}}"#,
            ],
        );
        assert!(matches!(
            &events[0],
            AgentEvent::ToolCall { id, call } if id == "rejected-workflow" && call.is_workflow_run()
        ));
        assert!(events.contains(&AgentEvent::ToolResult {
            id: "rejected-workflow".into(),
            is_error: true,
            output: Some("Invalid workflow script".into()),
            diff: None,
        }));
        assert_eq!(
            events.last(),
            Some(&tag("rejected-workflow", settled(DoneStatus::Errored)))
        );
        // An accepted launch's (non-error) result settles nothing.
        let events = run_frames(
            &mut Normalizer::new(),
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"wf","name":"Workflow","input":{"name":"audit"}}]}}"#,
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"wf","content":"Started"}]}}"#,
            ],
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::Subagent { .. }))
        );
    }

    #[test]
    fn running_workflow_refreshes_with_its_phase_and_agent_snapshot() {
        use zuse_protocol::WorkflowAgentState;
        let progress = |tick: u32, state: &str, extra: &str| {
            format!(
                r#"{{"type":"system","subtype":"task_progress","task_id":"task-1","tool_use_id":"workflow-1","description":"Map: map:files","usage":{{"total_tokens":10,"tool_uses":1,"duration_ms":5}},"workflow_progress":[{{"type":"workflow_phase","index":1,"title":"Map"}},{{"type":"workflow_agent","index":1,"label":"map:files","phaseIndex":1,"phaseTitle":"Map","state":"{state}","startedAt":100,"promptPreview":"List files","lastProgressAt":{tick},"tokens":{tokens}{extra}}},{{"type":"workflow_log","message":"ignored"}}]}}"#,
                tokens = tick * 10
            )
        };
        let frames = [
            r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"workflow-1","name":"Workflow","input":{"script":"export const meta = { name: 'audit' };"}}]}}"#.to_owned(),
            r#"{"type":"system","subtype":"task_started","task_id":"task-1","tool_use_id":"workflow-1","task_type":"local_workflow","workflow_name":"audit","description":"Audit"}"#.to_owned(),
            progress(1, "progress", ""),
            progress(2, "progress", ""),
            progress(3, "done", r#","durationMs":40,"resultPreview":"3 files""#),
        ];
        let frames: Vec<&str> = frames.iter().map(String::as_str).collect();
        let events = run_frames(&mut Normalizer::new(), &frames);
        let refreshes = workflow_refreshes(&events);
        // task_started names the run; two distinct snapshots follow — the
        // repeated one (only counters moved) is deduped.
        assert_eq!(refreshes.len(), 3, "{refreshes:?}");
        let (id, started) = refreshes[0];
        assert_eq!(id, "workflow-1");
        assert_eq!(started.workflow_name(), Some("audit"));
        assert_eq!(started.workflow_progress(), None);

        let (_, first) = refreshes[1];
        assert!(first.is_workflow_run());
        let ToolCall::Unknown {
            input: Some(input), ..
        } = first
        else {
            panic!("{first:?}")
        };
        // The launch input rides along, so the chip keeps its source.
        assert_eq!(input["script"], "export const meta = { name: 'audit' };");
        assert_eq!(input["workflow_name"], "audit");
        let snapshot = first.workflow_progress().expect("snapshot");
        assert_eq!(snapshot.phases.len(), 1);
        assert_eq!(snapshot.phases[0].title, "Map");
        let agent = &snapshot.agents[0];
        assert_eq!(
            (agent.index, agent.label.as_str(), agent.state),
            (1, "map:files", WorkflowAgentState::Running)
        );
        assert_eq!(
            (agent.phase_index, agent.phase_title.as_deref()),
            (Some(1), Some("Map"))
        );
        assert_eq!(agent.started_at, Some(100));
        assert_eq!(agent.prompt_preview.as_deref(), Some("List files"));

        let done = refreshes[2].1.workflow_progress().expect("snapshot");
        assert_eq!(done.agents[0].state, WorkflowAgentState::Done);
        assert_eq!(done.agents[0].duration_ms, Some(40));
        assert_eq!(done.agents[0].result_preview.as_deref(), Some("3 files"));
        // Progress never opens a subagent transcript.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::Subagent { .. }))
        );
    }

    #[test]
    fn workflow_keeps_identity_and_source_through_background_completion() {
        let events = run_frames(
            &mut Normalizer::new(),
            &[
                r#"{"type":"assistant","parent_tool_use_id":null,"message":{"content":[{"type":"tool_use","id":"workflow-1","name":"Workflow","input":{"script":"export const meta = { name: 'audit', phases: [] };","resumeFromRunId":"prior-run"}}]}}"#,
                r#"{"type":"system","subtype":"task_started","task_id":"run-1","tool_use_id":"workflow-1","task_type":"local_workflow","workflow_name":"audit","description":"Audit the project"}"#,
                // Settles by task id alone.
                r#"{"type":"system","subtype":"task_notification","task_id":"run-1","status":"failed","summary":"Build failed","usage":{"duration_ms":2000,"total_tokens":1,"tool_uses":1}}"#,
                // A straggler after the settle is not a workflow's any more.
                r#"{"type":"system","subtype":"task_progress","task_id":"run-1","tool_use_id":"workflow-1","workflow_progress":[{"type":"workflow_phase","index":1,"title":"Map"}]}"#,
            ],
        );
        assert!(matches!(
            &events[0],
            AgentEvent::ToolCall { id, call } if id == "workflow-1" && call.is_workflow_run()
        ));
        let refreshes = workflow_refreshes(&events);
        assert_eq!(refreshes.len(), 1);
        let ToolCall::Unknown {
            input: Some(input), ..
        } = refreshes[0].1
        else {
            panic!("{refreshes:?}")
        };
        assert_eq!(input["resumeFromRunId"], "prior-run");
        assert_eq!(input["workflow_name"], "audit");
        assert_eq!(
            events.last(),
            Some(&tag("workflow-1", settled(DoneStatus::Errored)))
        );
    }

    #[test]
    fn background_shell_task_frames_are_never_workflow_traffic() {
        let events = run_frames(
            &mut Normalizer::new(),
            &[
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_bash","name":"Bash","input":{"command":"sleep 5","run_in_background":true}}]}}"#,
                r#"{"type":"system","subtype":"task_started","task_id":"b1","tool_use_id":"toolu_bash","task_type":"local_bash"}"#,
                r#"{"type":"system","subtype":"task_progress","task_id":"b1","tool_use_id":"toolu_bash","workflow_progress":[{"type":"workflow_phase","index":1,"title":"x"}]}"#,
                r#"{"type":"system","subtype":"task_notification","task_id":"b1","tool_use_id":"toolu_bash","status":"completed"}"#,
            ],
        );
        assert_eq!(events.len(), 2, "{events:?}"); // the call + context bookkeeping only
        assert!(workflow_refreshes(&events).is_empty());
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::Subagent { .. }))
        );
    }

    #[tokio::test]
    async fn resumed_session_restores_workflow_launches() {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("projects/p");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join("session-w.jsonl"),
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_wf","name":"Workflow","input":{"name":"audit"}}]}}"#,
        )
        .unwrap();
        let mut norm = Normalizer::for_resume(root.path(), "session-w").await;
        let events = run_frames(
            &mut norm,
            &[
                r#"{"type":"system","subtype":"task_notification","tool_use_id":"toolu_wf","status":"completed"}"#,
            ],
        );
        assert_eq!(
            events,
            vec![tag("toolu_wf", settled(DoneStatus::Completed))]
        );
    }
}
