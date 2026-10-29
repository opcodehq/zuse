//! A native workflow run as a phase board: one column per phase, left to
//! right in run order, each agent a tile. Selecting a tile shows what it was
//! asked and what it returned; columns stack when the transcript is narrow.
//!
//! The pure half ([`workflow_sections`], [`phase_state`]) groups a run's
//! snapshot ([`WorkflowProgress`], carried on the run's tool call) into
//! sections; [`render_board`] draws them. Interaction state (selection,
//! expanded columns, the script disclosure) lives with the transcript, keyed
//! by row, and comes back through [`BoardAction`]s.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AnyElement, App, Hsla, SharedString, Window, div, prelude::*, px};
use zuse_protocol::{WorkflowAgentProgress, WorkflowAgentState, WorkflowProgress};

use crate::theme::Theme;

/// Tiles a column shows before its "Show N more" control.
pub(crate) const VISIBLE_AGENTS: usize = 6;
/// Below this content width the columns stack (a 32rem container).
pub(crate) const STACK_BELOW_WIDTH: f32 = 512.0;
/// A working agent's mark turns once per this period.
const MARK_TURN: Duration = Duration::from_secs(3);

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
    /// The tile's hover text: the result (or error), else the prompt.
    pub preview: Option<String>,
    pub prompt: Option<String>,
    pub result: Option<String>,
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
        preview: result.clone().or_else(|| agent.prompt_preview.clone()),
        prompt: agent.prompt_preview.clone(),
        result,
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

fn run_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Working => "Running",
        RunStatus::Done => "Done",
        RunStatus::Failed => "Failed",
        RunStatus::Stopped => "Stopped",
    }
}

fn agent_label(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Queued => "Queued",
        AgentStatus::Working => "Running",
        AgentStatus::Done => "Done",
        AgentStatus::Failed => "Failed",
        AgentStatus::Stopped => "Stopped",
    }
}

/// Per-row interaction state, owned by the transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BoardState {
    pub selected: Option<u32>,
    /// Section keys whose every tile shows.
    pub expanded: HashSet<String>,
    pub script_open: bool,
}

