//! The pull request card a turn leaves in the transcript when its agent opens
//! one: the PR's title and state, the branch's commits beside the base's since
//! they forked, whether it merges cleanly (with the conflicting lines on
//! hover), its checks, and the one next step — resolve, fix, mark ready or
//! merge. It follows the PR live while it shows, and publishes a one-line
//! status the composer's chip reads.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use gpui::{
    Action, AnyElement, App, Context, Entity, EventEmitter, Global, Hsla, PathBuilder,
    SharedString, Subscription, Task, Window, canvas, div, point, prelude::*, px,
};
use zuse_protocol::branch_graph::{BranchGraph, BranchGraphRequest, MergePreview};
use zuse_protocol::change_request_assessment::{check_failed, check_status};
use zuse_protocol::presentation::git_history::{GraphLayout, SegmentShape, layout_graph};
use zuse_protocol::{
    ChangeRequestDetail, ChangeRequestMergeMethod as Method, ChangeRequestUpdate as Update,
    SessionStatus, ToolCall,
};
use zuse_rpc::methods;

use crate::change_requests::ChangeRequestBadgeTone;
use crate::icons::{self, icon};
use crate::popover;
use crate::pull_request_detail::{PullRequestRepair, failure_reason, repair_prompt};
use crate::state::AppState;
use crate::theme::Theme;

/// What the composer's Create PR chip asks the agent; the card follows once
/// `gh pr create` runs.
pub const CREATE_PR_PROMPT: &str = "Create PR: commit what's left, push the branch, and open the pull request with `gh pr create`.";

/// Send `.0` to the focused chat's agent as the user's next message.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct SendToChat(pub String);

const ROW_HEIGHT: f32 = 26.0;
const LANE_SPACING: f32 = 12.0;
const GUTTER_PAD: f32 = 7.0;
const NODE_RADIUS: f32 = 3.5;
const STROKE: f32 = 1.5;
/// Commits listed before the rest fold under "N earlier commits".
const VISIBLE_COMMITS: usize = 10;
const POLL_BUSY: Duration = Duration::from_secs(12);
const POLL_IDLE: Duration = Duration::from_secs(60);
/// A commit row's entrance, one row after another.
const ROW_ENTER: Duration = Duration::from_millis(420);
const ROW_STAGGER: Duration = Duration::from_millis(55);
/// The tint on a commit that arrived after the card first loaded.
const FRESH_TINT: Duration = Duration::from_millis(1600);
/// The footer's and merge row's words changing.
const STAGE_ENTER: Duration = Duration::from_millis(260);

/// Eased progress of an entrance that starts at `start`, 1 once settled.
fn entrance(start: Instant, duration: Duration, now: Instant) -> f32 {
    let elapsed = now.saturating_duration_since(start).as_secs_f32();
    let total = duration.as_secs_f32() * crate::motion::speed_scale();
    if now < start {
        return 0.0;
    }
    crate::motion::EASE_OUT_EXPO.eval((elapsed / total.max(0.001)).min(1.0))
}

// ---------------------------------------------------------------------------
// Pure: finding the PR a turn opened
// ---------------------------------------------------------------------------

/// What the agent was doing toward the PR when the card first shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CreateStep {
    Committing,
    Pushing,
    Opening,
}

impl CreateStep {
    fn label(self) -> &'static str {
        match self {
            Self::Committing => "Committing the last changes",
            Self::Pushing => "Pushing the branch",
            Self::Opening => "Opening the pull request",
        }
    }
}

/// A turn that opened a pull request: its URL once `gh pr create` printed
/// it, else the step still running.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct OpenedPullRequest {
    pub url: Option<String>,
    pub step: Option<CreateStep>,
}

/// The first `https://…/pull/<n>` URL in `text`.
pub fn pull_request_url(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '(' | ')'))
        .filter(|word| word.starts_with("https://"))
        .find_map(|word| {
            let word = word.trim_end_matches(['.', ',', ';', ':', '`']);
            let (_, tail) = word.split_once("/pull/")?;
            let digits = tail.split(['/', '#', '?']).next()?;
            (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| word[..word.len() - tail.len() + digits.len()].to_owned())
        })
}

fn runs(command: &str, needle: &str) -> bool {
    command
        .split(['&', ';', '|', '\n'])
        .any(|part| part.trim_start().starts_with(needle) || part.contains(&format!(" {needle}")))
}

/// The pull request a turn's tool calls opened, from `(call, output,
/// resolved, failed)`: present once `gh pr create` runs; `None` when it
/// didn't, or when it failed without a URL in a finished turn.
pub fn opened_pull_request<'a>(
    tools: impl IntoIterator<Item = (&'a ToolCall, Option<&'a str>, bool, bool)>,
    streaming: bool,
) -> Option<OpenedPullRequest> {
    let mut creating = false;
    let mut url = None;
    let mut step = None;
    for (call, output, resolved, failed) in tools {
        let ToolCall::Exec { command } = call else {
            continue;
        };
        let opens = runs(command, "gh pr create");
        if opens {
            creating = true;
            if resolved && !failed && url.is_none() {
                url = output.and_then(pull_request_url);
            }
        }
        if !resolved {
            step = if opens {
                Some(CreateStep::Opening)
            } else if runs(command, "git push") {
                Some(CreateStep::Pushing)
            } else if runs(command, "git commit") {
                Some(CreateStep::Committing)
            } else {
                step
            };
        }
    }
    if !creating || (url.is_none() && !streaming) {
        return None;
    }
    Some(OpenedPullRequest {
        step: if url.is_some() {
            None
        } else {
            Some(step.unwrap_or(CreateStep::Opening))
        },
        url,
    })
}

// ---------------------------------------------------------------------------
// Pure: where the PR stands
// ---------------------------------------------------------------------------

/// The card's reading of the PR, most pressing first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Creating(CreateStep),
    Loading,
    Checking,
    Conflicts(usize),
    Resolving,
    ChecksRunning { done: usize, total: usize },
    ChecksFailing(usize),
    Draft,
    Blocked(&'static str),
    Ready,
    Merged,
    Closed,
}

fn pending(status: &str) -> bool {
    matches!(
        status,
        "PENDING" | "IN_PROGRESS" | "QUEUED" | "EXPECTED" | "WAITING" | "REQUESTED"
    )
}

/// Conflicting files: the local preview when it ran, else GitHub's flag.
fn conflict_count(detail: Option<&ChangeRequestDetail>, merge: Option<&MergePreview>) -> usize {
    match merge {
        Some(merge) if !merge.clean => (merge.conflicts.len() + merge.more_files).max(1),
        Some(_) => 0,
        None => usize::from(detail.is_some_and(|detail| detail.mergeable == "CONFLICTING")),
    }
}

