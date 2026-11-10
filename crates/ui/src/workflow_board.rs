//! A native workflow run in the transcript: one summary line, then a line per
//! phase in run order, with the live phase's agents beneath it. Clicking an
//! agent opens it in the right pane (see [`crate::workflow_agent_pane`]).
//!
//! The pure half ([`workflow_sections`], [`phase_state`]) groups a run's
//! snapshot ([`WorkflowProgress`], carried on the run's tool call) into
//! sections; [`render_board`] draws them. Interaction state (folded and
//! expanded phases, the script disclosure) lives with the transcript, keyed
//! by row, and comes back through [`BoardAction`]s.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, Hsla, SharedString, Window, div, prelude::*, px};
use zuse_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zuse_protocol::{WorkflowAgentProgress, WorkflowAgentState, WorkflowProgress};

use crate::theme::Theme;

/// Agents a phase shows before its "Show N more" control.
pub(crate) const VISIBLE_AGENTS: usize = 8;

/// The run's lifecycle, from its chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RunStatus {
    Working,
    Done,
    Failed,
    /// Interrupted. Chips record an interrupted subagent as done today, so
    /// only the board's own logic (and tests) reach this; kept so the board
    /// reads it correctly once chips carry it.
    #[cfg_attr(not(test), allow(dead_code))]
    Stopped,
}

/// One agent's lifecycle as the board shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AgentStatus {
    Queued,
    Working,
    Done,
    Failed,
    Stopped,
}