impl BoardState {
    /// Apply an action; `true` when the board's height may have changed.
    pub(crate) fn apply(&mut self, action: &BoardAction) -> bool {
        match action {
            BoardAction::Select(index) => {
                self.selected = (self.selected != Some(*index)).then_some(*index);
            }
            BoardAction::ShowAll(key) => {
                self.expanded.insert(key.clone());
            }
            BoardAction::ToggleScript => self.script_open = !self.script_open,
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BoardAction {
    /// Toggle the detail panel for this agent.
    Select(u32),
    /// Show every tile of this section.
    ShowAll(String),
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
    /// The chat provider's mark; a generic agent glyph without one.
    pub brand: Option<&'static str>,
    pub stacked: bool,
    pub state: &'a BoardState,
    pub now_ms: i64,
    /// Elapsed since an arbitrary epoch, for the turning marks; `None` holds
    /// them still (reduced motion).
    pub spin: Option<Duration>,
    /// The status word's shimmer: (text, phase) → element.
    pub status_shimmer: Option<Box<dyn FnOnce(SharedString) -> AnyElement + 'a>>,
}

/// Run status tones (lime while working, emerald when done).
fn run_tone(status: RunStatus, theme: &Theme) -> Hsla {
    match status {
        RunStatus::Working => theme.busy,
        RunStatus::Done => theme.success,
        RunStatus::Failed => theme.danger,
        RunStatus::Stopped => theme.text_muted,
    }
}

fn column_tone(state: PhaseState, theme: &Theme) -> Hsla {
    match state {
        PhaseState::Pending | PhaseState::Stopped => theme.ink(0.025),
        PhaseState::Active => theme.busy.opacity(0.06),
        PhaseState::Done => theme.ink(0.04),
        PhaseState::Failed => theme.danger.opacity(0.06),
    }
}

/// The provider's mark: it turns while the agent works and takes the
/// outcome's tone after.
fn agent_mark(
    brand: Option<&'static str>,
    status: AgentStatus,
    spin: Option<Duration>,
    theme: &Theme,
) -> AnyElement {
    let tone = match status {
        AgentStatus::Queued | AgentStatus::Stopped => theme.text_muted.opacity(0.4),
        AgentStatus::Working => theme.text.opacity(0.85),
        AgentStatus::Done => theme.text_muted,
        AgentStatus::Failed => theme.danger,
    };
    let mark = crate::icons::icon(brand.unwrap_or(crate::icons::BOT))
        .size(px(14.0))
        .text_color(tone);
    match spin.filter(|_| status == AgentStatus::Working) {
        Some(spin) => {
            let turn = (spin.as_secs_f32() % MARK_TURN.as_secs_f32()) / MARK_TURN.as_secs_f32();
            mark.with_transformation(gpui::Transformation::rotate(gpui::radians(
                turn * std::f32::consts::TAU,
            )))
            .into_any_element()
        }
        None => mark.into_any_element(),
    }
}

/// A phase's glyph: a live dot while active, else its outcome.
fn phase_glyph(state: PhaseState, theme: &Theme) -> AnyElement {
    let slot = div()
        .size(px(14.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center();
    match state {
        PhaseState::Active => slot
            .child(
                div()
                    .size(px(14.0))
                    .rounded_full()
                    .bg(theme.busy.opacity(0.2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(px(8.0)).rounded_full().bg(theme.busy)),
            )
            .into_any_element(),
        PhaseState::Pending => slot
            .child(
                div()
                    .size(px(11.0))
                    .rounded_full()
                    .border_1()
                    .border_dashed()
                    .border_color(theme.text_muted.opacity(0.5)),
            )
            .into_any_element(),
        PhaseState::Done => slot
            .child(
                crate::icons::icon(crate::icons::CHECK_CIRCLE)
                    .size(px(14.0))
                    .text_color(theme.success),
            )
            .into_any_element(),
        PhaseState::Failed => slot
            .child(
                crate::icons::icon(crate::icons::CLOSE_CIRCLE)
                    .size(px(14.0))
                    .text_color(theme.danger),
            )
            .into_any_element(),
        PhaseState::Stopped => slot
            .child(
                div()
                    .size(px(11.0))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.text_muted.opacity(0.6))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w(px(5.0))
                            .h(px(1.0))
                            .bg(theme.text_muted.opacity(0.6)),
                    ),
            )
            .into_any_element(),
    }
}

fn agent_timing(agent: &AgentView, now_ms: i64) -> Option<String> {
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

#[allow(clippy::too_many_arguments)]
fn agent_tile(
    row_id: &SharedString,
    agent: &AgentView,
    selected: bool,
    brand: Option<&'static str>,
    spin: Option<Duration>,
    now_ms: i64,
    dispatch: &BoardDispatch,
    theme: &Theme,
) -> AnyElement {
    let inspectable = agent.prompt.is_some() || agent.result.is_some();
    let tile = div()
        .id(SharedString::from(format!(
            "{row_id}-agent-{}",
            agent.index
        )))
        .h(px(32.0))
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(8.0))
        .rounded(px(8.0))
        .border_1()
        .border_color(if selected {
            theme.hairline(0.2)
        } else {
            gpui::transparent_black()
        })
        .bg(theme.bg.opacity(0.7))
        .child(agent_mark(brand, agent.status, spin, theme))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_color(if agent.status == AgentStatus::Failed {
                    theme.danger
                } else {
                    theme.text.opacity(0.85)
                })
                .child(SharedString::from(agent.label.clone())),
        )
        .when_some(agent_timing(agent, now_ms), |tile, timing| {
            tile.child(
                div()
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.7))
                    .child(SharedString::from(timing)),
            )
        });
    if !inspectable {
        return tile.into_any_element();
    }
    let index = agent.index;
    let dispatch = dispatch.clone();
    let hover = theme.bg;
    tile.cursor_pointer()
        .hover(move |style| style.bg(hover))
        .when_some(agent.preview.clone(), |tile, preview| {
            tile.tooltip(crate::settings::widgets::text_tooltip(preview))
        })
        .on_click(move |_, window, cx| dispatch(BoardAction::Select(index), window, cx))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn phase_column(
    props: &BoardProps<'_>,
    section: &Section,
    state: PhaseState,
    last: bool,
    dispatch: &BoardDispatch,
    theme: &Theme,
) -> AnyElement {
    let show_all = props.state.expanded.contains(&section.key);
    let visible = if show_all {
        section.agents.len()
    } else {
        section.agents.len().min(VISIBLE_AGENTS)
    };
    let hidden = section.agents.len() - visible;
    let header = div()
        .h(px(24.0))
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(px(4.0))
        .child(phase_glyph(state, theme))
        .child(
            div()
                .min_w_0()
                .truncate()
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(if state == PhaseState::Pending {
                    theme.text_muted
                } else {
                    theme.text.opacity(0.9)
                })
                .child(SharedString::from(
                    section.title.clone().unwrap_or_else(|| "Agents".into()),
                )),
        )
        .when(!section.agents.is_empty(), |header| {
            header.child(
                div()
                    .ml_auto()
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.7))
                    .child(SharedString::from(format!(
                        "{}/{}",
                        section.done,
                        section.agents.len()
                    ))),
            )
        });
    let body = if section.agents.is_empty() {
        div()
            .px(px(4.0))
            .pb(px(4.0))
            .text_color(theme.text_muted.opacity(0.6))
            .child(SharedString::from(section.detail.clone().unwrap_or_else(
                || {
                    if state == PhaseState::Pending {
                        "Starts after the earlier phases finish".into()
                    } else {
                        "No agents ran in this phase".into()
                    }
                },
            )))
            .into_any_element()
    } else {
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(section.agents[..visible].iter().map(|agent| {
                agent_tile(
                    &props.row_id,
                    agent,
                    props.state.selected == Some(agent.index),
                    props.brand,
                    props.spin,
                    props.now_ms,
                    dispatch,
                    theme,
                )
            }))
            .into_any_element()
    };
    let more = (hidden > 0).then(|| {
        let key = section.key.clone();
        let dispatch = dispatch.clone();
        let hover_bg = theme.bg.opacity(0.6);
        let hover_text = theme.text;
        div()
            .id(SharedString::from(format!(
                "{}-more-{}",
                props.row_id, section.key
            )))
            .h(px(28.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .rounded(px(8.0))
            .cursor_pointer()
            .text_color(theme.text_muted)
            .hover(move |style| style.bg(hover_bg).text_color(hover_text))
            .on_click(move |_, window, cx| dispatch(BoardAction::ShowAll(key.clone()), window, cx))
            .child(SharedString::from(format!("Show {hidden} more")))
    });
    let column = div()
        .min_w_0()
        .flex_1()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .p(px(8.0))
        .rounded(px(12.0))
        .bg(column_tone(state, theme))
        .child(header)
        .child(body)
        .children(more);
    div()
        .min_w_0()
        .flex_1()
        .flex()
        .flex_row()
        .items_stretch()
        .gap(px(6.0))
        .when(!props.stacked, |item| item.min_w(px(160.0)))
        .child(column)
        .when(!last && !props.stacked, |item| {
            item.child(
                div().flex_none().flex().items_center().child(
                    crate::icons::icon(crate::icons::ALT_ARROW_RIGHT)
                        .size(px(12.0))
                        .text_color(theme.text_muted.opacity(0.4)),
                ),
            )
        })
        .into_any_element()
}

fn preview(
    title: &'static str,
    text: &str,
    failed: bool,
    id: SharedString,
    theme: &Theme,
) -> AnyElement {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(11.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(title),
        )
        .child(
            div()
                .id(id)
                .mt(px(2.0))
                .max_h(px(160.0))
                .overflow_y_scroll()
                .line_height(px(19.5))
                .text_color(if failed {
                    theme.danger
                } else {
                    theme.text.opacity(0.85)
                })
                .child(SharedString::from(text.to_owned())),
        )
        .into_any_element()
}

fn agent_detail(props: &BoardProps<'_>, agent: &AgentView, theme: &Theme) -> AnyElement {
    div()
        .mt(px(6.0))
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .rounded(px(12.0))
        .bg(theme.ink(0.035))
        .px(px(12.0))
        .py(px(10.0))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(agent_mark(props.brand, agent.status, props.spin, theme))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text.opacity(0.9))
                        .child(SharedString::from(agent.label.clone())),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.text_muted)
                        .child(agent_label(agent.status)),
                )
                .when_some(agent_timing(agent, props.now_ms), |row, timing| {
                    row.child(
                        div()
                            .ml_auto()
                            .flex_none()
                            .text_color(theme.text_muted.opacity(0.7))
                            .child(SharedString::from(timing)),
                    )
                }),
        )
        .when_some(agent.prompt.as_deref(), |detail, prompt| {
            detail.child(preview(
                "Prompt",
                prompt,
                false,
                SharedString::from(format!("{}-prompt-{}", props.row_id, agent.index)),
                theme,
            ))
        })
        .when_some(agent.result.as_deref(), |detail, result| {
            detail.child(preview(
                "Result",
                result,
                agent.status == AgentStatus::Failed,
                SharedString::from(format!("{}-result-{}", props.row_id, agent.index)),
                theme,
            ))
        })
        .into_any_element()
}