pub fn stage(
    step: Option<CreateStep>,
    detail: Option<&ChangeRequestDetail>,
    graph: Option<&BranchGraph>,
    resolving: bool,
) -> Stage {
    if let Some(step) = step {
        return Stage::Creating(step);
    }
    let Some(detail) = detail else {
        return Stage::Loading;
    };
    match detail.state.as_str() {
        "MERGED" => return Stage::Merged,
        "CLOSED" => return Stage::Closed,
        _ => {}
    }
    if resolving {
        return Stage::Resolving;
    }
    let conflicts = conflict_count(Some(detail), graph.and_then(|graph| graph.merge.as_ref()));
    if conflicts > 0 {
        return Stage::Conflicts(conflicts);
    }
    let statuses: Vec<String> = detail
        .status_check_rollup
        .iter()
        .map(check_status)
        .collect();
    let failing = statuses
        .iter()
        .filter(|status| check_failed(status))
        .count();
    if failing > 0 {
        return Stage::ChecksFailing(failing);
    }
    let running = statuses.iter().filter(|status| pending(status)).count();
    if running > 0 {
        return Stage::ChecksRunning {
            done: statuses.len() - running,
            total: statuses.len(),
        };
    }
    if detail.is_draft {
        return Stage::Draft;
    }
    if detail.mergeable == "UNKNOWN" || detail.mergeable.is_empty() {
        return Stage::Checking;
    }
    match detail.review_decision.as_str() {
        "CHANGES_REQUESTED" => Stage::Blocked("changes requested"),
        "REVIEW_REQUIRED" => Stage::Blocked("review required"),
        _ => Stage::Ready,
    }
}

impl Stage {
    /// The word beside "Merge into main".
    fn merge_word(&self) -> SharedString {
        match self {
            Self::Creating(_) | Self::Loading | Self::Checking => "checking".into(),
            Self::Conflicts(1) => "1 conflict".into(),
            Self::Conflicts(n) => format!("{n} conflicts").into(),
            Self::Resolving => "resolving".into(),
            Self::ChecksRunning { .. } => "waiting on checks".into(),
            Self::ChecksFailing(_) => "checks failing".into(),
            Self::Draft => "draft".into(),
            Self::Blocked(why) => (*why).into(),
            Self::Ready => "ready".into(),
            Self::Merged => "merged".into(),
            Self::Closed => "closed".into(),
        }
    }

    /// The composer chip's words.
    pub fn chip_label(&self) -> SharedString {
        match self {
            Self::Creating(_) => "Opening".into(),
            Self::Loading | Self::Checking => "Checking".into(),
            Self::Conflicts(1) => "1 conflict".into(),
            Self::Conflicts(n) => format!("{n} conflicts").into(),
            Self::Resolving => "Resolving".into(),
            Self::ChecksRunning { .. } => "Checks running".into(),
            Self::ChecksFailing(1) => "1 check failing".into(),
            Self::ChecksFailing(n) => format!("{n} checks failing").into(),
            Self::Draft => "Draft".into(),
            Self::Blocked(why) => capitalize(why).into(),
            Self::Ready => "Ready to merge".into(),
            Self::Merged => "Merged".into(),
            Self::Closed => "Closed".into(),
        }
    }

    pub fn tone(&self, theme: &Theme) -> Hsla {
        match self {
            Self::Conflicts(_) | Self::Blocked(_) => theme.warning,
            Self::ChecksFailing(_) | Self::Closed => theme.danger,
            Self::Ready => theme.success,
            Self::Resolving | Self::ChecksRunning { .. } => theme.accent,
            Self::Merged => ChangeRequestBadgeTone::Merged.color(theme),
            _ => theme.text_muted,
        }
    }