/// A phase the script declared up front (its `meta`), when known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclaredPhase {
    pub title: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentView {
    pub index: u32,
    pub label: String,
    pub status: AgentStatus,
    pub prompt: Option<String>,
    pub result: Option<String>,
    pub model: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Section {
    pub key: String,
    /// `None` for agents started outside any phase.
    pub title: Option<String>,
    pub detail: Option<String>,
    pub agents: Vec<AgentView>,
    pub done: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PhaseState {
    Pending,
    Active,
    Done,
    Failed,
    Stopped,
}

/// An agent the snapshot still shows as active cannot outlive its run: once
/// the run settles, it inherits the run's outcome.
fn agent_status(agent: &WorkflowAgentProgress, run: RunStatus) -> AgentStatus {
    match agent.state {
        WorkflowAgentState::Done => AgentStatus::Done,
        WorkflowAgentState::Failed => AgentStatus::Failed,
        WorkflowAgentState::Queued if run == RunStatus::Working => AgentStatus::Queued,
        WorkflowAgentState::Running if run == RunStatus::Working => AgentStatus::Working,
        _ if run == RunStatus::Done => AgentStatus::Done,
        _ => AgentStatus::Stopped,
    }
}

/// "map:catalog" under "Map" reads as "catalog"; the phase already says Map.
fn label_within_phase(label: &str, phase: Option<&str>) -> String {
    let (Some(phase), Some(separator)) = (phase, label.find(':')) else {
        return label.to_owned();
    };
    if separator == 0 {
        return label.to_owned();
    }
    let rest = label[separator + 1..].trim();
    if !rest.is_empty() && label[..separator].trim().to_lowercase() == phase.to_lowercase() {
        rest.to_owned()
    } else {
        label.to_owned()
    }
}

fn agent_view(agent: &WorkflowAgentProgress, run: RunStatus, phase: Option<&str>) -> AgentView {
    let status = agent_status(agent, run);
    let result = agent.error.clone().or_else(|| agent.result_preview.clone());
    let settled = !matches!(status, AgentStatus::Working | AgentStatus::Queued);
    AgentView {
        index: agent.index,
        label: label_within_phase(&agent.label, phase),
        status,
        prompt: agent.prompt_preview.clone(),
        result,
        model: agent.model.clone(),
        started_at: agent.started_at,
        completed_at: match (settled, agent.started_at, agent.duration_ms) {
            (true, Some(start), Some(duration)) => Some(start.saturating_add(duration as i64)),
            _ => None,
        },
    }
}

/// Group a run's agents under its phases: declared phases keep their script
/// order and detail, phases first seen at runtime follow in start order, and
/// agents outside any phase lead.
pub(crate) fn workflow_sections(
    declared: &[DeclaredPhase],
    progress: Option<&WorkflowProgress>,
    run: RunStatus,
) -> Vec<Section> {
    let mut sections: Vec<Section> = declared
        .iter()
        .enumerate()
        .map(|(ix, phase)| Section {
            key: format!("declared:{ix}"),
            title: Some(phase.title.clone()),
            detail: phase.detail.clone(),
            agents: Vec::new(),
            done: 0,
        })
        .collect();
    let mut by_index: HashMap<u32, usize> = HashMap::new();
    let mut matched: HashSet<usize> = HashSet::new();
    let mut phase = |sections: &mut Vec<Section>,
                     by_index: &mut HashMap<u32, usize>,
                     index: u32,
                     title: &str| {
        if let Some(&existing) = by_index.get(&index) {
            return existing;
        }
        let ix = match (0..sections.len())
            .find(|&ix| sections[ix].title.as_deref() == Some(title) && !matched.contains(&ix))
        {
            Some(ix) => ix,
            None => {
                sections.push(Section {
                    key: format!("runtime:{index}"),
                    title: Some(title.to_owned()),
                    detail: None,
                    agents: Vec::new(),
                    done: 0,
                });
                sections.len() - 1
            }
        };
        matched.insert(ix);
        by_index.insert(index, ix);
        ix
    };
    let empty = WorkflowProgress::default();
    let progress = progress.unwrap_or(&empty);
    for entry in &progress.phases {
        phase(&mut sections, &mut by_index, entry.index, &entry.title);
    }
    let mut unphased: Vec<AgentView> = Vec::new();
    for agent in &progress.agents {
        let mut target = agent.phase_index.and_then(|ix| by_index.get(&ix).copied());
        if target.is_none()
            && let (Some(index), Some(title)) = (agent.phase_index, agent.phase_title.as_deref())
        {
            target = Some(phase(&mut sections, &mut by_index, index, title));
        }
        if target.is_none()
            && agent.phase_index.is_none()
            && let Some(title) = agent.phase_title.as_deref()
        {
            // A title-only snapshot cannot identify which repeated
            // occurrence it means.
            let candidates: Vec<usize> = (0..sections.len())
                .filter(|&ix| sections[ix].title.as_deref() == Some(title))
                .collect();
            match candidates.as_slice() {
                [only] => target = Some(*only),
                [] => {
                    sections.push(Section {
                        key: format!("title:{title}"),
                        title: Some(title.to_owned()),
                        detail: None,
                        agents: Vec::new(),
                        done: 0,
                    });
                    target = Some(sections.len() - 1);
                }
                _ => {}
            }
        }
        match target {
            Some(ix) => {
                let view = agent_view(agent, run, sections[ix].title.as_deref());
                sections[ix].agents.push(view);
            }
            None => unphased.push(agent_view(agent, run, None)),
        }
    }
    if !unphased.is_empty() {
        sections.insert(
            0,
            Section {
                key: "unphased".into(),
                title: None,
                detail: None,
                agents: unphased,
                done: 0,
            },
        );
    }
    for section in &mut sections {
        section.done = section
            .agents
            .iter()
            .filter(|a| a.status == AgentStatus::Done)
            .count();
    }
    sections
}

/// A phase's outcome from its agents, failures first, then active work.
pub(crate) fn phase_state(section: &Section, run: RunStatus) -> PhaseState {
    let agents = &section.agents;
    if agents.iter().any(|a| a.status == AgentStatus::Failed) {
        PhaseState::Failed
    } else if agents
        .iter()
        .any(|a| matches!(a.status, AgentStatus::Working | AgentStatus::Queued))
    {
        PhaseState::Active
    } else if !agents.is_empty() && agents.iter().all(|a| a.status == AgentStatus::Done) {
        PhaseState::Done
    } else if !agents.is_empty() || run != RunStatus::Working {
        PhaseState::Stopped
    } else {
        PhaseState::Pending
    }
}

/// Elapsed time the way delegation rows show it: `42s`, `3m 05s`, `1h 02m`.
pub(crate) fn format_elapsed_ms(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    let minutes = seconds / 60;
    if minutes == 0 {
        return format!("{seconds}s");
    }
    let hours = minutes / 60;
    if hours == 0 {
        return format!("{minutes}m {:02}s", seconds % 60);
    }
    format!("{hours}h {:02}m", minutes % 60)
}

/// A span's elapsed text: live against `now` while it runs, fixed once it
/// has an end; nothing when it never started.
fn elapsed(start: Option<i64>, end: Option<i64>, live: bool, now_ms: i64) -> Option<String> {
    let start = start?;
    let end = end.or(live.then_some(now_ms))?;
    Some(format_elapsed_ms(end - start))
}

/// The run's own span: its first agent's start to its last agent's end (the
/// chip carries no run timestamps of its own).
fn run_span(sections: &[Section]) -> (Option<i64>, Option<i64>) {
    let agents = || sections.iter().flat_map(|s| &s.agents);
    let start = agents().filter_map(|a| a.started_at).min();
    let end = agents()
        .filter_map(|a| a.completed_at.or(a.started_at))
        .max();
    (start, end)
}

/// Per-row interaction state, owned by the transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BoardState {
    /// Section keys whose every agent shows.
    pub expanded: HashSet<String>,
    /// Section keys the user flipped from their default open state.
    pub flipped: HashSet<String>,
    /// The user flipped the whole run from its default open state.
    pub board_flipped: bool,
    pub script_open: bool,
}

impl BoardState {
    /// Apply an action; `true` when the board's height may have changed.
    pub(crate) fn apply(&mut self, action: &BoardAction) -> bool {
        match action {
            // Opening an agent happens in the right pane; the board is unchanged.
            BoardAction::Open(_) => return false,
            BoardAction::ShowAll(key) => {
                self.expanded.insert(key.clone());
            }
            BoardAction::ToggleSection(key) => {
                if !self.flipped.remove(key) {
                    self.flipped.insert(key.clone());
                }
            }
            BoardAction::ToggleBoard => self.board_flipped = !self.board_flipped,
            BoardAction::ToggleScript => self.script_open = !self.script_open,
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoardAction {
    /// Open this agent in the right pane.
    Open(u32),
    /// Show every agent of this section.
    ShowAll(String),
    /// Fold or unfold this section's agents.
    ToggleSection(String),
    /// Fold or unfold the whole run.
    ToggleBoard,
    ToggleScript,
}

pub(crate) type BoardDispatch = Rc<dyn Fn(BoardAction, &mut Window, &mut App)>;

/// Everything one paint of a board needs.
pub(crate) struct BoardProps<'a> {
    pub row_id: SharedString,
    pub name: SharedString,
    pub run: RunStatus,
    pub progress: Option<&'a WorkflowProgress>,
    pub script_path: Option<SharedString>,
    pub state: &'a BoardState,
    pub now_ms: i64,
    /// The summary's shimmer while the run works: text → element.
    pub status_shimmer: Option<Box<dyn FnOnce(SharedString) -> AnyElement + 'a>>,
    /// Elapsed since an arbitrary epoch, for the turning Claude marks;
    /// `None` holds them still (reduced motion).
    pub spin: Option<Duration>,
}

/// Header, phase and agent lines share one height.
const LINE_HEIGHT: f32 = 26.0;
/// Status marks sit in a fixed slot so every line's text starts together.
const MARK_SLOT: f32 = 12.0;
/// Claude marks are a touch larger than status marks; they lead their line.
const CLAUDE_SLOT: f32 = 14.0;
/// A working Claude mark turns once per this period.
const MARK_TURN: Duration = Duration::from_secs(3);

/// The Claude mark for one agent (workflows are a Claude Code feature, so
/// every agent runs on Claude): it turns in Claude's orange while working,
/// rests muted once done, and takes the danger tone on failure.
fn claude_mark(status: AgentStatus, spin: Option<Duration>, theme: &Theme) -> AnyElement {
    let color = match status {
        AgentStatus::Working => crate::icons::claude_brand(),
        AgentStatus::Done => theme.text_muted,
        AgentStatus::Failed => theme.danger,
        AgentStatus::Queued | AgentStatus::Stopped => theme.text_muted.opacity(0.35),
    };
    let mark = crate::icons::icon(crate::icons::CLAUDE_MARK)
        .size(px(CLAUDE_SLOT))
        .text_color(color);
    let mark = match spin.filter(|_| status == AgentStatus::Working) {
        Some(spin) => {
            let turn = (spin.as_secs_f32() % MARK_TURN.as_secs_f32()) / MARK_TURN.as_secs_f32();
            mark.with_transformation(gpui::Transformation::rotate(gpui::radians(
                turn * std::f32::consts::TAU,
            )))
            .into_any_element()
        }
        None => mark.into_any_element(),
    };
    div()
        .size(px(CLAUDE_SLOT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(mark)
        .into_any_element()
}

/// One clock for every board's turning marks.
pub(crate) fn spin_epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// An outlined ring, dashed while waiting, with a bar through it once stopped.
fn ring(size: f32, dashed: bool, bar: bool, color: Hsla) -> AnyElement {
    div()
        .size(px(size))
        .rounded_full()
        .border_1()
        .when(dashed, |ring| ring.border_dashed())
        .border_color(color)
        .flex()
        .items_center()
        .justify_center()
        .when(bar, |ring| {
            ring.child(div().w(px(size / 2.0)).h(px(1.0)).bg(color))
        })
        .into_any_element()
}

/// A small status mark, the same vocabulary delegation rows use: a live dot
/// in a soft halo while working, a check or cross once settled, a dashed
/// ring while waiting.
pub(crate) fn status_mark(status: AgentStatus, size: f32, theme: &Theme) -> AnyElement {
    let slot = div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    match status {
        AgentStatus::Working => slot.child(
            div()
                .size(px(size * 0.75))
                .rounded_full()
                .bg(theme.busy.opacity(0.25))
                .flex()
                .items_center()
                .justify_center()
                .child(div().size(px(size * 0.42)).rounded_full().bg(theme.busy)),
        ),
        AgentStatus::Queued => {
            slot.child(ring(size * 0.7, true, false, theme.text_muted.opacity(0.5)))
        }
        AgentStatus::Stopped => {
            slot.child(ring(size * 0.7, false, true, theme.text_muted.opacity(0.6)))
        }
        AgentStatus::Done => slot.child(
            crate::icons::icon(crate::icons::CHECK)
                .size(px(size))
                .text_color(theme.success),
        ),
        AgentStatus::Failed => slot.child(
            crate::icons::icon(crate::icons::CLOSE)
                .size(px(size))
                .text_color(theme.danger),
        ),
    }
    .into_any_element()
}

/// A phase's status mark, centred in the Claude marks' slot so phase and
/// agent text start on the same edge as their rails.
fn phase_mark(state: PhaseState, theme: &Theme) -> AnyElement {
    let mark = status_mark(
        match state {
            PhaseState::Pending => AgentStatus::Queued,
            PhaseState::Active => AgentStatus::Working,
            PhaseState::Done => AgentStatus::Done,
            PhaseState::Failed => AgentStatus::Failed,
            PhaseState::Stopped => AgentStatus::Stopped,
        },
        MARK_SLOT,
        theme,
    );
    div()
        .size(px(CLAUDE_SLOT))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(mark)
        .into_any_element()
}

pub(crate) fn agent_timing(agent: &AgentView, now_ms: i64) -> Option<String> {
    if agent.status == AgentStatus::Queued {
        return None;
    }
    elapsed(
        agent.started_at,
        agent.completed_at,
        agent.status == AgentStatus::Working,
        now_ms,
    )
}

/// Collapse whitespace runs so multi-line previews read as one line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The quiet line after an agent's label: what it returned once settled,
/// what it was asked while it runs.
pub(crate) fn agent_tail(agent: &AgentView) -> Option<String> {
    let text = match agent.status {
        AgentStatus::Queued => return Some("Queued".into()),
        AgentStatus::Working => agent.prompt.as_deref(),
        AgentStatus::Done | AgentStatus::Failed => agent.result.as_deref(),
        AgentStatus::Stopped => agent.result.as_deref().or(Some("Stopped")),
    }?;
    let line = one_line(text);
    (!line.is_empty()).then_some(line)
}

/// One agent: status, name, what it is doing, time, and an open arrow on
/// hover. The whole line opens the agent in the right pane.
fn agent_line(
    props: &BoardProps<'_>,
    agent: &AgentView,
    dispatch: &BoardDispatch,
    theme: &Theme,
) -> AnyElement {
    let index = agent.index;
    let dispatch = dispatch.clone();
    let failed = agent.status == AgentStatus::Failed;
    let hover = theme.ink(0.05);
    div()
        .id(SharedString::from(format!(
            "{}-agent-{index}",
            props.row_id
        )))
        .group("workflow-agent")
        .h(px(LINE_HEIGHT))
        .ml(px(-6.0))
        .px(px(6.0))
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
        .on_click(move |_, window, cx| dispatch(BoardAction::Open(index), window, cx))
        .child(claude_mark(agent.status, props.spin, theme))
        .child(
            div()
                .flex_none()
                .max_w(gpui::relative(0.45))
                .truncate()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(if failed {
                    theme.danger
                } else if agent.status == AgentStatus::Queued {
                    theme.text_muted
                } else {
                    theme.text.opacity(0.9)
                })
                .child(SharedString::from(agent.label.clone())),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_color(if failed {
                    theme.danger.opacity(0.75)
                } else {
                    theme.text_muted
                })
                .children(agent_tail(agent).map(SharedString::from)),
        )
        .when_some(agent_timing(agent, props.now_ms), |row, timing| {
            row.child(
                div()
                    .flex_none()
                    .text_color(theme.text_faint)
                    .child(SharedString::from(timing)),
            )
        })
        .child(
            crate::icons::icon(crate::icons::ARROW_UP_RIGHT)
                .flex_none()
                .size(px(12.0))
                .text_color(theme.text_muted)
                .opacity(0.0)
                .group_hover("workflow-agent", |style| style.opacity(1.0)),
        )
        .into_any_element()
}

fn chevron(open: bool, theme: &Theme) -> AnyElement {
    crate::icons::icon(if open {
        crate::icons::ALT_ARROW_DOWN
    } else {
        crate::icons::ALT_ARROW_RIGHT
    })
    .flex_none()
    .size(px(12.0))
    .text_color(theme.text_muted)
    .opacity(0.6)
    .into_any_element()
}

/// "Show N more" under a phase's visible agents.
fn more_control(
    props: &BoardProps<'_>,
    section: &Section,
    hidden: usize,
    dispatch: &BoardDispatch,
    theme: &Theme,
) -> AnyElement {
    let key = section.key.clone();
    let dispatch = dispatch.clone();
    let hover_text = theme.text;
    div()
        .id(SharedString::from(format!(
            "{}-more-{}",
            props.row_id, section.key
        )))
        .h(px(LINE_HEIGHT))
        .flex()
        .items_center()
        .cursor_pointer()
        .text_color(theme.text_muted)
        .hover(move |style| style.text_color(hover_text))
        .on_click(move |_, window, cx| dispatch(BoardAction::ShowAll(key.clone()), window, cx))
        .child(SharedString::from(format!("Show {hidden} more")))
        .into_any_element()
}

fn script_control(
    props: &BoardProps<'_>,
    dispatch: &BoardDispatch,
    theme: &Theme,
) -> Option<AnyElement> {
    let path = props.script_path.clone()?;
    let open = props.state.script_open;
    let dispatch = dispatch.clone();
    let hover_text = theme.text;
    Some(
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!("{}-script", props.row_id)))
                    .h(px(LINE_HEIGHT))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .text_color(theme.text_faint)
                    .hover(move |style| style.text_color(hover_text))
                    .on_click(move |_, window, cx| dispatch(BoardAction::ToggleScript, window, cx))
                    .child(if open { "Hide script" } else { "View script" }),
            )
            .when(open, |block| {
                block.child(
                    div()
                        .font_family(theme.font_mono.clone())
                        .text_size(px(11.0))
                        .text_color(theme.text_muted)
                        .child(path),
                )
            })
            .into_any_element(),
    )
}

/// What the whole board needs to know about its run, computed once.
struct RunFacts {
    sections: Vec<Section>,
    states: Vec<PhaseState>,
    total: usize,
    done: usize,
    failed: usize,
    elapsed: Option<String>,
}

impl RunFacts {
    fn new(props: &BoardProps<'_>) -> Self {
        let sections = workflow_sections(&[], props.progress, props.run);
        let states = sections.iter().map(|s| phase_state(s, props.run)).collect();
        let agents = || sections.iter().flat_map(|s| &s.agents);
        let total = agents().count();
        let done = agents().filter(|a| a.status == AgentStatus::Done).count();
        let failed = agents().filter(|a| a.status == AgentStatus::Failed).count();
        let (start, end) = run_span(&sections);
        let elapsed = elapsed(
            start,
            (props.run != RunStatus::Working).then_some(end).flatten(),
            props.run == RunStatus::Working,
            props.now_ms,
        );
        Self {
            sections,
            states,
            total,
            done,
            failed,
            elapsed,
        }
    }

    /// The phase the run is in now (its first active phase).
    fn current_phase(&self) -> Option<&str> {
        self.sections
            .iter()
            .zip(&self.states)
            .find(|(_, state)| matches!(state, PhaseState::Active))
            .and_then(|(section, _)| section.title.as_deref())
    }

    /// "Verify · 4 of 6 done · 2m 23s"
    fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.extend(self.current_phase().map(str::to_owned));
        if self.total > 0 {
            parts.push(format!("{} of {} done", self.done, self.total));
        }
        if self.failed > 0 {
            parts.push(format!("{} failed", self.failed));
        }
        parts.extend(self.elapsed.clone());
        parts.join(" · ")
    }
}

/// Live and failed phases list their agents; settled ones fold to a line
/// unless the user opened them.
fn section_open(props: &BoardProps<'_>, section: &Section, state: PhaseState) -> bool {
    let default = matches!(state, PhaseState::Active | PhaseState::Failed);
    !section.agents.is_empty() && default != props.state.flipped.contains(&section.key)
}

/// A section's visible agents and how many stay behind "Show N more".
fn visible_agents<'s>(props: &BoardProps<'_>, section: &'s Section) -> (&'s [AgentView], usize) {
    let shown = if props.state.expanded.contains(&section.key) {
        section.agents.len()
    } else {
        section.agents.len().min(VISIBLE_AGENTS)
    };
    (&section.agents[..shown], section.agents.len() - shown)
}

