//! Devin subagent visualization over its ACP extension.
//!
//! Advertising `clientCapabilities._meta["cognition.ai/subagentSupport"]`
//! unlocks three tags on ordinary `session/update` frames (verified against
//! Devin CLI 3000.6.14): the parent's `run_subagent` tool call is followed by
//! `subagent_started`; child traffic carries `subagent_context.parentAgentId`;
//! and `subagent_completed` settles it. The stable child agent id is correlated
//! back to the parent's spawn tool-call id so nested traffic never leaks into
//! the parent transcript.
//!
//! Devin CLI 3000.14 (verified live) adds two wrinkles. Every lead
//! `usage_update` is ALSO emitted tagged `subagent_context.parentAgentId:
//! "root"` — the lead itself, not a child — so "root" routes to the plain
//! transcript. And the Fusion sidekick's own tool frames arrive as ordinary
//! `tool_call`/`tool_call_update`s whose `toolCallId` starts with `sk::` (or
//! whose `_meta` carries `cognition.ai/sidekick: true`), its usage under
//! `parentAgentId: "sidekick"`, bracketed by `_cognition.ai/sidekick/wait`
//! notifications. The lead's `sidekick` tool call never reaches ACP, so each
//! handoff gets a synthesized `Agent: Sidekick` spawn chip that settles with
//! the handoff (`sidekick_wait`) or the turn (`turn_settled`).

use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;
use zuse_protocol::{AgentEvent, DoneStatus, ToolCall};

use super::normalize::map_update;

/// `subagent_context.parentAgentId` values that are NOT `run_subagent`
/// children: "root" is the lead itself (its usage_update is duplicated
/// under this tag), "sidekick" is the Fusion sidekick.
const ROOT: &str = "root";
const SIDEKICK: &str = "sidekick";

/// Prefix of the synthesized sidekick chip ids. The run loop uses it to
/// tell a chip's ledger entries apart from the lead's own open tools —
/// a sidekick chip must not hold a deferred `Steered` boundary.
pub(crate) const SIDEKICK_CHIP_PREFIX: &str = "devin-sidekick-";

#[derive(Debug)]
struct PendingSpawn {
    tool_call_id: String,
    title: String,
}

#[derive(Default)]
pub(crate) struct DevinTracker {
    pending: VecDeque<PendingSpawn>,
    /// Devin child agent id -> the parent's run_subagent tool-call id.
    bound: HashMap<String, String>,
    /// Child ids that emitted assistant text; completion summaries are
    /// fallback text only and must not duplicate a streamed final answer.
    saw_text: HashSet<String>,
    /// Completion can race the final child frames; never reopen a settled
    /// transcript under the child id after its parent binding is gone.
    settled: HashSet<String>,
    /// The currently open sidekick chip — one per handoff.
    sidekick_chip: Option<String>,
    /// Sidekick toolCallId -> owning chip, kept after the chip settles so a
    /// late frame tags the OLD chip (the engine drops stragglers) instead of
    /// opening a new one.
    sidekick_tools: HashMap<String, String>,
    /// Sidekick toolCallId -> owning chip for calls that have NOT reported
    /// a terminal result yet. A wait-boundary settlement defers while its
    /// chip still has entries here.
    sidekick_open: HashMap<String, String>,
    /// Chips whose wait-boundary settlement deferred because their tools
    /// were still running, oldest first. Each emits its Done + chip
    /// ToolResult once its last open tool reports (or unconditionally at
    /// turn/finish settle — the backstop so nothing waits forever).
    deferred_sidekick: VecDeque<String>,
}