    fn busy(&self) -> bool {
        matches!(
            self,
            Self::Creating(_)
                | Self::Loading
                | Self::Checking
                | Self::Resolving
                | Self::ChecksRunning { .. }
        )
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// "6m", "2h", "3d" — the commit list's ages.
fn short_age(authored_at: &str, now: DateTime<Utc>) -> String {
    let Ok(at) = DateTime::parse_from_rfc3339(authored_at) else {
        return String::new();
    };
    let seconds = now
        .signed_duration_since(at.with_timezone(&Utc))
        .num_seconds()
        .max(0);
    match seconds {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        86_400..2_592_000 => format!("{}d", seconds / 86_400),
        _ => format!("{}mo", seconds / 2_592_000),
    }
}

// ---------------------------------------------------------------------------
// The composer's view of each PR
// ---------------------------------------------------------------------------

/// The latest stage each card read, by PR URL, for the composer's chip.
#[derive(Default)]
pub struct PullRequestStages(pub HashMap<String, Stage>);

impl Global for PullRequestStages {}

/// Seeded card data by URL (`""` for a PR still being opened); a card found
/// here never reads the engine.
#[derive(Default)]
pub struct PullRequestCardFixtures(pub HashMap<String, (Option<ChangeRequestDetail>, BranchGraph)>);

impl Global for PullRequestCardFixtures {}

// ---------------------------------------------------------------------------
// The card
// ---------------------------------------------------------------------------

/// The card's height may have changed: its row must be measured again.
pub struct CardLayoutChanged;

pub struct PullRequestCard {
    state: gpui::Entity<AppState>,
    watch_control: Option<Entity<crate::pr_watch::PrWatchControl>>,
    watch_subscription: Option<Subscription>,
    chat_id: String,
    url: Option<String>,
    step: Option<CreateStep>,
    detail: Option<ChangeRequestDetail>,
    graph: Option<BranchGraph>,
    layout: GraphLayout,
    fixture: bool,
    fetch: Option<Task<()>>,
    _poll: Option<Task<()>>,
    update: Option<Task<()>>,
    merging: bool,
    notice: Option<SharedString>,
    merge_menu: bool,
    conflicts_open: bool,
    /// Asked the agent to resolve; cleared once its turn ends.
    resolving: bool,
    show_all: bool,
    /// The turn's reading this card last took.
    seen: Option<OpenedPullRequest>,
    /// When each commit row enters, and whether it arrived after the first
    /// load (those flash a tint, like `main` moving on).
    appeared: HashMap<String, (Instant, bool)>,
    /// The stage the card reads now and since when, for its words' entrance.
    stage_since: Option<(Stage, Instant)>,
}

impl EventEmitter<CardLayoutChanged> for PullRequestCard {}

impl PullRequestCard {
    pub fn new(
        state: gpui::Entity<AppState>,
        chat_id: String,
        opened: &OpenedPullRequest,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut card = Self {
            state,
            chat_id,
            watch_control: None,
            watch_subscription: None,
            url: None,
            step: None,
            detail: None,
            graph: None,
            layout: GraphLayout::default(),
            fixture: false,
            fetch: None,
            _poll: None,
            update: None,
            merging: false,
            notice: None,
            merge_menu: false,
            conflicts_open: false,
            resolving: false,
            show_all: false,
            seen: None,
            appeared: HashMap::new(),
            stage_since: None,
        };
        card.set_opened(opened, cx);
        let poll = cx.spawn(async move |this, cx| {
            loop {
                let delay = this
                    .update(cx, |card, cx| card.poll_interval(cx))
                    .ok()
                    .flatten();
                let Some(delay) = delay else { break };
                cx.background_executor().timer(delay).await;
                // Only the chat on screen reads GitHub and the remote.
                let refreshed = this.update(cx, |card, cx| {
                    if card.state.read(cx).selected_chat.as_deref() == Some(card.chat_id.as_str()) {
                        card.refresh(true, cx);
                    }
                });
                if refreshed.is_err() {
                    break;
                }
            }
        });
        card._poll = Some(poll);
        card
    }

    /// The turn's tools moved on: a URL arrived or the running step changed.
    pub fn set_opened(&mut self, opened: &OpenedPullRequest, cx: &mut Context<Self>) {
        if self.seen.as_ref() == Some(opened) {
            return;
        }
        let first = self.seen.is_none();
        self.seen = Some(opened.clone());
        let url_changed = self.url != opened.url;
        self.url = opened.url.clone();
        self.step = opened.step;
        if url_changed || first {
            self.detail = None;
            let key = self.url.clone().unwrap_or_default();
            if let Some((detail, graph)) = cx
                .try_global::<PullRequestCardFixtures>()
                .and_then(|fixtures| fixtures.0.get(&key).cloned())
            {
                self.fixture = true;
                self.detail = detail;
                self.set_graph(Some(graph));
            }
        }
        if url_changed && !self.fixture {
            self.watch_control = self.url.as_ref().map(|url| {
                cx.new(|cx| {
                    crate::pr_watch::PrWatchControl::new(
                        self.state.clone(),
                        self.chat_id.clone(),
                        url.clone(),
                        self.target(cx),
                        false,
                        cx,
                    )
                })
            });
            self.watch_subscription = self
                .watch_control
                .as_ref()
                .map(|control| cx.observe(control, |card, _, cx| card.changed(cx)));
        }
        self.refresh(false, cx);
        self.changed(cx);
    }

    /// Re-read seeded data after a review fixture changed it.
    pub fn reload_fixture(&mut self, cx: &mut Context<Self>) {
        let key = self.url.clone().unwrap_or_default();
        let Some((detail, graph)) = cx
            .try_global::<PullRequestCardFixtures>()
            .and_then(|fixtures| fixtures.0.get(&key).cloned())
        else {
            return;
        };
        self.fixture = true;
        self.detail = detail;
        self.set_graph(Some(graph));
        self.changed(cx);
    }

    fn chat(&self, cx: &App) -> Option<zuse_protocol::Chat> {
        let state = self.state.read(cx);
        state
            .chats
            .iter()
            .find(|chat| chat.id == self.chat_id)
            .cloned()
    }

    /// The chat's device when it isn't this one.
    fn target(&self, cx: &App) -> Option<String> {
        let state = self.state.read(cx);
        let chat = state.chats.iter().find(|chat| chat.id == self.chat_id)?;
        (state.local_device_id.as_deref() != Some(chat.device_id.as_str()))
            .then(|| chat.device_id.clone())
    }

    fn agent_working(&self, cx: &App) -> bool {
        self.state
            .read(cx)
            .session_for(&self.chat_id)
            .is_some_and(|session| session.status == SessionStatus::Working)
    }

    fn stage(&self) -> Stage {
        stage(
            self.step,
            self.detail.as_ref(),
            self.graph.as_ref(),
            self.resolving,
        )
    }

    fn poll_interval(&mut self, cx: &mut Context<Self>) -> Option<Duration> {
        if self.fixture {
            return None;
        }
        if self.resolving && !self.agent_working(cx) {
            // The agent's turn ended: read where that left the PR.
            self.resolving = false;
            return Some(Duration::from_millis(500));
        }
        let stage = self.stage();
        Some(if stage.busy() || self.agent_working(cx) {
            POLL_BUSY
        } else if matches!(stage, Stage::Merged | Stage::Closed) {
            Duration::from_secs(10 * 60)
        } else {
            POLL_IDLE
        })
    }

    fn set_graph(&mut self, graph: Option<BranchGraph>) {
        // New commits enter top to bottom; after the first load they also
        // flash, so the base moving on reads as something that happened.
        if let Some(graph) = &graph {
            let fresh = !self.appeared.is_empty();
            let now = Instant::now();
            let mut order = 0u32;
            for commit in graph.commits.iter().take(VISIBLE_COMMITS * 3) {
                self.appeared.entry(commit.sha.clone()).or_insert_with(|| {
                    order += 1;
                    (now + ROW_STAGGER * (order - 1).min(12), fresh)
                });
            }
        }
        self.layout = graph
            .as_ref()
            .map(|graph| layout_graph(&graph.commits, graph.head_sha.as_deref()))
            .unwrap_or_default();
        self.graph = graph;
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        if let Some(url) = &self.url {
            let stage = self.stage();
            cx.default_global::<PullRequestStages>()
                .0
                .insert(url.clone(), stage);
            cx.refresh_windows();
        }
        cx.emit(CardLayoutChanged);
        cx.notify();
    }

    /// Read the PR and the branch graph from the chat's device. `refresh`
    /// asks GitHub and the remote for their latest.
    fn refresh(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.fixture {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let target = self.target(cx);
        let cwd = self.chat(cx).and_then(|chat| {
            chat.cwd
                .or_else(|| chat.source_context.map(|source| source.cwd))
        });
        let url = self.url.clone();
        let base = self
            .detail
            .as_ref()
            .map(|detail| detail.base_ref_name.clone())
            .filter(|base| !base.is_empty());
        self.fetch = Some(cx.spawn(async move |this, cx| {
            let with_target = |mut params: serde_json::Value| {
                if let Some(target) = &target {
                    params["targetDeviceId"] = target.clone().into();
                }
                params
            };
            let detail = match &url {
                Some(url) => engine
                    .client()
                    .call(
                        methods::GET_CHANGE_REQUEST,
                        with_target(serde_json::json!({ "url": url, "refresh": refresh })),
                    )
                    .await
                    .ok()
                    .and_then(|value| serde_json::from_value::<ChangeRequestDetail>(value).ok()),
                None => None,
            };
            let base = detail
                .as_ref()
                .map(|detail| detail.base_ref_name.clone())
                .filter(|base| !base.is_empty())
                .or(base)
                .unwrap_or_else(|| "main".into());
            let graph = match cwd {
                Some(cwd) => {
                    let request = BranchGraphRequest {
                        cwd,
                        base,
                        fetch: refresh,
                        limit: Some(24),
                    };
                    engine
                        .client()
                        .call(
                            methods::GET_BRANCH_GRAPH,
                            with_target(serde_json::to_value(request).unwrap_or_default()),
                        )
                        .await
                        .ok()
                        .and_then(|value| serde_json::from_value::<BranchGraph>(value).ok())
                }
                None => None,
            };
            let _ = this.update(cx, |card, cx| {
                if card.url != url {
                    return;
                }
                if detail.is_some() {
                    card.detail = detail;
                }
                if graph.is_some() {
                    card.set_graph(graph);
                }
                card.changed(cx);
            });
        }));
    }

    fn resolve(&mut self, repair: PullRequestRepair, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(detail), Some(url)) = (&self.detail, &self.url) else {
            return;
        };
        let prompt = repair_prompt(repair, detail, url);
        self.resolving = repair == PullRequestRepair::Conflicts;
        self.conflicts_open = false;
        window.dispatch_action(Box::new(SendToChat(prompt)), cx);
        self.changed(cx);
    }

    fn update_pull_request(&mut self, update: Update, cx: &mut Context<Self>) {
        let Some(url) = self.url.clone() else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let mut params = serde_json::json!({ "url": url, "update": update });
        if let Some(target) = self.target(cx) {
            params["targetDeviceId"] = target.into();
        }
        self.merge_menu = false;
        self.merging = true;
        self.notice = None;
        self.update = Some(cx.spawn(async move |this, cx| {
            let notice = match engine
                .client()
                .call(methods::UPDATE_CHANGE_REQUEST, params)
                .await
            {
                Ok(value) => value["refusal"].as_str().map(str::to_owned),
                Err(error) => Some(format!(
                    "Couldn’t update the pull request. {}",
                    failure_reason(&error)
                )),
            };
            let _ = this.update(cx, |card, cx| {
                card.merging = false;
                card.notice = notice.map(SharedString::from);
                card.refresh(true, cx);
                card.changed(cx);
            });
        }));
        cx.notify();
    }

    fn open(&self, window: &mut Window, cx: &mut App) {
        if let Some(url) = &self.url {
            crate::pull_request_detail::open_on_device(url, self.target(cx), window, cx);
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Lane colors: the branch in the accent, the base muted, branches merged
/// into the base green.
fn lane_colors(graph: &BranchGraph, layout: &GraphLayout, theme: &Theme) -> HashMap<usize, Hsla> {
    let head: HashSet<&str> = graph.head_shas.iter().map(String::as_str).collect();
    // Merged branches read green, or merged purple when the accent is green.
    let side = if (theme.accent.h - theme.success.h).abs() < 0.08 {
        ChangeRequestBadgeTone::Merged.color(theme)
    } else {
        theme.success
    };
    let mut colors = HashMap::new();
    let base_color = layout
        .rows
        .iter()
        .find(|row| !head.contains(row.sha.as_str()))
        .map(|row| row.node_color_id);
    for row in &layout.rows {
        let color = if head.contains(row.sha.as_str()) {
            theme.accent
        } else if Some(row.node_color_id) == base_color {
            theme.text_muted.opacity(0.7)
        } else {
            side
        };
        colors.entry(row.node_color_id).or_insert(color);
    }
    colors
}

fn gutter_width(lanes: usize) -> f32 {
    GUTTER_PAD * 2.0 + lanes.saturating_sub(1) as f32 * LANE_SPACING + NODE_RADIUS * 2.0
}

fn lane_x(lane: usize) -> f32 {
    GUTTER_PAD + NODE_RADIUS + lane as f32 * LANE_SPACING
}

fn chip(text: SharedString, color: Hsla, theme: &Theme) -> AnyElement {
    div()
        .flex_none()
        .px(px(5.0))
        .h(px(16.0))
        .flex()
        .items_center()
        .rounded(px(4.0))
        .bg(color.opacity(0.12))
        .text_color(color)
        .text_size(px(10.5))
        .font_family(theme.font_mono.clone())
        .child(text)
        .into_any_element()
}

fn spinner(id: &'static str, theme: &Theme) -> AnyElement {
    crate::loaders::ring_spinner(
        id,
        12.0,
        1.5,
        theme.text_muted.opacity(0.25),
        theme.text_muted,
    )
}

fn button(
    id: &'static str,
    label: SharedString,
    glyph: Option<&'static str>,
    tone: Hsla,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .flex_none()
        .h(px(24.0))
        .px(px(9.0))
        .flex()
        .items_center()
        .gap(px(5.0))
        .rounded(px(7.0))
        .bg(tone.opacity(0.1))
        .text_color(tone)
        .text_size(px(12.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(move |style| style.bg(tone.opacity(0.18)))
        .when_some(glyph, |el, glyph| {
            el.child(icon(glyph).size(px(13.0)).text_color(tone))
        })
        .child(label)
}

impl PullRequestCard {
    fn render_header(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let detail = self.detail.as_ref();
        let title: SharedString = detail
            .map(|detail| detail.title.replace(['\r', '\n'], " "))
            .or_else(|| self.chat(cx).and_then(|chat| chat.title))
            .unwrap_or_else(|| "Pull request".into())
            .into();
        let (label, tone) = match detail.map(|detail| (detail.state.as_str(), detail.is_draft)) {
            None if self.step.is_some() => ("Opening", theme.text_muted),
            None => ("", theme.text_muted),
            Some(("MERGED", _)) => ("Merged", ChangeRequestBadgeTone::Merged.color(theme)),
            Some(("CLOSED", _)) => ("Closed", theme.danger),
            Some((_, true)) => ("Draft", theme.text_muted),
            Some(_) => ("Open", theme.success),
        };
        div()
            .id("pr-card-header")
            .debug_selector(|| "pr-card-header".into())
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(14.0))
            .pt(px(12.0))
            .pb(px(6.0))
            .when(self.url.is_some(), |el| {
                el.cursor_pointer()
                    .on_click(cx.listener(|card, _, window, cx| card.open(window, cx)))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::text_sm())
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.text_strong)
                            .child(title),
                    )
                    .when_some(detail.map(|detail| detail.number), |el, number| {
                        el.child(
                            div()
                                .flex_none()
                                .text_size(crate::typography::text_sm())
                                .text_color(theme.text_muted)
                                .child(SharedString::from(format!("#{number}"))),
                        )
                    }),
            )
            .when(!label.is_empty(), |el| {
                el.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .text_size(px(12.0))
                        .text_color(tone)
                        .child(div().size(px(6.0)).rounded_full().bg(tone))
                        .child(label),
                )
            })
            .into_any_element()
    }

    /// The top row: what merging into the base would do, or the work not
    /// committed yet while the PR is being opened.
    /// How far the current stage's words have entered.
    fn stage_entrance(&self) -> f32 {
        self.stage_since.as_ref().map_or(1.0, |(_, since)| {
            entrance(*since, STAGE_ENTER, Instant::now())
        })
    }

    fn render_merge_row(&self, stage: &Stage, base: &str, theme: &Theme) -> AnyElement {
        let gutter = gutter_width(self.layout.max_lane_count.max(1));
        let uncommitted = self.graph.as_ref().map_or(0, |graph| graph.uncommitted);
        let (label, word, tone, glyph): (SharedString, SharedString, Hsla, Option<&str>) =
            if matches!(stage, Stage::Creating(_)) && uncommitted > 0 {
                (
                    "Uncommitted changes".into(),
                    format!("{uncommitted} files").into(),
                    theme.text_muted,
                    None,
                )
            } else if matches!(stage, Stage::Merged) {
                (
                    format!("Merged into {base}").into(),
                    stage.merge_word(),
                    stage.tone(theme),
                    Some(icons::GIT_MERGE),
                )
            } else {
                let glyph = match stage {
                    Stage::Conflicts(_) => Some(icons::DANGER_TRIANGLE),
                    Stage::Ready => Some(icons::CHECK_CIRCLE),
                    _ => None,
                };
                (
                    format!("Merge into {base}").into(),
                    stage.merge_word(),
                    stage.tone(theme),
                    glyph,
                )
            };
        let marker = match glyph {
            Some(glyph) => icon(glyph)
                .size(px(13.0))
                .text_color(tone)
                .into_any_element(),
            None if stage.busy() => spinner("pr-card-merge-busy", theme),
            None => div()
                .size(px(8.0))
                .rounded_full()
                .border_1()
                .border_color(tone)
                .into_any_element(),
        };
        div()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .child(
                div()
                    .w(px(gutter))
                    .flex_none()
                    .flex()
                    .justify_start()
                    .pl(px(lane_x(0) - 6.5))
                    .child(div().w(px(13.0)).flex().justify_center().child(marker)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.5))
                    .text_color(theme.text_muted)
                    .child(label),
            )
            .child(
                div()
                    .flex_none()
                    .pr(px(14.0))
                    .relative()
                    .opacity(self.stage_entrance())
                    .left(px(6.0 * (1.0 - self.stage_entrance())))
                    .text_size(px(12.0))
                    .text_color(tone)
                    .child(word),
            )
            .into_any_element()
    }

    fn render_commits(&self, theme: &Theme, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(graph) = &self.graph else {
            return Vec::new();
        };
        let colors = lane_colors(graph, &self.layout, theme);
        let head: HashSet<&str> = graph.head_shas.iter().map(String::as_str).collect();
        let lanes = self.layout.max_lane_count.max(1);
        let gutter = gutter_width(lanes);
        let now = Utc::now();
        let instant = Instant::now();
        let shown = if self.show_all {
            graph.commits.len()
        } else {
            graph.commits.len().min(VISIBLE_COMMITS)
        };
        // A branch label rides the newest commit of each lane.
        let mut labelled: HashSet<usize> = HashSet::new();
        let mut rows = Vec::with_capacity(shown + 1);
        for (ix, (commit, layout)) in graph
            .commits
            .iter()
            .zip(&self.layout.rows)
            .take(shown)
            .enumerate()
        {
            let on_head = head.contains(commit.sha.as_str());
            let color = colors
                .get(&layout.node_color_id)
                .copied()
                .unwrap_or(theme.text_muted);
            let segments: Vec<(f32, f32, Hsla, SegmentShape)> = layout
                .segments
                .iter()
                .map(|segment| {
                    (
                        lane_x(segment.from_lane),
                        lane_x(segment.to_lane),
                        colors
                            .get(&segment.color_id)
                            .copied()
                            .unwrap_or(theme.text_muted),
                        segment.shape.clone(),
                    )
                })
                .collect();
            let node_x = lane_x(layout.node_lane);
            let last = ix + 1 == shown && shown == graph.commits.len();
            let graph_cell = canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let top = bounds.origin.y;
                    let mid = top + bounds.size.height / 2.0;
                    let bottom = top + bounds.size.height;
                    let x0 = bounds.origin.x;
                    for (from, to, color, shape) in &segments {
                        let mut path = PathBuilder::stroke(px(STROKE));
                        let (start, end) = match shape {
                            SegmentShape::Through => {
                                (point(x0 + px(*from), top), point(x0 + px(*to), bottom))
                            }
                            SegmentShape::Incoming => {
                                (point(x0 + px(*from), top), point(x0 + px(*to), mid))
                            }
                            SegmentShape::Outgoing => {
                                if last {
                                    continue;
                                }
                                (point(x0 + px(*from), mid), point(x0 + px(*to), bottom))
                            }
                        };
                        path.move_to(start);
                        if start.x == end.x {
                            path.line_to(end);
                        } else {
                            let bend = (end.y - start.y) * 0.6;
                            path.cubic_bezier_to(
                                end,
                                point(start.x, start.y + bend),
                                point(end.x, end.y - bend),
                            );
                        }
                        if let Ok(path) = path.build() {
                            window.paint_path(path, *color);
                        }
                    }
                },
            )
            .absolute()
            .inset_0();
            let label = if labelled.insert(layout.node_lane) {
                commit
                    .refs
                    .iter()
                    .find(|reference| reference.kind != zuse_protocol::GitHistoryRefKind::Tag)
                    .map(|reference| reference.label.clone())
                    .or_else(|| (ix == 0 && on_head).then(|| graph.head_branch.clone()))
            } else {
                None
            };
            let author: SharedString = if graph
                .self_email
                .as_deref()
                .is_some_and(|email| email.eq_ignore_ascii_case(&commit.author_email))
            {
                "You".into()
            } else {
                commit
                    .author_name
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .to_owned()
                    .into()
            };
            let age = short_age(&commit.authored_at, now);
            let is_fork = graph.fork_sha.as_deref() == Some(commit.sha.as_str());
            let (enter, tint) = self
                .appeared
                .get(&commit.sha)
                .map(|(start, fresh)| {
                    let enter = entrance(*start, ROW_ENTER, instant);
                    let tint = fresh
                        .then(|| 1.0 - entrance(*start, FRESH_TINT, instant))
                        .unwrap_or(0.0);
                    (enter, tint)
                })
                .unwrap_or((1.0, 0.0));
            rows.push(
                div()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .relative()
                    .opacity(enter)
                    .top(px(-6.0 * (1.0 - enter)))
                    .when(tint > 0.0, |row| row.bg(theme.warning.opacity(0.14 * tint)))
                    .child(
                        div()
                            .w(px(gutter))
                            .h_full()
                            .flex_none()
                            .relative()
                            .child(graph_cell)
                            .child(
                                div()
                                    .absolute()
                                    .left(px(node_x - NODE_RADIUS))
                                    .top(px(ROW_HEIGHT / 2.0 - NODE_RADIUS))
                                    .size(px(NODE_RADIUS * 2.0))
                                    .rounded_full()
                                    .when(commit.parent_shas.len() > 1, |el| {
                                        el.border(px(1.5))
                                            .border_color(color)
                                            .bg(theme.surface_card)
                                    })
                                    .when(commit.parent_shas.len() <= 1, |el| el.bg(color)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .text_color(if on_head {
                                        theme.text
                                    } else {
                                        theme.text_muted
                                    })
                                    .child(SharedString::from(commit.subject.clone())),
                            )
                            .when_some(label, |el, label| {
                                el.child(chip(label.into(), color, theme))
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .pl(px(10.0))
                            .pr(px(14.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .text_size(px(11.5))
                            .text_color(theme.text_muted.opacity(0.8))
                            .child(div().font_family(theme.font_mono.clone()).child(
                                SharedString::from(commit.sha.chars().take(7).collect::<String>()),
                            ))
                            .child(SharedString::from(if is_fork || age.is_empty() {
                                author.to_string()
                            } else {
                                format!("{author} · {age}")
                            })),
                    )
                    .into_any_element(),
            );
        }
        if graph.commits.len() > VISIBLE_COMMITS {
            let hidden = graph.commits.len() - VISIBLE_COMMITS;
            let label: SharedString = if self.show_all {
                "Show fewer".into()
            } else {
                format!("{hidden} earlier commits").into()
            };
            rows.push(
                div()
                    .id("pr-card-more")
                    .h(px(22.0))
                    .pl(px(gutter))
                    .flex()
                    .items_center()
                    .text_size(px(11.5))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.text))
                    .on_click(cx.listener(|card, _, _, cx| {
                        card.show_all = !card.show_all;
                        card.changed(cx);
                    }))
                    .child(label)
                    .into_any_element(),
            );
        }
        rows
    }

    /// Who changed the conflicting lines on the base, and each region's two
    /// sides.
    fn render_conflicts(&self, base: &str, theme: &Theme) -> AnyElement {
        let merge = self.graph.as_ref().and_then(|graph| graph.merge.as_ref());
        let authors: Vec<&str> = merge
            .map(|merge| {
                let mut seen = Vec::new();
                for author in merge
                    .conflicts
                    .iter()
                    .filter_map(|c| c.base_author.as_deref())
                {
                    if !seen.contains(&author) {
                        seen.push(author);
                    }
                }
                seen
            })
            .unwrap_or_default();
        let who = match authors.as_slice() {
            [] => "Someone".to_owned(),
            [one] => (*one).to_owned(),
            [one, two] => format!("{one} and {two}"),
            [one, rest @ ..] => format!("{one} and {} others", rest.len()),
        };
        let first_name = |name: &str| name.split_whitespace().next().unwrap_or(name).to_owned();
        let mut card = popover::popover_card(theme)
            .w(px(420.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme.text_muted)
                    .child(
                        div()
                            .flex()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_color(theme.text_strong)
                                    .child(SharedString::from(who)),
                            )
                            .child(SharedString::from(format!(
                                "changed the same lines on {base}"
                            ))),
                    ),
            );
        let Some(merge) = merge else {
            return card
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.text_muted)
                        .child("GitHub reports conflicts with the base branch."),
                )
                .into_any_element();
        };
        for conflict in &merge.conflicts {
            let theirs = conflict
                .base_author
                .as_deref()
                .map(first_name)
                .unwrap_or_else(|| base.to_owned());
            let mut file = div().flex().flex_col().gap(px(4.0)).child(
                div()
                    .flex()
                    .gap(px(6.0))
                    .text_size(px(12.0))
                    .child(
                        div()
                            .text_color(theme.text_strong)
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(SharedString::from(conflict.path.clone())),
                    )
                    .when_some(conflict.hunks.first(), |el, hunk| {
                        el.child(
                            div()
                                .text_color(theme.text_muted)
                                .child(SharedString::from(format!("line {}", hunk.line))),
                        )
                    }),
            );
            if let Some(note) = &conflict.note {
                file = file.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(note.clone())),
                );
            }
            for hunk in conflict.hunks.iter().take(2) {
                let side = |who: String, text: &str, color: Hsla| {
                    div()
                        .flex()
                        .gap(px(10.0))
                        .pl(px(8.0))
                        .border_l_2()
                        .border_color(color)
                        .text_size(px(11.5))
                        .font_family(theme.font_mono.clone())
                        .child(
                            div()
                                .w(px(44.0))
                                .flex_none()
                                .truncate()
                                .text_color(theme.text_muted)
                                .child(SharedString::from(who)),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text)
                                .child(SharedString::from(text.to_owned())),
                        )
                };
                file = file.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(side(theirs.clone(), &hunk.base, theme.warning))
                        .child(side("You".into(), &hunk.head, theme.accent)),
                );
            }
            card = card.child(file);
        }
        if merge.more_files > 0 {
            card = card.child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(format!(
                        "{} more conflicting files",
                        merge.more_files
                    ))),
            );
        }
        card.into_any_element()
    }

    fn render_checks(&self, theme: &Theme) -> Option<AnyElement> {
        let detail = self.detail.as_ref()?;
        if detail.status_check_rollup.is_empty() {
            return None;
        }
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x(px(14.0))
            .gap_y(px(4.0))
            .px(px(14.0))
            .pt(px(6.0))
            .text_size(px(12.0));
        for check in detail.status_check_rollup.iter().take(6) {
            let status = check_status(check);
            let (glyph, tone, word) = if check_failed(&status) {
                (icons::CLOSE_CIRCLE, theme.danger, "failed")
            } else if pending(&status) {
                (icons::CLOCK_CIRCLE, theme.text_muted, "running")
            } else if matches!(status.as_str(), "SKIPPED" | "NEUTRAL") {
                (icons::CHECK, theme.text_muted, "skipped")
            } else {
                (icons::CHECK, theme.success, "passed")
            };
            let name = if check.name.is_empty() {
                check.context.clone()
            } else {
                check.name.clone()
            };
            row = row.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .child(icon(glyph).size(px(12.0)).text_color(tone))
                    .child(div().text_color(theme.text).child(SharedString::from(name)))
                    .child(div().text_color(theme.text_muted).child(word)),
            );
        }
        if detail.status_check_rollup.len() > 6 {
            row = row.child(
                div()
                    .text_color(theme.text_muted)
                    .child(SharedString::from(format!(
                        "+{} more",
                        detail.status_check_rollup.len() - 6
                    ))),
            );
        }
        Some(row.into_any_element())
    }

    fn render_footer(
        &self,
        stage: &Stage,
        base: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let status = |glyph: Option<AnyElement>, text: SharedString, tone: Hsla| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(7.0))
                .text_size(px(12.5))
                .text_color(tone)
                .children(glyph)
                .child(div().min_w_0().truncate().child(text))
        };
        let warn = |glyph: &'static str, tone: Hsla| {
            Some(
                icon(glyph)
                    .size(px(13.0))
                    .text_color(tone)
                    .into_any_element(),
            )
        };
        let (left, right): (AnyElement, Option<AnyElement>) = match stage {
            Stage::Creating(step) => (
                status(
                    Some(spinner("pr-card-step", theme)),
                    step.label().into(),
                    theme.text_muted,
                )
                .into_any_element(),
                None,
            ),
            Stage::Loading | Stage::Checking => (
                status(
                    Some(spinner("pr-card-checking", theme)),
                    "Checking it merges cleanly".into(),
                    theme.text_muted,
                )
                .into_any_element(),
                None,
            ),
            Stage::Conflicts(n) => {
                let open = self.conflicts_open;
                let trigger = div()
                    .id("pr-card-conflicts")
                    .debug_selector(|| "pr-card-conflicts".into())
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .text_size(px(12.5))
                    .text_color(theme.text_muted)
                    .cursor_default()
                    .on_hover(cx.listener(|card, hovered: &bool, _, cx| {
                        if card.conflicts_open != *hovered {
                            card.conflicts_open = *hovered;
                            cx.notify();
                        }
                    }))
                    .child(
                        icon(icons::DANGER_TRIANGLE)
                            .size(px(13.0))
                            .text_color(theme.warning),
                    )
                    .child(
                        div()
                            .text_color(theme.warning)
                            .border_b_1()
                            .border_color(theme.warning.opacity(0.4))
                            .child(SharedString::from(if *n == 1 {
                                "1 conflict".to_owned()
                            } else {
                                format!("{n} conflicts")
                            })),
                    )
                    .child(SharedString::from(format!("with {base}")))
                    .when(open, |el| {
                        el.child(popover::anchored_menu_above(
                            "pr-card-conflicts-popover",
                            self.render_conflicts(base, theme),
                            None,
                        ))
                    });
                (
                    div().flex_1().min_w_0().child(trigger).into_any_element(),
                    Some(
                        button(
                            "pr-card-resolve",
                            "Resolve conflicts".into(),
                            None,
                            theme.accent,
                        )
                        .on_click(cx.listener(|card, _, window, cx| {
                            card.resolve(PullRequestRepair::Conflicts, window, cx)
                        }))
                        .into_any_element(),
                    ),
                )
            }
            Stage::Resolving => (
                status(
                    Some(spinner("pr-card-resolving", theme)),
                    format!("Merging {base} into the branch and resolving conflicts").into(),
                    theme.text_muted,
                )
                .into_any_element(),
                None,
            ),
            Stage::ChecksRunning { done, total } => (
                status(
                    Some(spinner("pr-card-checks", theme)),
                    format!("Running checks · {done} of {total} done").into(),
                    theme.text_muted,
                )
                .into_any_element(),
                None,
            ),
            Stage::ChecksFailing(n) => (
                status(
                    warn(icons::CLOSE_CIRCLE, theme.danger),
                    if *n == 1 {
                        "1 check failing".into()
                    } else {
                        format!("{n} checks failing").into()
                    },
                    theme.danger,
                )
                .into_any_element(),
                Some(
                    button("pr-card-fix", "Fix checks".into(), None, theme.accent)
                        .on_click(cx.listener(|card, _, window, cx| {
                            card.resolve(PullRequestRepair::Checks, window, cx)
                        }))
                        .into_any_element(),
                ),
            ),
            Stage::Draft => (
                status(None, "Draft".into(), theme.text_muted).into_any_element(),
                Some(
                    button("pr-card-ready", "Mark ready".into(), None, theme.text)
                        .on_click(cx.listener(|card, _, _, cx| {
                            card.update_pull_request(Update::MarkReady, cx)
                        }))
                        .into_any_element(),
                ),
            ),
            Stage::Blocked(why) => (
                status(
                    warn(icons::INFO_CIRCLE, theme.warning),
                    capitalize(why).into(),
                    theme.text_muted,
                )
                .into_any_element(),
                None,
            ),
            Stage::Ready => {
                let menu = self.merge_menu.then(|| {
                    let item = |id: &'static str, label: &'static str, method: Method| {
                        popover::menu_row(theme, false, id)
                            .id(id)
                            .debug_selector(move || id.to_owned())
                            .text_size(crate::typography::text_xs())
                            .child(
                                icon(icons::GIT_MERGE)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            )
                            .child(div().flex_1().child(label))
                            .on_click(cx.listener(move |card, _, _, cx| {
                                card.update_pull_request(Update::Merge { method }, cx)
                            }))
                    };
                    popover::anchored_menu_below_end(
                        "pr-card-merge-menu",
                        popover::popover_card(theme)
                            .w(px(210.0))
                            .flex()
                            .flex_col()
                            .on_mouse_down_out(cx.listener(|card, _, _, cx| {
                                card.merge_menu = false;
                                cx.notify();
                            }))
                            .child(item(
                                "pr-card-merge-squash",
                                "Squash and merge",
                                Method::Squash,
                            ))
                            .child(item(
                                "pr-card-merge-commit",
                                "Create a merge commit",
                                Method::Merge,
                            ))
                            .child(item(
                                "pr-card-merge-rebase",
                                "Rebase and merge",
                                Method::Rebase,
                            ))
                            .into_any_element(),
                        None,
                    )
                });
                (
                    status(
                        warn(icons::CHECK, theme.success),
                        "Ready to merge".into(),
                        theme.text,
                    )
                    .into_any_element(),
                    Some(if self.merging {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .text_size(px(12.0))
                            .text_color(theme.text_muted)
                            .child(spinner("pr-card-merging", theme))
                            .child("Merging")
                            .into_any_element()
                    } else {
                        div()
                            .relative()
                            .child(
                                button(
                                    "pr-card-merge",
                                    "Merge".into(),
                                    Some(icons::GIT_MERGE),
                                    theme.accent,
                                )
                                .on_click(cx.listener(
                                    |card, _, _, cx| {
                                        card.merge_menu = !card.merge_menu;
                                        cx.notify();
                                    },
                                )),
                            )
                            .children(menu)
                            .into_any_element()
                    }),
                )
            }
            Stage::Merged => (
                status(
                    warn(
                        icons::GIT_MERGE,
                        ChangeRequestBadgeTone::Merged.color(theme),
                    ),
                    format!("Merged into {base}").into(),
                    theme.text,
                )
                .into_any_element(),
                self.url.clone().map(|url| {
                    button(
                        "pr-card-github",
                        "GitHub".into(),
                        Some(icons::SITE_GITHUB),
                        theme.text_muted,
                    )
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .into_any_element()
                }),
            ),
            Stage::Closed => (
                status(
                    warn(icons::PULL_REQUEST_CLOSED, theme.danger),
                    "Closed without merging".into(),
                    theme.text_muted,
                )
                .into_any_element(),
                None,
            ),
        };
        div()
            .mt(px(6.0))
            .mx(px(8.0))
            .mb(px(8.0))
            .px(px(6.0))
            .h(px(36.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .border_t_1()
            .border_color(theme.border.opacity(0.6))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .relative()
                    .opacity(self.stage_entrance())
                    .top(px(4.0 * (1.0 - self.stage_entrance())))
                    .child(left)
                    .children(right),
            )
            .into_any_element()
    }
}