fn pending_text(section: &Section, state: PhaseState) -> String {
    section.detail.clone().unwrap_or_else(|| {
        if state == PhaseState::Pending {
            "Waiting for earlier phases".into()
        } else {
            "No agents ran".into()
        }
    })
}

/// A section's own span, for its phase line.
fn section_elapsed(section: &Section, state: PhaseState, now_ms: i64) -> Option<String> {
    let (start, end) = run_span(std::slice::from_ref(section));
    let live = state == PhaseState::Active;
    elapsed(start, (!live).then_some(end).flatten(), live, now_ms)
}

/// The run's header line; the whole line folds the run.
fn header_line(
    props: &mut BoardProps<'_>,
    facts: &RunFacts,
    open: bool,
    dispatch: &BoardDispatch,
    theme: &Theme,
) -> AnyElement {
    let dispatch = dispatch.clone();
    let hover_text = theme.text;
    let summary = SharedString::from(facts.summary());
    let summary = match props.status_shimmer.take() {
        Some(shimmer) if props.run == RunStatus::Working => shimmer(summary),
        _ => summary.into_any_element(),
    };
    div()
        .id(SharedString::from(format!("{}-header", props.row_id)))
        .h(px(LINE_HEIGHT))
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .cursor_pointer()
        .text_color(theme.text_muted)
        .hover(move |style| style.text_color(hover_text))
        .on_click(move |_, window, cx| dispatch(BoardAction::ToggleBoard, window, cx))
        .child(claude_mark(
            match props.run {
                RunStatus::Working => AgentStatus::Working,
                RunStatus::Failed => AgentStatus::Failed,
                // A settled run keeps Claude's colour: it names the run.
                RunStatus::Done | RunStatus::Stopped => AgentStatus::Working,
            },
            props.spin.filter(|_| props.run == RunStatus::Working),
            theme,
        ))
        .child(
            div()
                .flex_none()
                .max_w(gpui::relative(0.5))
                .truncate()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(props.name.clone()),
        )
        .child(div().min_w_0().truncate().child(summary))
        .child(chevron(open, theme))
        .into_any_element()
}