/// The whole board for one run.
pub(crate) fn render_board(
    mut props: BoardProps<'_>,
    dispatch: BoardDispatch,
    theme: &Theme,
) -> AnyElement {
    let sections = workflow_sections(&[], props.progress, props.run);
    let states: Vec<PhaseState> = sections.iter().map(|s| phase_state(s, props.run)).collect();
    let agents: Vec<&AgentView> = sections.iter().flat_map(|s| &s.agents).collect();
    let done = agents
        .iter()
        .filter(|a| a.status == AgentStatus::Done)
        .count();
    let failed = agents
        .iter()
        .filter(|a| a.status == AgentStatus::Failed)
        .count();
    let selected = props
        .state
        .selected
        .and_then(|ix| agents.iter().find(|a| a.index == ix).copied());
    let (start, end) = run_span(&sections);
    let run_elapsed = elapsed(
        start,
        (props.run != RunStatus::Working).then_some(end).flatten(),
        props.run == RunStatus::Working,
        props.now_ms,
    );
    let mut facts = String::new();
    if !agents.is_empty() {
        facts.push_str(&format!(" · {done} of {} agents done", agents.len()));
    }
    if failed > 0 {
        facts.push_str(&format!(" · {failed} failed"));
    }
    if let Some(elapsed) = &run_elapsed {
        facts.push_str(&format!(" · {elapsed}"));
    }
    let status_text = SharedString::from(run_label(props.run));
    let status = match props.status_shimmer.take() {
        Some(shimmer) if props.run == RunStatus::Working => shimmer(status_text),
        _ => status_text.into_any_element(),
    };
    let header = div()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(
            crate::icons::icon(props.brand.unwrap_or(crate::icons::TOOL_AGENT))
                .size(px(16.0))
                .text_color(theme.text.opacity(0.8)),
        )
        .child(
            div()
                .min_w_0()
                .flex_shrink(1.0)
                .truncate()
                .text_size(px(13.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(props.name.clone()),
        )
        .child(
            div()
                .ml_auto()
                .min_w_0()
                .max_w(gpui::relative(0.65))
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .overflow_hidden()
                .text_color(theme.text_muted)
                .child(
                    div()
                        .flex_none()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(run_tone(props.run, theme))
                        .child(status),
                )
                .child(div().min_w_0().truncate().child(SharedString::from(facts))),
        );
    let columns = (!sections.is_empty()).then(|| {
        let last = sections.len() - 1;
        div()
            .mt(px(12.0))
            .min_w_0()
            .flex()
            .gap(px(6.0))
            .map(|list| {
                if props.stacked {
                    list.flex_col()
                } else {
                    list.flex_row().items_stretch()
                }
            })
            .children(
                sections
                    .iter()
                    .zip(&states)
                    .enumerate()
                    .map(|(ix, (section, state))| {
                        phase_column(&props, section, *state, ix == last, &dispatch, theme)
                    }),
            )
    });
    let detail = selected.map(|agent| agent_detail(&props, agent, theme));
    let script = props.script_path.clone().map(|path| {
        let open = props.state.script_open;
        let dispatch = dispatch.clone();
        let hover_bg = theme.ink(0.06);
        let hover_text = theme.text;
        div()
            .mt(px(6.0))
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id(SharedString::from(format!("{}-script", props.row_id)))
                    .ml(px(-6.0))
                    .h(px(24.0))
                    .px(px(6.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .text_color(theme.text_muted)
                    .hover(move |style| style.bg(hover_bg).text_color(hover_text))
                    .on_click(move |_, window, cx| dispatch(BoardAction::ToggleScript, window, cx))
                    .child(crate::icons::icon(crate::icons::FILE_CODE).size(px(14.0)))
                    .child(if open { "Hide script" } else { "View script" }),
            )
            .when(open, |block| {
                block.child(
                    div()
                        .mt(px(4.0))
                        .font_family(theme.font_mono.clone())
                        .text_size(px(11.0))
                        .text_color(theme.text_muted)
                        .child(path),
                )
            })
    });
    div()
        .min_w_0()
        .w_full()
        .px(px(16.0))
        .py(px(6.0))
        .text_size(px(12.0))
        .line_height(px(16.0))
        .child(header)
        .children(columns)
        .children(detail)
        .children(script)
        .into_any_element()
}

/// Wall-clock now in epoch ms (agent timestamps are epoch ms).
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// One clock for every board's turning marks.
pub(crate) fn spin_epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
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
        assert_eq!(first.preview.as_deref(), Some("A is fine"));
        assert_eq!(first.prompt.as_deref(), Some("Read a"));
        assert_eq!((first.started_at, first.completed_at), (Some(10), Some(15)));
        let running = &sections[2].agents[1];
        assert_eq!((running.started_at, running.completed_at), (Some(12), None));
        assert_eq!(sections[1].agents[0].preview.as_deref(), Some("Timed out"));
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
    fn board_actions_toggle_selection_and_disclosures() {
        let mut state = BoardState::default();
        state.apply(&BoardAction::Select(3));
        assert_eq!(state.selected, Some(3));
        state.apply(&BoardAction::Select(3));
        assert_eq!(state.selected, None);
        state.apply(&BoardAction::ShowAll("runtime:1".into()));
        assert!(state.expanded.contains("runtime:1"));
        state.apply(&BoardAction::ToggleScript);
        assert!(state.script_open);
    }
}