impl gpui::Render for PullRequestCard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let stage = self.stage();
        let now = Instant::now();
        if crate::motion::reduced_motion(cx) {
            self.stage_since = Some((stage.clone(), now - STAGE_ENTER * 4));
            for (start, _) in self.appeared.values_mut() {
                *start = (*start).min(now - FRESH_TINT * 2);
            }
        } else if self
            .stage_since
            .as_ref()
            .is_none_or(|(seen, _)| *seen != stage)
        {
            // The first reading is there already; later ones arrive.
            let since = if self.stage_since.is_none() {
                now - STAGE_ENTER * 4
            } else {
                now
            };
            self.stage_since = Some((stage.clone(), since));
        }
        let settle = |start: Instant, duration: Duration| start + duration * 2 > now;
        let moving =
            self.stage_since
                .as_ref()
                .is_some_and(|(_, since)| settle(*since, STAGE_ENTER))
                || self.appeared.values().any(|(start, fresh)| {
                    settle(*start, if *fresh { FRESH_TINT } else { ROW_ENTER })
                });
        if moving {
            window.request_animation_frame();
        }
        let base = self
            .detail
            .as_ref()
            .map(|detail| detail.base_ref_name.clone())
            .filter(|base| !base.is_empty())
            .or_else(|| {
                self.graph
                    .as_ref()
                    .map(|graph| {
                        graph
                            .base_ref
                            .rsplit('/')
                            .next()
                            .unwrap_or_default()
                            .to_owned()
                    })
                    .filter(|base| !base.is_empty())
            })
            .unwrap_or_else(|| "main".into());
        let header = self.render_header(&theme, cx);
        let merge_row = self.render_merge_row(&stage, &base, &theme);
        let commits = self.render_commits(&theme, cx);
        let checks = matches!(
            stage,
            Stage::Ready | Stage::ChecksRunning { .. } | Stage::ChecksFailing(_)
        )
        .then(|| self.render_checks(&theme))
        .flatten();
        let footer = self.render_footer(&stage, &base, &theme, cx);
        div().w_full().py(px(4.0)).child(
            div()
                .debug_selector(|| "pr-card".into())
                .w_full()
                .flex()
                .flex_col()
                .rounded(px(12.0))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface_card)
                .child(header)
                .children(
                    self.watch_control
                        .clone()
                        .map(|control| div().px(px(14.0)).pb(px(6.0)).child(control)),
                )
                .child(merge_row)
                .children(commits)
                .children(checks)
                .when_some(self.notice.clone(), |el, notice| {
                    el.child(
                        div()
                            .px(px(14.0))
                            .pt(px(6.0))
                            .text_size(px(11.5))
                            .text_color(theme.danger)
                            .child(notice),
                    )
                })
                .child(footer),
        )
    }
}