/// One phase: its line, then its agents while open.
fn phase_block(
    props: &BoardProps<'_>,
    section: &Section,
    state: PhaseState,
    dispatch: &BoardDispatch,
    theme: &Theme,
) -> AnyElement {
    let open = section_open(props, section, state);
    let has_agents = !section.agents.is_empty();
    let mut meta: Vec<String> = Vec::new();
    if has_agents {
        let n = section.agents.len();
        meta.push(if open {
            format!("{}/{n}", section.done)
        } else {
            format!("{n} {}", if n == 1 { "agent" } else { "agents" })
        });
        meta.extend(section_elapsed(section, state, props.now_ms));
    } else {
        meta.push(pending_text(section, state));
    }
    let key = section.key.clone();
    let toggle = dispatch.clone();
    let hover_text = theme.text;
    let line = div()
        .id(SharedString::from(format!(
            "{}-phase-{}",
            props.row_id, section.key
        )))
        .h(px(LINE_HEIGHT))
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .text_color(theme.text_muted)
        .when(has_agents, |row| {
            row.cursor_pointer()
                .hover(move |style| style.text_color(hover_text))
                .on_click(move |_, window, cx| {
                    toggle(BoardAction::ToggleSection(key.clone()), window, cx)
                })
        })
        .child(phase_mark(state, theme))
        .child(
            div()
                .flex_none()
                .text_color(if state == PhaseState::Pending {
                    theme.text_muted
                } else {
                    theme.text
                })
                .child(SharedString::from(
                    section.title.clone().unwrap_or_else(|| "Agents".into()),
                )),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(theme.text_faint)
                .child(SharedString::from(meta.join(" · "))),
        )
        .when(has_agents, |row| row.child(chevron(open, theme)));
    let (agents, hidden) = visible_agents(props, section);
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .child(line)
        .when(open, |block| {
            block.child(
                div()
                    .ml(px(CLAUDE_SLOT / 2.0))
                    .pl(px(CLAUDE_SLOT / 2.0 + 8.0))
                    .border_l_1()
                    .border_color(theme.hairline(0.1))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .children(
                        agents
                            .iter()
                            .map(|agent| agent_line(props, agent, dispatch, theme)),
                    )
                    .when(hidden > 0, |block| {
                        block.child(more_control(props, section, hidden, dispatch, theme))
                    }),
            )
        })
        .into_any_element()
}