impl DevinTracker {
    pub(crate) fn map(&mut self, update: &Value) -> Vec<AgentEvent> {
        if let Some(started) = meta(update, "cognition.ai/subagent_started") {
            let Some(agent_id) = nonempty(started, "agentId") else {
                return Vec::new();
            };
            let title = nonempty(started, "title").unwrap_or_default();
            let index = self
                .pending
                .iter()
                .position(|p| !title.is_empty() && p.title == title)
                .or((!self.pending.is_empty()).then_some(0));
            let parent = index
                .and_then(|i| self.pending.remove(i))
                .map(|p| p.tool_call_id)
                // A missing replayed spawn degrades to a stable synthetic
                // owner instead of dropping the child transcript.
                .unwrap_or_else(|| agent_id.to_owned());
            self.bound.insert(agent_id.to_owned(), parent);
            // This lifecycle update's toolCallId is the child agent id, not a
            // real parent-chat tool call, so it must not render by itself.
            return Vec::new();
        }

        if let Some(completed) = meta(update, "cognition.ai/subagent_completed") {
            let Some(agent_id) = nonempty(completed, "agentId") else {
                return Vec::new();
            };
            if !self.settled.insert(agent_id.to_owned()) {
                return Vec::new();
            }
            let parent = self
                .bound
                .get(agent_id)
                .cloned()
                .unwrap_or_else(|| agent_id.to_owned());
            let mut events = Vec::new();
            if !self.saw_text.remove(agent_id)
                && let Some(summary) = nonempty(completed, "summary")
            {
                events.push(tag(
                    &parent,
                    AgentEvent::TextDelta {
                        text: summary.to_owned(),
                    },
                ));
            }
            let status = if completed.get("success").and_then(Value::as_bool) == Some(false) {
                DoneStatus::Errored
            } else {
                DoneStatus::Completed
            };
            events.push(tag(
                &parent,
                AgentEvent::Done {
                    status,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ));
            return events;
        }

        if is_spawn(update) {
            self.remember_spawn(update);
        }

        let child_id = meta(update, "cognition.ai/subagent_context")
            .and_then(|ctx| nonempty(ctx, "parentAgentId"));
        match child_id {
            Some(SIDEKICK) => self.sidekick_events(update),
            // "root" is the lead, and untagged sidekick frames carry the
            // `cognition.ai/sidekick` marker or an `sk::` toolCallId — both
            // route to the sidekick chip, never the parent transcript.
            None | Some(ROOT) if is_sidekick_frame(update) => self.sidekick_events(update),
            // Untagged lead traffic (including the duplicated root-tagged
            // usage_update — the repeat ContextUsage is harmless).
            None | Some(ROOT) => map_update(update),
            Some(child_id) => {
                if self.settled.contains(child_id) {
                    return Vec::new();
                }
                let parent = self
                    .bound
                    .get(child_id)
                    .cloned()
                    .unwrap_or_else(|| child_id.to_owned());
                let events = map_update(update);
                if events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::TextDelta { .. }))
                {
                    self.saw_text.insert(child_id.to_owned());
                }
                events
                    .into_iter()
                    .map(|event| tag(&parent, event))
                    .collect()
            }
        }
    }

    /// `_cognition.ai/sidekick/wait`: a handoff boundary. `waiting: true`
    /// opens one (settling any chip a previous handoff left open — one chip
    /// per handoff); `waiting: false` closes a BLOCKING handoff's chip. A
    /// non-blocking dispatch's wait:false lands ~0.3s in, before any
    /// sidekick frame, so it is a no-op — the chip its frames open later
    /// settles with the turn instead.
    ///
    /// The settle defers while the chip's own tools are still open: Devin
    /// can release the wait before the last `tool_call_update` lands, and a
    /// result arriving after the settle would tag a settled chip — the
    /// engine drops stragglers by design, leaving the tool unresolved.
    pub(crate) fn sidekick_wait(&mut self) -> Vec<AgentEvent> {
        if let Some(chip) = self.sidekick_chip.clone()
            && self.chip_tools_open(&chip)
        {
            self.sidekick_chip = None;
            self.deferred_sidekick.push_back(chip);
            return Vec::new();
        }
        self.settle_chip_now(DoneStatus::Completed)
    }

    /// The prompt response settled the turn: a sidekick chip still open (a
    /// non-blocking handoff whose frames arrived mid-turn) settles with the
    /// turn's status, before the turn's untagged Done. Deferred handoff
    /// settlements flush here too — the backstop that keeps a tool which
    /// never reports from holding its chip open forever.
    pub(crate) fn turn_settled(&mut self, status: DoneStatus) -> Vec<AgentEvent> {
        self.settle_sidekick(status)
    }

    /// Settle children when their owning ACP process/run goes away before a
    /// lifecycle completion arrives (interrupt, crash, or consumer close).
    pub(crate) fn finish_open(&mut self, status: DoneStatus) -> Vec<AgentEvent> {
        let open: Vec<(String, String)> = self
            .bound
            .iter()
            .filter(|(agent_id, _)| !self.settled.contains(*agent_id))
            .map(|(agent_id, parent)| (agent_id.clone(), parent.clone()))
            .collect();
        let mut events: Vec<AgentEvent> = open
            .into_iter()
            .map(|(agent_id, parent)| {
                self.settled.insert(agent_id);
                tag(
                    &parent,
                    AgentEvent::Done {
                        status,
                        result: None,
                        error: None,
                        session_id: None,
                    },
                )
            })
            .collect();
        events.extend(self.settle_sidekick(status));
        events
    }

    /// Route one sidekick frame: tag its mapped events with the handoff's
    /// chip, opening a synthesized spawn chip first when the frame carries
    /// real content. Usage-only and result-only frames between handoffs are
    /// dropped so they never mint empty chips.
    fn sidekick_events(&mut self, update: &Value) -> Vec<AgentEvent> {
        let events = map_update(update);
        if events.is_empty() {
            return Vec::new();
        }
        let tool_id = nonempty(update, "toolCallId").unwrap_or_default();
        let tool_frame = matches!(
            update.get("sessionUpdate").and_then(Value::as_str),
            Some("tool_call" | "tool_call_update")
        );
        // A late frame for a tool whose handoff already settled tags the OLD
        // chip: the engine drops stragglers for settled chips, and opening a
        // fresh chip here would resurrect a finished transcript.
        if let Some(chip) = self.sidekick_tools.get(tool_id)
            && self.sidekick_chip.as_deref() != Some(chip.as_str())
        {
            let chip = chip.clone();
            let (mut tagged, deferred) = self.tag_sidekick(&chip, events, tool_frame);
            tagged.extend(deferred);
            return tagged;
        }
        let chip = match self.sidekick_chip.clone() {
            Some(chip) => chip,
            None => {
                let opens = events.iter().any(|event| {
                    matches!(
                        event,
                        AgentEvent::ToolCall { .. }
                            | AgentEvent::TextDelta { .. }
                            | AgentEvent::ReasoningDelta { .. }
                    )
                });
                if !opens {
                    return Vec::new();
                }
                // Unique across runs: the engine keys subagent docs by chip id.
                let chip = format!("{SIDEKICK_CHIP_PREFIX}{}", uuid::Uuid::new_v4().simple());
                self.sidekick_chip = Some(chip.clone());
                let mut out = Vec::with_capacity(events.len() + 1);
                // The lead's own `sidekick` tool call never reaches ACP, so
                // the chip is synthesized — the "Agent: " name is what makes
                // it a spawn chip (`ToolCall::is_subagent_spawn`).
                out.push(AgentEvent::ToolCall {
                    id: chip.clone(),
                    call: ToolCall::Unknown {
                        name: "Agent: Sidekick".into(),
                        input: None,
                    },
                });
                if !tool_id.is_empty() {
                    self.sidekick_tools.insert(tool_id.to_owned(), chip.clone());
                }
                let (mut tagged, deferred) = self.tag_sidekick(&chip, events, tool_frame);
                out.append(&mut tagged);
                out.extend(deferred);
                return out;
            }
        };
        if !tool_id.is_empty() {
            self.sidekick_tools.insert(tool_id.to_owned(), chip.clone());
        }
        let (mut tagged, deferred) = self.tag_sidekick(&chip, events, tool_frame);
        tagged.extend(deferred);
        tagged
    }

    /// Tag `events` with `chip` and track the frame's tool open/close state
    /// (tool frames only). Returns the tagged events plus any deferred
    /// handoff settlements this frame made due — a `ToolResult` that closes
    /// a deferred chip's last open tool emits its Done + chip ToolResult
    /// right after it.
    fn tag_sidekick(
        &mut self,
        chip: &str,
        events: Vec<AgentEvent>,
        tool_frame: bool,
    ) -> (Vec<AgentEvent>, Vec<AgentEvent>) {
        let mut tagged = Vec::with_capacity(events.len());
        for event in events {
            if tool_frame {
                match &event {
                    AgentEvent::ToolCall { id, .. } => {
                        self.sidekick_open.insert(id.clone(), chip.to_owned());
                    }
                    AgentEvent::ToolResult { id, .. } => {
                        self.sidekick_open.remove(id);
                    }
                    _ => {}
                }
            }
            tagged.push(tag(chip, event));
        }
        (tagged, self.flush_deferred_sidekick())
    }

    /// Emit deferred handoff settlements whose chips have no open tools
    /// left, oldest first. A still-running chip keeps its place — later
    /// deferred chips wait behind it (turn settle is the backstop).
    fn flush_deferred_sidekick(&mut self) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Some(chip) = self.deferred_sidekick.front() {
            if self.chip_tools_open(chip) {
                break;
            }
            let chip = self.deferred_sidekick.pop_front().expect("front checked");
            events.extend(settle_chip_events(&chip, DoneStatus::Completed));
        }
        events
    }

    /// Whether `chip` still has sidekick tools that have not reported a
    /// terminal result.
    fn chip_tools_open(&self, chip: &str) -> bool {
        self.sidekick_open.values().any(|owner| owner == chip)
    }

    /// Close the open sidekick chip NOW, ignoring deferred settlements —
    /// the wait boundary's immediate path.
    fn settle_chip_now(&mut self, status: DoneStatus) -> Vec<AgentEvent> {
        let Some(chip) = self.sidekick_chip.take() else {
            return Vec::new();
        };
        settle_chip_events(&chip, status)
    }

    /// Close the open sidekick chip plus every deferred one — the
    /// unconditional backstop used at turn/finish settle. Deferred chips
    /// emit first (they are older) with Completed: their wait boundary
    /// already ended the handoff, only the emission was held.
    fn settle_sidekick(&mut self, status: DoneStatus) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Some(chip) = self.deferred_sidekick.pop_front() {
            self.sidekick_open.retain(|_, owner| owner != &chip);
            events.extend(settle_chip_events(&chip, DoneStatus::Completed));
        }
        if let Some(chip) = self.sidekick_chip.take() {
            self.sidekick_open.retain(|_, owner| owner != &chip);
            events.extend(settle_chip_events(&chip, status));
        }
        events
    }

    fn remember_spawn(&mut self, update: &Value) {
        let id = nonempty(update, "toolCallId").unwrap_or_default();
        if id.is_empty() {
            return;
        }
        if matches!(
            update.get("status").and_then(Value::as_str),
            Some("failed" | "cancelled")
        ) {
            self.pending.retain(|p| p.tool_call_id != id);
            return;
        }
        let title = update
            .get("rawInput")
            .and_then(|raw| nonempty(raw, "title"))
            .unwrap_or_default();
        match self.pending.iter_mut().find(|p| p.tool_call_id == id) {
            Some(p) if p.title.is_empty() => p.title = title.to_owned(),
            Some(_) => {}
            None => self.pending.push_back(PendingSpawn {
                tool_call_id: id.to_owned(),
                title: title.to_owned(),
            }),
        }
    }
}