// ---------------------------------------------------------------------------
// The composer's chips
// ---------------------------------------------------------------------------

/// The selected chat's pull request and diff size as chips above the
/// composer: "#128 · 3 conflicts" opens the PR, "+482 −37" its size. A Git
/// chat with changes and no PR offers Create PR instead. `None` when the
/// chat has neither.
pub fn composer_chips(state: &AppState, theme: &Theme, cx: &App) -> Option<AnyElement> {
    let chat = state.selected_chat_row()?;
    let summary = state.change_request_for_chat(chat).cloned();
    let diff = state.diff_stat_for_chat(chat);
    if summary.is_none() && diff.is_none() {
        return None;
    }
    let pill = |id: &'static str| {
        div()
            .id(id)
            .debug_selector(move || id.to_owned())
            .flex_none()
            .h(px(24.0))
            .px(px(9.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded_full()
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_card)
            .text_size(px(12.0))
    };
    let mut row = div().flex().items_center().gap(px(6.0)).px(px(4.0));
    let working = state
        .session_for(&chat.id)
        .is_some_and(|session| session.status == SessionStatus::Working);
    if summary.is_none() && diff.is_some() && chat.source_context.is_some() && !working {
        row = row.child(
            pill("composer-create-pr")
                .cursor_pointer()
                .text_color(theme.text)
                .hover(|style| style.bg(theme.element_hover))
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(SendToChat(CREATE_PR_PROMPT.into())), cx)
                })
                .child(
                    icon(icons::PULL_REQUEST)
                        .size(px(13.0))
                        .text_color(theme.text_muted),
                )
                .child("Create PR"),
        );
    }
    if let Some(summary) = summary {
        let stage = cx
            .try_global::<PullRequestStages>()
            .and_then(|stages| stages.0.get(&summary.url))
            .cloned()
            .unwrap_or(match summary.state {
                zuse_protocol::ChangeRequestState::Merged => Stage::Merged,
                zuse_protocol::ChangeRequestState::Closed => Stage::Closed,
                zuse_protocol::ChangeRequestState::Open => Stage::Loading,
            });
        let (label, tone) = match stage {
            Stage::Loading | Stage::Checking => ("Open".into(), theme.success),
            stage => (stage.chip_label(), stage.tone(theme)),
        };
        let url = summary.url.clone();
        let device = (state.local_device_id.as_deref() != Some(chat.device_id.as_str()))
            .then(|| chat.device_id.clone());
        row = row.child(
            pill("composer-pr-chip")
                .cursor_pointer()
                .hover(|style| style.bg(theme.element_hover))
                .on_click(move |_, window, cx| {
                    crate::pull_request_detail::open_on_device(&url, device.clone(), window, cx)
                })
                .child(div().size(px(6.0)).rounded_full().bg(tone))
                .child(
                    div()
                        .text_color(theme.text)
                        .child(SharedString::from(format!("#{}", summary.number))),
                )
                .child(div().text_color(tone).child(label)),
        );
    }
    if let Some((additions, deletions)) = diff {
        // Five blocks, like GitHub's: green for additions, red for
        // deletions, the rest empty for small changes.
        let total = (additions + deletions).max(1) as f32;
        let filled = ((total.log10() + 1.0).floor() as usize).clamp(1, 5);
        let green = ((additions as f32 / total) * filled as f32).round() as usize;
        let red = filled - green.min(filled);
        let blocks = (0..5).map(|ix| {
            let color = if ix < green {
                theme.success
            } else if ix < green + red {
                theme.danger
            } else {
                theme.text_muted.opacity(0.25)
            };
            div().size(px(6.0)).rounded(px(1.5)).bg(color)
        });
        row = row.child(
            pill("composer-diff-chip")
                .child(
                    div()
                        .text_color(theme.success)
                        .child(SharedString::from(format!("+{additions}"))),
                )
                .child(
                    div()
                        .text_color(theme.danger)
                        .child(SharedString::from(format!("\u{2212}{deletions}"))),
                )
                .child(div().flex().gap(px(2.0)).children(blocks)),
        );
    }
    Some(row.into_any_element())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zuse_protocol::ChangeRequestCheck;

    fn exec(command: &str) -> ToolCall {
        ToolCall::Exec {
            command: command.into(),
        }
    }

    #[test]
    fn finds_the_url_gh_printed() {
        assert_eq!(
            pull_request_url("Creating pull request...\nhttps://github.com/acme/web/pull/128\n"),
            Some("https://github.com/acme/web/pull/128".into())
        );
        assert_eq!(
            pull_request_url("see https://github.com/acme/web/pull/7/files."),
            Some("https://github.com/acme/web/pull/7".into())
        );
        assert_eq!(pull_request_url("https://github.com/acme/web/pulls"), None);
        assert_eq!(pull_request_url("no link"), None);
    }

    #[test]
    fn a_turn_opens_a_pull_request_only_through_gh_pr_create() {
        let commit = exec("git add -A && git commit -m 'x'");
        let create = exec("gh pr create --fill");
        let out = "https://github.com/acme/web/pull/128";
        // Live: the step running now.
        assert_eq!(
            opened_pull_request([(&commit, None, false, false)], true),
            None
        );
        assert_eq!(
            opened_pull_request(
                [(&commit, None, true, false), (&create, None, false, false)],
                true
            ),
            Some(OpenedPullRequest {
                url: None,
                step: Some(CreateStep::Opening)
            })
        );
        assert_eq!(
            opened_pull_request([(&create, Some(out), true, false)], false),
            Some(OpenedPullRequest {
                url: Some(out.into()),
                step: None
            })
        );
        // A finished turn whose create failed leaves no card.
        assert_eq!(
            opened_pull_request([(&create, Some("already exists"), true, true)], false),
            None
        );
    }

    fn detail(state: &str, mergeable: &str, checks: &[&str]) -> ChangeRequestDetail {
        ChangeRequestDetail {
            state: state.into(),
            mergeable: mergeable.into(),
            status_check_rollup: checks
                .iter()
                .map(|status| ChangeRequestCheck {
                    name: "Tests".into(),
                    conclusion: (*status).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn stage_reads_the_most_pressing_state_first() {
        let conflicted = BranchGraph {
            merge: Some(MergePreview {
                clean: false,
                conflicts: vec![Default::default(), Default::default()],
                more_files: 1,
            }),
            ..Default::default()
        };
        assert_eq!(stage(None, None, None, false), Stage::Loading);
        assert_eq!(
            stage(
                None,
                Some(&detail("OPEN", "MERGEABLE", &["FAILURE"])),
                Some(&conflicted),
                false
            ),
            Stage::Conflicts(3)
        );
        assert_eq!(
            stage(
                None,
                Some(&detail("OPEN", "CONFLICTING", &[])),
                Some(&conflicted),
                true
            ),
            Stage::Resolving
        );
        assert_eq!(
            stage(
                None,
                Some(&detail("OPEN", "MERGEABLE", &["SUCCESS", "PENDING"])),
                None,
                false
            ),
            Stage::ChecksRunning { done: 1, total: 2 }
        );
        assert_eq!(
            stage(
                None,
                Some(&detail("OPEN", "MERGEABLE", &["SUCCESS"])),
                None,
                false
            ),
            Stage::Ready
        );
        assert_eq!(
            stage(
                None,
                Some(&detail("MERGED", "UNKNOWN", &[])),
                Some(&conflicted),
                false
            ),
            Stage::Merged
        );
        assert_eq!(
            stage(Some(CreateStep::Pushing), None, None, false),
            Stage::Creating(CreateStep::Pushing)
        );
    }
}