/// The whole run: one summary line, then (while open) a line per phase with
/// the live phase's agents beneath it. No card: it sits on the transcript's
/// text edge like any other work row.
pub(crate) fn render_board(
    mut props: BoardProps<'_>,
    dispatch: BoardDispatch,
    theme: &Theme,
) -> AnyElement {
    let facts = RunFacts::new(&props);
    // A live run stays open, like a working subagent group; a settled run
    // folds to its header line.
    let open = (props.run == RunStatus::Working) != props.state.board_flipped;
    let header = header_line(&mut props, &facts, open, &dispatch, theme);
    let body = open.then(|| {
        div()
            .ml(px(CLAUDE_SLOT / 2.0))
            .pl(px(CLAUDE_SLOT / 2.0 + 8.0))
            .border_l_1()
            .border_color(theme.hairline(0.1))
            .min_w_0()
            .flex()
            .flex_col()
            .children(
                facts
                    .sections
                    .iter()
                    .zip(&facts.states)
                    .map(|(section, state)| phase_block(&props, section, *state, &dispatch, theme)),
            )
            .children(script_control(&props, &dispatch, theme))
    });
    div()
        .min_w_0()
        .w_full()
        .py(px(4.0))
        .text_size(px(12.0))
        .line_height(px(16.0))
        .child(header)
        .children(body)
        .into_any_element()
}