/// A frame belonging to the Fusion sidekick even without a
/// `subagent_context` tag: `cognition.ai/sidekick: true` in `_meta`, or an
/// `sk::`-prefixed toolCallId.
fn is_sidekick_frame(update: &Value) -> bool {
    meta(update, "cognition.ai/sidekick").and_then(Value::as_bool) == Some(true)
        || nonempty(update, "toolCallId").is_some_and(|id| id.starts_with("sk::"))
}

fn is_spawn(update: &Value) -> bool {
    update
        .get("_meta")
        .and_then(|m| m.get("cognition.ai/inferenceToolName"))
        .and_then(Value::as_str)
        == Some("run_subagent")
}

fn meta<'a>(update: &'a Value, key: &str) -> Option<&'a Value> {
    update.get("_meta")?.get(key)
}

fn nonempty<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str().filter(|s| !s.is_empty())
}

fn tag(parent: &str, event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: parent.to_owned(),
        event: Box::new(event),
    }
}

/// A settled sidekick chip's closing pair: a tagged Done for its doc plus
/// an untagged ToolResult resolving the synthesized spawn chip (no
/// lead-side tool call ever settles it).
fn settle_chip_events(chip: &str, status: DoneStatus) -> Vec<AgentEvent> {
    vec![
        tag(
            chip,
            AgentEvent::Done {
                status,
                result: None,
                error: None,
                session_id: None,
            },
        ),
        AgentEvent::ToolResult {
            id: chip.to_owned(),
            is_error: status == DoneStatus::Errored,
            output: None,
            diff: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use zuse_protocol::{AgentEvent, DoneStatus, ToolCall};

    use super::DevinTracker;

    fn sidekick_usage() -> serde_json::Value {
        json!({
            "sessionUpdate": "usage_update", "used": 10631, "size": 262000,
            "_meta": { "cognition.ai/inputTokens": 10552, "cognition.ai/outputTokens": 79,
                "cognition.ai/subagent_context": { "parentAgentId": "sidekick" } }
        })
    }

    #[test]
    fn force_settle_clears_open_tools_but_preserves_late_result_identity() {
        let mut tracker = DevinTracker::default();
        tracker.map(&sidekick_tool_call("sk::old"));
        let chip = tracker.sidekick_chip.clone().unwrap();
        assert!(!tracker.sidekick_open.is_empty());
        assert!(!tracker.turn_settled(DoneStatus::Completed).is_empty());
        assert!(tracker.sidekick_open.is_empty());
        let late = tracker.map(&sidekick_tool_done("sk::old"));
        assert!(late.iter().any(|event| matches!(event,
            AgentEvent::Subagent { parent_tool_use_id, event }
                if parent_tool_use_id == &chip && matches!(event.as_ref(), AgentEvent::ToolResult { .. }))));
        assert!(tracker.sidekick_chip.is_none());
        assert!(tracker.sidekick_open.is_empty());
    }

    fn sidekick_tool_call(id: &str) -> serde_json::Value {
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": id,
            "title": "Ran ls, echo",
            "kind": "execute",
            "rawInput": { "command": "ls" },
            "_meta": { "cognition.ai/inferenceToolName": "exec", "cognition.ai/sidekick": true }
        })
    }

    fn sidekick_tool_done(id: &str) -> serde_json::Value {
        json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "status": "completed",
            "_meta": { "cognition.ai/inferenceToolName": "exec", "cognition.ai/sidekick": true }
        })
    }

    /// The chip id out of the spawn ToolCall that opens a sidekick handoff.
    fn opened_chip(events: &[AgentEvent]) -> String {
        match events.first() {
            Some(AgentEvent::ToolCall { id, call }) if call.is_subagent_spawn() => id.clone(),
            other => panic!("expected a spawn chip first, got {other:?}"),
        }
    }

    #[test]
    fn root_tagged_usage_duplicate_is_plain_lead_traffic() {
        let mut tracker = DevinTracker::default();
        let meta = json!({
            "cognition.ai/inputTokens": 13172, "cognition.ai/outputTokens": 56,
            "cognition.ai/cachedWriteTokens": 13169
        });
        let untagged = tracker.map(&json!({
            "sessionUpdate": "usage_update", "used": 13228, "size": 1050000, "_meta": meta
        }));
        assert_eq!(
            untagged,
            vec![AgentEvent::ContextUsage {
                tokens: Some(13228),
                window: Some(1050000)
            }]
        );
        // Devin 3000.14 emits every lead usage_update twice; the "root" copy
        // must not become a subagent (it used to open a doc that never
        // settled — "Waiting for 1 subagent" after every turn).
        let mut root_meta = meta;
        root_meta["cognition.ai/subagent_context"] = json!({ "parentAgentId": "root" });
        let tagged = tracker.map(&json!({
            "sessionUpdate": "usage_update", "used": 13228, "size": 1050000, "_meta": root_meta
        }));
        assert_eq!(tagged, untagged);
        assert!(
            !tagged
                .iter()
                .any(|e| matches!(e, AgentEvent::Subagent { .. }))
        );
    }

    #[test]
    fn blocking_sidekick_handoff_tags_frames_and_settles_on_wait_false() {
        let mut tracker = DevinTracker::default();
        let mut all: Vec<AgentEvent> = Vec::new();
        // wait:true opens the handoff; nothing is open yet.
        all.extend(tracker.sidekick_wait());
        // Sidekick usage before any content frame is dropped (no empty chip).
        assert!(tracker.map(&sidekick_usage()).is_empty());
        // The sk:: tool_call opens the chip.
        let events = tracker.map(&sidekick_tool_call("sk::exec:0#abc"));
        let chip = opened_chip(&events);
        all.extend(events);
        // Its completion update tags the same chip.
        let events = tracker.map(&sidekick_tool_done("sk::exec:0#abc"));
        assert!(
            events.iter().all(|e| matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, .. } if *parent_tool_use_id == chip
            )),
            "{events:?}"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::Subagent { event, .. }
                if matches!(event.as_ref(), AgentEvent::ToolResult { id, is_error: false, .. } if id == "sk::exec:0#abc")
        )));
        all.extend(events);
        // wait:false settles the blocking handoff.
        let settled = tracker.sidekick_wait();
        all.extend(settled.clone());
        assert!(matches!(
            &settled[0],
            AgentEvent::Subagent { parent_tool_use_id, event }
                if *parent_tool_use_id == chip
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })
        ));
        assert!(matches!(
            &settled[1],
            AgentEvent::ToolResult { id, is_error: false, .. } if *id == chip
        ));
        // Exactly one untagged ToolCall (the chip) and one untagged
        // ToolResult (its resolution); every sk:: frame stayed tagged.
        let untagged_calls: Vec<_> = all
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCall { id, .. } if !id.starts_with("devin-sidekick-")))
            .collect();
        assert!(untagged_calls.is_empty(), "{untagged_calls:?}");
        assert!(
            all.iter().all(|e| match e {
                AgentEvent::ToolCall { id, call } => {
                    id.starts_with("devin-sidekick-") && call.is_subagent_spawn()
                }
                AgentEvent::Subagent { .. } | AgentEvent::ToolResult { .. } => true,
                _ => false,
            }),
            "{all:?}"
        );
        // Nothing left open: a second wait is a no-op, and the turn settle too.
        assert!(tracker.sidekick_wait().is_empty());
        assert!(tracker.turn_settled(DoneStatus::Completed).is_empty());
    }

    #[test]
    fn nonblocking_sidekick_chip_settles_with_the_turn() {
        let mut tracker = DevinTracker::default();
        // waiting:true then waiting:false ~0.3s later, BEFORE any frame.
        assert!(tracker.sidekick_wait().is_empty());
        assert!(tracker.sidekick_wait().is_empty());
        // The sidekick works afterwards: its frames open the chip mid-turn.
        let events = tracker.map(&sidekick_tool_call("sk::exec:0#nb"));
        let chip = opened_chip(&events);
        // Devin holds the prompt response until the sidekick is done, so the
        // chip settles when the turn does.
        let settled = tracker.turn_settled(DoneStatus::Completed);
        assert!(matches!(
            &settled[0],
            AgentEvent::Subagent { parent_tool_use_id, event }
                if *parent_tool_use_id == chip
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })
        ));
        assert!(matches!(
            &settled[1],
            AgentEvent::ToolResult { id, .. } if *id == chip
        ));
        // A usage-only sidekick frame with no open chip yields nothing.
        let mut fresh = DevinTracker::default();
        assert!(fresh.map(&sidekick_usage()).is_empty());
    }

    #[test]
    fn late_sidekick_update_tags_the_settled_chip_not_a_new_one() {
        let mut tracker = DevinTracker::default();
        // Handoff 1: wait, one tool, wait:false — the settle defers while
        // the tool is still open.
        assert!(tracker.sidekick_wait().is_empty());
        let events = tracker.map(&sidekick_tool_call("sk::exec:0#first"));
        let chip1 = opened_chip(&events);
        assert!(tracker.sidekick_wait().is_empty());
        // The tool's result flushes the deferred settle: a tagged
        // ToolResult for the tool itself, then the chip's Done + ToolResult.
        let settled = tracker.map(&sidekick_tool_done("sk::exec:0#first"));
        assert!(
            settled.iter().any(|e| matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if *parent_tool_use_id == chip1
                        && matches!(event.as_ref(), AgentEvent::ToolResult { id, .. } if id == "sk::exec:0#first")
            )),
            "{settled:?}"
        );
        assert!(matches!(
            &settled[1],
            AgentEvent::Subagent { parent_tool_use_id, event }
                if *parent_tool_use_id == chip1
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })
        ));
        // Handoff 2 opens a DISTINCT chip.
        assert!(tracker.sidekick_wait().is_empty());
        assert!(tracker.sidekick_wait().is_empty());
        let events = tracker.map(&sidekick_tool_call("sk::exec:0#second"));
        let chip2 = opened_chip(&events);
        assert_ne!(chip1, chip2);
        // A late update for handoff 1's tool tags chip1 — it must not open a
        // third chip (the engine would resurrect a settled transcript).
        let late = tracker.map(&sidekick_tool_done("sk::exec:0#first"));
        assert!(
            late.iter().any(|e| matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, event }
                    if *parent_tool_use_id == chip1
                        && matches!(event.as_ref(), AgentEvent::ToolResult { id, .. } if id == "sk::exec:0#first")
            )),
            "{late:?}"
        );
        assert!(
            late.iter().all(|e| matches!(
                e,
                AgentEvent::Subagent { parent_tool_use_id, .. } if *parent_tool_use_id == chip1
            )),
            "{late:?}"
        );
        assert!(
            !late.iter().any(|e| matches!(
                e,
                AgentEvent::ToolCall { id, .. } if id.starts_with("devin-sidekick-")
            )),
            "{late:?}"
        );
    }

    /// A `waiting:false` that lands while the handoff's tool still runs
    /// must not settle the chip: the tool's own result would then tag a
    /// settled chip and the engine would drop it as a straggler. The
    /// settlement waits for the last open tool instead.
    #[test]
    fn handoff_settle_waits_for_the_chips_last_open_tool() {
        let mut tracker = DevinTracker::default();
        assert!(tracker.sidekick_wait().is_empty());
        let events = tracker.map(&sidekick_tool_call("sk::exec:0#slow"));
        let chip = opened_chip(&events);
        // wait:false mid-tool: nothing settles yet.
        assert!(tracker.sidekick_wait().is_empty());
        // The tool's result lands first (tagged under the chip), then the
        // deferred settlement: tagged Done + untagged chip ToolResult.
        let events = tracker.map(&sidekick_tool_done("sk::exec:0#slow"));
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(matches!(
            &events[0],
            AgentEvent::Subagent { parent_tool_use_id, event }
                if *parent_tool_use_id == chip
                    && matches!(event.as_ref(), AgentEvent::ToolResult { id, .. } if id == "sk::exec:0#slow")
        ));
        assert!(matches!(
            &events[1],
            AgentEvent::Subagent { parent_tool_use_id, event }
                if *parent_tool_use_id == chip
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })
        ));
        assert!(matches!(
            &events[2],
            AgentEvent::ToolResult { id, is_error: false, .. } if *id == chip
        ));
        // Nothing deferred remains: the turn settle is a no-op.
        assert!(tracker.turn_settled(DoneStatus::Completed).is_empty());
    }

    /// The turn settle is the backstop: a deferred chip whose tool never
    /// reports still settles with the turn, before its untagged Done.
    #[test]
    fn deferred_sidekick_chip_settles_with_the_turn() {
        let mut tracker = DevinTracker::default();
        assert!(tracker.sidekick_wait().is_empty());
        let events = tracker.map(&sidekick_tool_call("sk::exec:0#stuck"));
        let chip = opened_chip(&events);
        assert!(tracker.sidekick_wait().is_empty());
        let settled = tracker.turn_settled(DoneStatus::Completed);
        assert!(matches!(
            &settled[0],
            AgentEvent::Subagent { parent_tool_use_id, event }
                if *parent_tool_use_id == chip
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })
        ));
        assert!(matches!(
            &settled[1],
            AgentEvent::ToolResult { id, .. } if *id == chip
        ));
        assert!(tracker.finish_open(DoneStatus::Interrupted).is_empty());
    }

    #[test]
    fn finish_open_settles_an_open_sidekick_chip() {
        let mut tracker = DevinTracker::default();
        let events = tracker.map(&sidekick_tool_call("sk::exec:0#open"));
        let chip = opened_chip(&events);
        let events = tracker.finish_open(DoneStatus::Errored);
        assert!(matches!(
            &events[0],
            AgentEvent::Subagent { parent_tool_use_id, event }
                if *parent_tool_use_id == chip
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Errored, .. })
        ));
        assert!(matches!(
            &events[1],
            AgentEvent::ToolResult { id, is_error: true, .. } if *id == chip
        ));
        assert!(tracker.finish_open(DoneStatus::Interrupted).is_empty());
    }

    #[test]
    fn correlates_spawn_child_traffic_and_completion() {
        let mut tracker = DevinTracker::default();
        let events = tracker.map(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "spawn-1",
            "kind": "other",
            "status": "in_progress",
            "rawInput": { "title": "Inspect auth", "task": "Review auth.rs" },
            "_meta": { "cognition.ai/inferenceToolName": "run_subagent" }
        }));
        assert!(matches!(
            &events[0],
            AgentEvent::ToolCall { id, call: ToolCall::Unknown { name, .. } }
                if id == "spawn-1" && name == "Agent: Inspect auth"
        ));

        assert!(
            tracker
                .map(&json!({
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": "agent-7",
                    "_meta": { "cognition.ai/subagent_started": {
                        "agentId": "agent-7", "title": "Inspect auth", "profile": "Explore"
                    }}
                }))
                .is_empty()
        );

        let child = tracker.map(&json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "Found the issue" },
            "_meta": { "cognition.ai/subagent_context": { "parentAgentId": "agent-7" }}
        }));
        assert_eq!(
            child,
            vec![AgentEvent::Subagent {
                parent_tool_use_id: "spawn-1".into(),
                event: Box::new(AgentEvent::TextDelta {
                    text: "Found the issue".into()
                }),
            }]
        );

        let completed = tracker.map(&json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "agent-7",
            "_meta": { "cognition.ai/subagent_completed": {
                "agentId": "agent-7", "success": true, "summary": "Found the issue"
            }}
        }));
        assert_eq!(completed.len(), 1, "streamed summary must not duplicate");
        assert!(matches!(
            &completed[0],
            AgentEvent::Subagent { parent_tool_use_id, event }
                if parent_tool_use_id == "spawn-1"
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Completed, .. })
        ));
    }

    #[test]
    fn completion_summary_is_a_fallback_when_child_was_quiet() {
        let mut tracker = DevinTracker::default();
        tracker.map(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "spawn-2",
            "rawInput": { "title": "Quiet" },
            "_meta": { "cognition.ai/inferenceToolName": "run_subagent" }
        }));
        tracker.map(&json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "agent-8",
            "_meta": { "cognition.ai/subagent_started": {
                "agentId": "agent-8", "title": "Quiet"
            }}
        }));
        let events = tracker.map(&json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "agent-8",
            "_meta": { "cognition.ai/subagent_completed": {
                "agentId": "agent-8", "success": false, "summary": "Could not inspect"
            }}
        }));
        assert!(matches!(
            &events[0],
            AgentEvent::Subagent { parent_tool_use_id, event }
                if parent_tool_use_id == "spawn-2"
                    && matches!(event.as_ref(), AgentEvent::TextDelta { text } if text == "Could not inspect")
        ));
        assert!(matches!(
            &events[1],
            AgentEvent::Subagent { event, .. }
                if matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Errored, .. })
        ));
    }

    #[test]
    fn unfinished_children_settle_when_the_parent_run_ends() {
        let mut tracker = DevinTracker::default();
        tracker.map(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "spawn-3",
            "rawInput": { "title": "Background" },
            "_meta": { "cognition.ai/inferenceToolName": "run_subagent" }
        }));
        tracker.map(&json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "agent-9",
            "_meta": { "cognition.ai/subagent_started": {
                "agentId": "agent-9", "title": "Background"
            }}
        }));
        let events = tracker.finish_open(DoneStatus::Interrupted);
        assert!(matches!(
            &events[0],
            AgentEvent::Subagent { parent_tool_use_id, event }
                if parent_tool_use_id == "spawn-3"
                    && matches!(event.as_ref(), AgentEvent::Done { status: DoneStatus::Interrupted, .. })
        ));
        assert!(tracker.finish_open(DoneStatus::Interrupted).is_empty());
    }
}