/// One agent as the right pane shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentSnapshot {
    pub workflow: SharedString,
    pub phase: Option<String>,
    pub run: RunStatus,
    pub agent: AgentView,
}

/// Find one agent of a run by its index, with the phase it ran under.
pub(crate) fn find_agent(
    progress: Option<&WorkflowProgress>,
    run: RunStatus,
    index: u32,
) -> Option<(Option<String>, AgentView)> {
    workflow_sections(&[], progress, run)
        .into_iter()
        .find_map(|section| {
            let title = section.title;
            section
                .agents
                .into_iter()
                .find(|agent| agent.index == index)
                .map(|agent| (title, agent))
        })
}

/// One agent as a conversation the transcript can render: what it was
/// asked as the user turn, what it returned (or the error) as the reply. The
/// reply streams while the agent works and is absent while it waits.
pub(crate) fn agent_entries(snapshot: &AgentSnapshot, device_id: &str) -> Vec<SessionMessageEntry> {
    let agent = &snapshot.agent;
    let started = agent.started_at.unwrap_or(0);
    let text = |id: &str, text: String| MessagePart::Text {
        id: id.into(),
        text,
    };
    let mut entries = vec![SessionMessageEntry {
        id: format!("workflow-agent-{}-prompt", agent.index),
        role: MessageRole::User,
        parts: vec![text(
            "prompt",
            agent.prompt.clone().unwrap_or_else(|| agent.label.clone()),
        )],
        created_at: started,
        device_id: device_id.to_owned(),
        status: None,
        continuation_of: None,
        duration_ms: None,
    }];
    if agent.status == AgentStatus::Queued {
        return entries;
    }
    let status = match agent.status {
        AgentStatus::Working | AgentStatus::Queued => MessageStatus::Streaming,
        AgentStatus::Done | AgentStatus::Failed => MessageStatus::Complete,
        AgentStatus::Stopped => MessageStatus::Aborted,
    };
    entries.push(SessionMessageEntry {
        id: format!("workflow-agent-{}-reply", agent.index),
        role: MessageRole::Assistant,
        parts: agent
            .result
            .clone()
            .map(|result| vec![text("reply", result)])
            .unwrap_or_default(),
        created_at: started,
        device_id: device_id.to_owned(),
        status: Some(status),
        continuation_of: None,
        duration_ms: agent
            .completed_at
            .zip(agent.started_at)
            .map(|(end, start)| end - start),
    });
    entries
}

/// Wall-clock now in epoch ms (agent timestamps are epoch ms).
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zuse_protocol::WorkflowPhase;

    fn agent(index: u32, label: &str, state: WorkflowAgentState) -> WorkflowAgentProgress {
        WorkflowAgentProgress {
            index,
            label: label.into(),
            state,
            phase_index: None,
            phase_title: None,
            model: None,
            started_at: None,
            duration_ms: None,
            prompt_preview: None,
            result_preview: None,
            error: None,
        }
    }

    fn phase(index: u32, title: &str) -> WorkflowPhase {
        WorkflowPhase {
            index,
            title: title.into(),
        }
    }

    fn declared(title: &str, detail: Option<&str>) -> DeclaredPhase {
        DeclaredPhase {
            title: title.into(),
            detail: detail.map(Into::into),
        }
    }

    /// A run mid-flight: a done and a running agent under Map, a queued agent
    /// outside any phase, and a failed agent addressed by phase title only.
    fn progress() -> WorkflowProgress {
        WorkflowProgress {
            phases: vec![phase(1, "Map"), phase(2, "Review")],
            agents: vec![
                WorkflowAgentProgress {
                    phase_index: Some(1),
                    phase_title: Some("Map".into()),
                    started_at: Some(10),
                    duration_ms: Some(5),
                    prompt_preview: Some("Read a".into()),
                    result_preview: Some("A is fine".into()),
                    ..agent(1, "map:a", WorkflowAgentState::Done)
                },
                WorkflowAgentProgress {
                    phase_index: Some(1),
                    started_at: Some(12),
                    duration_ms: Some(8),
                    prompt_preview: Some("Read b".into()),
                    ..agent(2, "map:b", WorkflowAgentState::Running)
                },
                agent(3, "setup", WorkflowAgentState::Queued),
                WorkflowAgentProgress {
                    phase_title: Some("Review".into()),
                    error: Some("Timed out".into()),
                    ..agent(4, "check:a", WorkflowAgentState::Failed)
                },
            ],
        }
    }

    type Summary = (Option<String>, Vec<(String, AgentStatus)>, usize);

    fn summary(sections: &[Section]) -> Vec<Summary> {
        sections
            .iter()
            .map(|s| {
                (
                    s.title.clone(),
                    s.agents
                        .iter()
                        .map(|a| (a.label.clone(), a.status))
                        .collect(),
                    s.done,
                )
            })
            .collect()
    }

    #[test]
    fn groups_agents_under_declared_phases_in_script_order() {
        let sections = workflow_sections(
            &[
                declared("Review", Some("Check findings")),
                declared("Map", None),
                declared("Ship", Some("Not started")),
            ],
            Some(&progress()),
            RunStatus::Working,
        );
        let s = |t: &str| Some(t.to_owned());
        let a = |l: &str, st| (l.to_owned(), st);
        assert_eq!(
            summary(&sections),
            vec![
                (None, vec![a("setup", AgentStatus::Queued)], 0),
                (s("Review"), vec![a("check:a", AgentStatus::Failed)], 0),
                (
                    s("Map"),
                    vec![a("a", AgentStatus::Done), a("b", AgentStatus::Working)],
                    1
                ),
                (s("Ship"), vec![], 0),
            ]
        );
        let first = &sections[2].agents[0];
        assert_eq!(first.result.as_deref(), Some("A is fine"));
        assert_eq!(first.prompt.as_deref(), Some("Read a"));
        assert_eq!((first.started_at, first.completed_at), (Some(10), Some(15)));
        let running = &sections[2].agents[1];
        assert_eq!((running.started_at, running.completed_at), (Some(12), None));
        assert_eq!(sections[1].agents[0].result.as_deref(), Some("Timed out"));
        assert_eq!(sections[3].detail.as_deref(), Some("Not started"));
    }

    #[test]
    fn runtime_phases_alone_build_the_board() {
        let sections = workflow_sections(&[], Some(&progress()), RunStatus::Working);
        let titles: Vec<_> = sections.iter().map(|s| s.title.as_deref()).collect();
        assert_eq!(titles, [None, Some("Map"), Some("Review")]);
        let keys: Vec<_> = sections.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["unphased", "runtime:1", "runtime:2"]);
        assert!(workflow_sections(&[], None, RunStatus::Working).is_empty());
    }

    #[test]
    fn settles_agents_the_final_snapshot_left_active() {
        let statuses = |run| -> Vec<AgentStatus> {
            workflow_sections(&[], Some(&progress()), run)
                .iter()
                .flat_map(|s| s.agents.iter().map(|a| a.status))
                .collect()
        };
        use AgentStatus::*;
        // Section order: unphased (setup), Map (a, b), Review (check:a).
        assert_eq!(statuses(RunStatus::Done), [Done, Done, Done, Failed]);
        assert_eq!(
            statuses(RunStatus::Stopped),
            [Stopped, Done, Stopped, Failed]
        );
        assert_eq!(
            statuses(RunStatus::Failed),
            [Stopped, Done, Stopped, Failed]
        );
    }

    #[test]
    fn repeated_titles_stay_separate_phases() {
        let progress = WorkflowProgress {
            phases: vec![phase(10, "Review"), phase(20, "Review")],
            agents: vec![
                WorkflowAgentProgress {
                    phase_index: Some(10),
                    ..agent(1, "first", WorkflowAgentState::Done)
                },
                WorkflowAgentProgress {
                    phase_index: Some(20),
                    ..agent(2, "second", WorkflowAgentState::Running)
                },
            ],
        };
        let sections = workflow_sections(
            &[
                declared("Review", Some("First review")),
                declared("Review", Some("Second review")),
            ],
            Some(&progress),
            RunStatus::Working,
        );
        let got: Vec<_> = sections
            .iter()
            .map(|s| {
                (
                    s.detail.clone(),
                    s.agents.iter().map(|a| a.label.clone()).collect::<Vec<_>>(),
                    s.done,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (Some("First review".into()), vec!["first".to_owned()], 1),
                (Some("Second review".into()), vec!["second".to_owned()], 0),
            ]
        );
        let runtime = workflow_sections(&[], Some(&progress), RunStatus::Done);
        let keys: Vec<_> = runtime.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, ["runtime:10", "runtime:20"]);
        assert_eq!(
            workflow_sections(
                &[declared("Review", None), declared("Review", None)],
                None,
                RunStatus::Working
            )
            .len(),
            2
        );
    }

    #[test]
    fn phase_states_prioritize_failure_then_activity() {
        let sections = workflow_sections(
            &[declared("Ship", None)],
            Some(&progress()),
            RunStatus::Working,
        );
        let states: Vec<_> = sections
            .iter()
            .map(|s| (s.title.clone(), phase_state(s, RunStatus::Working)))
            .collect();
        assert_eq!(
            states,
            vec![
                (None, PhaseState::Active),
                (Some("Ship".into()), PhaseState::Pending),
                (Some("Map".into()), PhaseState::Active),
                (Some("Review".into()), PhaseState::Failed),
            ]
        );
        // Settled runs never leave a phase pending.
        let ship = &sections[1];
        assert_eq!(phase_state(ship, RunStatus::Done), PhaseState::Stopped);
        let done = workflow_sections(&[], Some(&progress()), RunStatus::Done);
        assert_eq!(phase_state(&done[1], RunStatus::Done), PhaseState::Done);
    }

    #[test]
    fn labels_drop_their_phase_prefix_only_when_it_matches() {
        assert_eq!(label_within_phase("map:catalog", Some("Map")), "catalog");
        assert_eq!(
            label_within_phase("map:catalog", Some("Review")),
            "map:catalog"
        );
        assert_eq!(label_within_phase("map:", Some("Map")), "map:");
        assert_eq!(label_within_phase(":x", Some("")), ":x");
        assert_eq!(label_within_phase("map:catalog", None), "map:catalog");
    }

    #[test]
    fn elapsed_reads_like_delegation_rows() {
        assert_eq!(format_elapsed_ms(-5), "0s");
        assert_eq!(format_elapsed_ms(42_900), "42s");
        assert_eq!(format_elapsed_ms(185_000), "3m 05s");
        assert_eq!(format_elapsed_ms(3_720_000), "1h 02m");
        assert_eq!(elapsed(Some(0), None, true, 2_000).as_deref(), Some("2s"));
        assert_eq!(elapsed(Some(0), None, false, 2_000), None);
        assert_eq!(elapsed(None, Some(5), false, 0), None);
    }

    #[test]
    fn agent_lines_say_what_each_agent_is_doing() {
        let sections = workflow_sections(&[], Some(&progress()), RunStatus::Working);
        let (phase, done) = find_agent(Some(&progress()), RunStatus::Working, 1).unwrap();
        assert_eq!(phase.as_deref(), Some("Map"));
        assert_eq!(agent_tail(&done).as_deref(), Some("A is fine"));
        let running = &sections[1].agents[1];
        assert_eq!(running.status, AgentStatus::Working);
        assert_eq!(agent_tail(running).as_deref(), Some("Read b"));
        let queued = &sections[0].agents[0];
        assert_eq!(agent_tail(queued).as_deref(), Some("Queued"));
        assert!(find_agent(Some(&progress()), RunStatus::Working, 99).is_none());
        assert_eq!(one_line("  a\n\tb  c "), "a b c");
    }

    #[test]
    fn an_agent_reads_as_prompt_then_reply() {
        let snapshot = |index| {
            let (phase, agent) = find_agent(Some(&progress()), RunStatus::Working, index).unwrap();
            AgentSnapshot {
                workflow: "audit".into(),
                phase,
                run: RunStatus::Working,
                agent,
            }
        };
        let text = |entry: &SessionMessageEntry| match entry.parts.first() {
            Some(MessagePart::Text { text, .. }) => Some(text.clone()),
            _ => None,
        };
        let done = agent_entries(&snapshot(1), "device");
        assert_eq!(done.len(), 2);
        assert_eq!(done[0].role, MessageRole::User);
        assert_eq!(text(&done[0]).as_deref(), Some("Read a"));
        assert_eq!(done[1].role, MessageRole::Assistant);
        assert_eq!(text(&done[1]).as_deref(), Some("A is fine"));
        assert_eq!(done[1].status, Some(MessageStatus::Complete));
        assert_eq!(done[1].duration_ms, Some(5));
        let working = agent_entries(&snapshot(2), "device");
        assert_eq!(working[1].status, Some(MessageStatus::Streaming));
        assert!(working[1].parts.is_empty(), "no reply until it returns");
        let queued = agent_entries(&snapshot(3), "device");
        assert_eq!(queued.len(), 1, "a waiting agent has only its prompt");
        assert_eq!(text(&queued[0]).as_deref(), Some("setup"));
    }

    #[test]
    fn board_actions_toggle_disclosures() {
        let mut state = BoardState::default();
        assert!(
            !state.apply(&BoardAction::Open(3)),
            "opening leaves the board as is"
        );
        assert_eq!(state, BoardState::default());
        state.apply(&BoardAction::ToggleSection("runtime:1".into()));
        assert!(state.flipped.contains("runtime:1"));
        state.apply(&BoardAction::ToggleSection("runtime:1".into()));
        assert!(state.flipped.is_empty());
        state.apply(&BoardAction::ToggleBoard);
        assert!(state.board_flipped);
        state.apply(&BoardAction::ShowAll("runtime:1".into()));
        assert!(state.expanded.contains("runtime:1"));
        state.apply(&BoardAction::ToggleScript);
        assert!(state.script_open);
    }
}
