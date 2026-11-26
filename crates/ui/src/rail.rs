//! Compact conversation navigation on the right edge of the transcript.
//! Marks represent turns, highlight the visible range, and preview each turn
//! on hover. Clicking a mark smoothly scrolls to its prompt.

use gpui::{AnimationExt, AnyElement, Context, ListOffset, SharedString, div, prelude::*, px};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use zuse_doc::{MessagePart, MessageRole, SessionMessageEntry};

use crate::motion;
use crate::popover;
use crate::theme::Theme;
use crate::transcript::Transcript;

/// Keep the rail available in split layouts while avoiding extremely narrow panes.
pub const RAIL_MIN_CONTAINER_WIDTH: f32 = 240.0;

pub fn rail_visible(container_width: f32) -> bool {
    container_width >= RAIL_MIN_CONTAINER_WIDTH
}

/// Aria-label cap for a rail mark (grapheme-unaware; the hover card clamps
/// visually to one title line and two description lines).
pub const PREVIEW_PROMPT_CHARS: usize = 160;

/// One rail tick: a user prompt and the opening of the reply that followed.
#[derive(Debug, Clone, PartialEq)]
pub struct RailTick {
    /// Message id — equals the user row's id in the transcript row model.
    pub message_id: String,
    pub prompt: String,
    pub reply: Option<String>,
}

fn user_text(entry: &SessionMessageEntry) -> String {
    let raw = entry
        .parts
        .iter()
        .filter_map(|part| match part {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    // The hover title is the words the user typed. Context pills, review
    // comments and attachments stay out of that line unless the send has no
    // prompt of its own.
    rail_prompt_label(&raw)
}

/// Title for one rail mark.
///
/// A prompt wins over anything attached to it. A send with no prompt names
/// the annotation, comment and attachment pills instead of the raw markup.
pub fn rail_prompt_label(content: &str) -> String {
    let (text, badges) = crate::badges::split(content);
    let parsed = crate::attachments::parse_user_message_images(&text);
    let prompt = parsed.text.trim();
    if !is_placeholder_prompt(prompt) {
        return prompt.to_string();
    }
    let mut parts: Vec<String> = badges
        .iter()
        .map(|badge| badge.label.trim().to_string())
        .filter(|label| !label.is_empty())
        .collect();
    if !parsed.attachments.is_empty() {
        parts.push(crate::attachments::user_message_rail_text(&text));
    }
    if parts.is_empty() {
        let fallback = prompt;
        if fallback.is_empty() {
            content.trim().to_string()
        } else {
            fallback.to_string()
        }
    } else {
        parts.join(" · ")
    }
}

/// Boilerplate the composer inserts when a send is only pills or files.
fn is_placeholder_prompt(text: &str) -> bool {
    text.is_empty()
        || crate::attachments::is_attachment_only_text(text)
        || text == crate::comments::COMMENT_ONLY_TEXT
}

fn first_reply_text(entries: &[SessionMessageEntry]) -> Option<String> {
    entries
        .iter()
        .take_while(|e| e.role != MessageRole::User)
        .find(|e| e.role == MessageRole::Assistant)
        .and_then(|entry| {
            entry.parts.iter().find_map(|part| match part {
                MessagePart::Text { text, .. } if !text.trim().is_empty() => {
                    Some(text.trim().to_string())
                }
                _ => None,
            })
        })
}

/// Extract rail ticks from the transcript: one per user entry (doc entries
/// first, then unconfirmed echoes — matching transcript row order). Each tick
/// carries the opening of the assistant reply that followed it, for the hover
/// preview card.
pub fn rail_ticks(
    entries: &[SessionMessageEntry],
    echoes: &[SessionMessageEntry],
) -> Vec<RailTick> {
    let mut ticks: Vec<RailTick> = Vec::new();
    for (ix, entry) in entries.iter().enumerate() {
        if entry.role != MessageRole::User {
            continue;
        }
        ticks.push(RailTick {
            message_id: entry.id.clone(),
            prompt: user_text(entry),
            reply: first_reply_text(&entries[ix + 1..]),
        });
    }
    for echo in echoes {
        if echo.role == MessageRole::User && !ticks.iter().any(|t| t.message_id == echo.id) {
            ticks.push(RailTick {
                message_id: echo.id.clone(),
                prompt: user_text(echo),
                reply: None,
            });
        }
    }
    ticks
}

/// Resolve outline targets in one pass over the rows, preserving first matches.
fn position_ticks<'a>(
    ticks: Vec<RailTick>,
    row_ids: impl IntoIterator<Item = (usize, &'a str)>,
) -> Vec<(RailTick, usize)> {
    let row_ids = row_ids.into_iter();
    let mut positions = HashMap::with_capacity(row_ids.size_hint().0);
    for (index, id) in row_ids {
        positions.entry(id).or_insert(index);
    }
    ticks
        .into_iter()
        .filter_map(|tick| {
            let index = *positions.get(tick.message_id.as_str())?;
            Some((tick, index))
        })
        .collect()
}

/// The active tick for a scroll position: the last tick whose transcript row is
/// at or above the viewport-top row (the prompt whose section you're reading).
/// Before the first tick's row, the first tick is active.
pub fn active_tick(tick_rows: &[usize], top_row: usize) -> Option<usize> {
    if tick_rows.is_empty() {
        return None;
    }
    match tick_rows.iter().rposition(|&row| row <= top_row) {
        Some(ix) => Some(ix),
        None => Some(0),
    }
}

// ---------------------------------------------------------------------------
// Fixed-footprint outline (shadcn MessageScroller "Transcript Outline")
// ---------------------------------------------------------------------------

/// Slot size used only to count how many marks fit the viewport. Drawn rows
/// are shorter than this so the stack stays compact.
pub const TICK_SLOT: f32 = 10.0;
pub const TICK_GAP: f32 = 3.0;
/// Vertical breathing room kept clear above/below the tick stack.
pub const RAIL_V_MARGIN: f32 = 24.0;

/// The top-right outline control: its offset under the titlebar and from the
/// transcript's right edge, its width, and one mark's row (a 2px mark with a
/// 4px gap, so the stack stays tight).
const OUTLINE_TOP: f32 = 12.0;
const OUTLINE_RIGHT: f32 = 12.0;
const OUTLINE_WIDTH: f32 = 40.0;
const OUTLINE_TICK_PITCH: f32 = 6.0;
const OUTLINE_MARK: f32 = 2.0;
/// Hover preview: a fixed card, one title line, two description lines.
/// Width sits with the pull-request page menus.
const OUTLINE_LIST_WIDTH: f32 = 232.0;
const OUTLINE_CARD_PAD_X: f32 = 12.0;
const OUTLINE_CARD_PAD_Y: f32 = 10.0;
const OUTLINE_CARD_GAP: f32 = 2.0;
const OUTLINE_TITLE_LH: f32 = 18.0;
const OUTLINE_BODY_LH: f32 = 16.0;
/// Text width inside the card's horizontal padding and 1px border.
const OUTLINE_TEXT_W: f32 = OUTLINE_LIST_WIDTH - 2.0 * (OUTLINE_CARD_PAD_X + 1.0);
const OUTLINE_SIDE_OFFSET: f32 = 8.0;
const OUTLINE_CLOSE_DELAY: Duration = Duration::from_millis(140);

/// Bound the rail for very long conversations.
pub const MAX_RAIL_TICKS: usize = 40;

/// How many tick slots fit in a rail of `height` px (always ≥ 1).
pub fn rail_capacity(height: f32) -> usize {
    let usable = (height - 2.0 * RAIL_V_MARGIN).max(TICK_SLOT);
    (((usable + TICK_GAP) / (TICK_SLOT + TICK_GAP)).floor() as usize).max(1)
}

/// Slots the rail actually uses: what fits the viewport, hard-capped at
/// [`MAX_RAIL_TICKS`] so the outline stays compact even on tall windows.
pub fn rail_slots(height: f32) -> usize {
    rail_capacity(height).min(MAX_RAIL_TICKS)
}

/// Partition turns into contiguous buckets when they exceed available slots.
pub fn tick_buckets(n: usize, capacity: usize) -> Vec<(usize, usize)> {
    if n == 0 {
        return Vec::new();
    }
    let cap = capacity.clamp(1, n);
    (0..cap).map(|k| (k * n / cap, (k + 1) * n / cap)).collect()
}

/// The bucket containing tick `ix` (for active/hover mapping).
pub fn bucket_of(buckets: &[(usize, usize)], ix: usize) -> Option<usize> {
    buckets.iter().position(|&(s, e)| ix >= s && ix < e)
}

/// Char-cap a preview with an ellipsis. Whitespace runs (including newlines —
/// prompts and replies are free text) collapse to single spaces first: the
/// preview card's title is a one-line surface (message-rail.tsx line-clamp-1).
pub fn truncate_preview(text: &str, max_chars: usize) -> String {
    let flat = crate::transcript::single_line(text);
    if flat.chars().count() <= max_chars {
        return flat;
    }
    let cut: String = flat.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

// ---------------------------------------------------------------------------
// Duration-based glide timeline (pure)
// ---------------------------------------------------------------------------

/// Duration-based scroll glide (browser smooth-scroll parity: the Electron
/// rail used `scrollToItem({behavior:"smooth"})` — a fixed-duration gentle
/// ease over the WHOLE distance, never percent-of-remaining).
///
/// Rows above the viewport are unmeasured, so the total pixel distance can
/// only be ESTIMATED per frame. The timeline therefore hands out each frame's
/// movement as a fraction of whatever distance currently remains:
/// `(e_now − e_prev) / (1 − e_prev)` for eased progress `e`. With a stable
/// estimate this telescopes to exactly `start + e(t)·total` — the fixed eased
/// timeline. When the estimate changes mid-flight (a row got measured, the
/// bottom-aligned layout re-glued an anchor), the SAME timeline simply
/// continues over the corrected remainder — no restart, no compensating jump.
#[derive(Debug, Clone)]
pub struct GlideTimeline {
    eased_prev: f32,
}

impl Default for GlideTimeline {
    fn default() -> Self {
        Self::new()
    }
}

impl GlideTimeline {
    pub fn new() -> Self {
        Self { eased_prev: 0.0 }
    }

    /// Fraction of the CURRENT remaining distance to consume for eased
    /// progress `eased` (monotone, `0..=1`; `1.0` lands exactly).
    pub fn step(&mut self, eased: f32) -> f32 {
        let eased = eased.clamp(self.eased_prev, 1.0);
        let denom = 1.0 - self.eased_prev;
        let frac = if denom <= 1e-6 {
            1.0
        } else {
            (eased - self.eased_prev) / denom
        };
        self.eased_prev = eased;
        frac.clamp(0.0, 1.0)
    }
}

/// `ZUSE_SCROLL_TRACE=1` logs per-frame glide positions at `warn` level —
/// the smoothness measurement knob (same family as `ZUSE_FRAME_STATS`).
fn scroll_trace_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED
        .get_or_init(|| std::env::var("ZUSE_SCROLL_TRACE").is_ok_and(|v| !v.is_empty() && v != "0"))
}

// ---------------------------------------------------------------------------
// Rendering + smooth scroll (Transcript extension)
// ---------------------------------------------------------------------------

impl Transcript {
    /// Smooth-scroll the list so `target` sits at the viewport top, reusing the
    /// transcript scroll-task slot (any running stick/jump animation yields).
    ///
    /// A [`motion::SCROLL_GLIDE`] (500ms ease-in-out) timeline drives every
    /// frame's position; per-frame movement comes from the timeline, never
    /// from a percent of the remaining distance:
    ///
    /// - a glued bottom anchor (`item_ix == len`, one viewport BELOW the
    ///   visible top) is first materialized as the true viewport-top anchor —
    ///   stepping straight from the glued anchor lands inside the re-glue band
    ///   and layout undoes it every frame (the old stall→double-jump path);
    /// - rows above the viewport are unmeasured, so the anchor glides in item
    ///   space along the same timeline, estimating sub-row offsets from a
    ///   local row-height EMA; the position is read back each frame, so a
    ///   measurement correcting the estimate just re-enters the timeline;
    /// - once the target row is measured the glide is pixel-exact.
    pub fn scroll_to_row(&mut self, target: usize, cx: &mut Context<Self>) {
        self.begin_scroll_navigation();
        if motion::reduced_motion(cx) {
            self.list_state().scroll_to(ListOffset {
                item_ix: target,
                offset_in_item: px(0.0),
            });
            cx.notify();
            return;
        }
        self.set_scroll_task(cx.spawn(async move |this, cx| {
            let started = Instant::now();
            let total = motion::SCROLL_GLIDE.total().mul_f32(motion::speed_scale());
            let mut timeline = GlideTimeline::new();
            let mut height_ema: Option<f32> = None;
            let trace = scroll_trace_enabled();
            let frames = (total.as_millis() / 16) as usize + 90;
            for _ in 0..frames {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let raw = (started.elapsed().as_secs_f32() / total.as_secs_f32()).min(1.0);
                let eased = motion::SCROLL_GLIDE.curve.eval(raw);
                let frac = timeline.step(eased);
                let done = this.update(cx, |t, cx| {
                    let list = t.list_state().clone();
                    if raw >= 1.0 {
                        list.scroll_to(ListOffset {
                            item_ix: target,
                            offset_in_item: px(0.0),
                        });
                        cx.notify();
                        return true;
                    }
                    // Materialize the glued bottom representation as the true
                    // top anchor (same visual position, sticky anchor).
                    let viewport = f32::from(list.viewport_bounds().size.height);
                    if t.is_glued() && viewport > 0.0 {
                        list.scroll_by(px(-(viewport + 0.5)));
                    }
                    let top = list.logical_scroll_top();
                    let top_height = list
                        .bounds_for_item(top.item_ix)
                        .map(|b| f32::from(b.size.height).max(1.0));
                    // Row-height estimate for unmeasured territory: the mean
                    // over the whole visible span, recomputed per frame (the
                    // ~dozen mixed row kinds in a viewport average out — a
                    // single-row estimate whipsaws between paragraphs and
                    // code blocks and modulates the per-frame step visibly).
                    if viewport > 0.0 {
                        let bottom = f32::from(list.viewport_bounds().bottom());
                        let mut ix = top.item_ix;
                        let mut count = 0.0f32;
                        while let Some(b) = list.bounds_for_item(ix) {
                            if f32::from(b.top()) >= bottom {
                                break;
                            }
                            count += 1.0;
                            ix += 1;
                        }
                        if count > 0.0 {
                            let mean = viewport / count;
                            let ema = height_ema.get_or_insert(mean);
                            *ema += 0.5 * (mean - *ema);
                        }
                    }
                    if height_ema.is_none() {
                        height_ema = top_height;
                    }
                    // Where the viewport top actually is, in fractional item
                    // space — read back per frame (self-correcting: an anchor
                    // the layout adjusted or re-glued keeps its real remaining
                    // distance and continues the same timeline).
                    let here = top.item_ix as f32
                        + top_height
                            .map(|h| (f32::from(top.offset_in_item) / h).clamp(0.0, 1.0))
                            .unwrap_or(0.0);
                    if trace {
                        tracing::warn!(
                            ms = started.elapsed().as_millis() as u64,
                            eased,
                            here,
                            dist = t.distance_from_bottom(),
                            "scroll-glide"
                        );
                    }

                    if target < top.item_ix {
                        // Above the viewport (unmeasured): progressive
                        // item-space anchoring within the eased timeline.
                        let next = here - frac * (here - target as f32);
                        // Small steps ride `scroll_by` — the list keeps a
                        // 320px measured leading overdraw, so a step that
                        // fits inside it crosses rows at their TRUE heights
                        // (pixel-exact frames through the gentle start and
                        // landing, where jitter would show most).
                        let step_px = (here - next) * height_ema.unwrap_or(0.0);
                        if step_px > 0.0 && step_px <= crate::transcript::OVERDRAW_PX * 0.8 {
                            list.scroll_by(px(-step_px));
                            cx.notify();
                            return false;
                        }
                        let ix = (next.floor().max(0.0) as usize).min(top.item_ix);
                        let within = next - ix as f32;
                        let offset = if ix == top.item_ix {
                            // Same row as the current anchor: measured height,
                            // pixel-exact — and never below the current offset,
                            // so motion stays monotone even when a height
                            // estimate was corrected.
                            top_height
                                .map(|h| (within * h).min(f32::from(top.offset_in_item)))
                                .unwrap_or(0.0)
                        } else {
                            within * height_ema.unwrap_or(0.0)
                        };
                        list.scroll_to(ListOffset {
                            item_ix: ix,
                            offset_in_item: px(offset),
                        });
                        cx.notify();
                        return false;
                    }
                    match list.bounds_for_item(target) {
                        Some(bounds) => {
                            // Measured: pixel-exact step along the timeline.
                            let delta = f32::from(bounds.top() - list.viewport_bounds().top());
                            list.scroll_by(px(frac * delta));
                        }
                        None => {
                            // Below but unmeasured: item space, same timeline.
                            let next = here + frac * (target as f32 - here);
                            let ix = (next.floor().max(0.0) as usize).min(target);
                            let within = next - ix as f32;
                            list.scroll_to(ListOffset {
                                item_ix: ix,
                                offset_in_item: px(within * height_ema.unwrap_or(0.0)),
                            });
                        }
                    }
                    cx.notify();
                    false
                });
                match done {
                    Ok(true) | Err(_) => return,
                    Ok(false) => {}
                }
            }
            // Timeline exhausted (shouldn't happen): land exactly.
            this.update(cx, |t, cx| {
                t.list_state().scroll_to(ListOffset {
                    item_ix: target,
                    offset_in_item: px(0.0),
                });
                cx.notify();
            })
            .ok();
        }));
    }

    /// Open a turn preview and cancel a pending close.
    fn open_outline(&mut self, active: Option<usize>, cx: &mut Context<Self>) {
        let menu = self.outline_menu_mut();
        menu.close_task = None;
        if !menu.open {
            menu.open = true;
            menu.highlighted = active;
            cx.notify();
        }
    }

    /// Close after [`OUTLINE_CLOSE_DELAY`], so the pointer can cross the gap
    /// between the rail and the list.
    fn schedule_outline_close(&mut self, cx: &mut Context<Self>) {
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OUTLINE_CLOSE_DELAY).await;
            this.update(cx, |this, cx| this.close_outline(cx)).ok();
        });
        self.outline_menu_mut().close_task = Some(task);
    }

    fn close_outline(&mut self, cx: &mut Context<Self>) {
        let menu = self.outline_menu_mut();
        menu.close_task = None;
        if menu.open {
            menu.open = false;
            menu.highlighted = None;
            cx.notify();
        }
    }

    /// Render the conversation markers and the hovered turn preview.
    pub fn render_rail(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if !self.rail_enabled() {
            return gpui::Empty.into_any_element();
        }
        let pairs = {
            let state = self.state_entity().read(cx);
            position_ticks(
                rail_ticks(&state.transcript, state.pending_echoes()),
                self.rows()
                    .iter()
                    .enumerate()
                    .map(|(index, row)| (index, row.id.as_ref())),
            )
        };
        // A minimap of one exchange is noise, not navigation — the original
        // rail hides below two marks (message-rail.tsx `marks.length < 2`).
        if pairs.len() < 2 {
            return gpui::Empty.into_any_element();
        }
        let tick_rows: Vec<usize> = pairs.iter().map(|(_, row)| *row).collect();
        // Active detection reads from the READING line, not the raw clip top:
        // the titlebar overlays the list, so a row whose top sits within that
        // chrome band is what you're reading — the sliver of the previous row
        // above it is behind the blur. Concretely, the own-turn hold parks the
        // newest prompt exactly at the chrome inset, and crediting the row at
        // the raw clip top kept the PREVIOUS tick lit for the whole runway
        // (user report). Walk forward over measured rows whose tops are at or
        // above the reading line; unmeasured rows (None bounds) stop the walk,
        // leaving the raw top row — the pre-fix behavior.
        let mut top_row = self.list_state().logical_scroll_top().item_ix;
        let read_top = f32::from(self.list_state().viewport_bounds().top())
            + crate::transcript::OWN_SEND_TOP_INSET_PX
            + 0.5;
        while let Some(bounds) = self.list_state().bounds_for_item(top_row + 1) {
            if f32::from(bounds.top()) <= read_top {
                top_row += 1;
            } else {
                break;
            }
        }
        let active = active_tick(&tick_rows, top_row);
        let theme = Theme::of(cx).clone();

        // Fixed footprint (shadcn Transcript Outline): a compact stack of at
        // most MAX_RAIL_TICKS marks — past that, ticks become even buckets
        // over the conversation. Pre-layout the viewport reads 0; assume a
        // typical height for that one frame rather than collapsing to a
        // single tick.
        let viewport_h = f32::from(self.list_state().viewport_bounds().size.height);
        let capacity = rail_slots(if viewport_h > 0.0 { viewport_h } else { 600.0 });
        let buckets = tick_buckets(pairs.len(), capacity);
        let highlighted = self.outline_menu().highlighted;
        let hovered_bucket = highlighted.and_then(|ix| bucket_of(&buckets, ix));
        // The measured visible rows describe which turns intersect the viewport.
        // Always include the turn containing the reading line, even if its
        // prompt is above the viewport.
        let viewport_bottom = self.list_state().viewport_bounds().bottom();
        let mut bottom_row = top_row;
        while let Some(bounds) = self.list_state().bounds_for_item(bottom_row + 1) {
            if bounds.top() >= viewport_bottom {
                break;
            }
            bottom_row += 1;
        }
        let visible_end = active_tick(&tick_rows, bottom_row).unwrap_or(0);
        let visible_start = active.unwrap_or(0);
        let reduced_motion = motion::reduced_motion(cx);
        let focus = self.outline_menu().focus.clone();
        let marks = buckets.iter().enumerate().map(|(ix, &(start, end))| {
            let selected = hovered_bucket == Some(ix);
            let neighbor = hovered_bucket.is_some_and(|hover| hover.abs_diff(ix) == 1);
            let in_view = start <= visible_end && end > visible_start;
            let target = if active.is_some_and(|active| active >= start && active < end) {
                active.unwrap_or(start)
            } else {
                start
            };
            let row = pairs[target].1;
            let width = if selected {
                28.0
            } else if neighbor {
                20.0
            } else {
                12.0
            };
            let mark = div()
                .h(px(OUTLINE_MARK))
                .w(px(width))
                .rounded_full()
                .bg(if in_view {
                    theme.accent
                } else {
                    theme.text_muted.opacity(0.38)
                });
            let mark: AnyElement = if reduced_motion {
                mark.into_any_element()
            } else {
                let from = if selected { 12.0 } else { width };
                mark.with_animation(
                    SharedString::from(format!("rail-mark-{ix}-{selected}-{neighbor}")),
                    motion::MENU_IN.animation(),
                    move |el, t| el.w(px(motion::lerp(from, width, t))),
                )
                .into_any_element()
            };
            div()
                .id(("outline-turn", ix))
                .debug_selector(move || format!("outline-turn-{ix}"))
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(truncate_preview(
                    &pairs[target].0.prompt,
                    PREVIEW_PROMPT_CHARS,
                )))
                .w(px(OUTLINE_WIDTH))
                .h(px(OUTLINE_TICK_PITCH))
                .flex()
                .items_center()
                .justify_end()
                .pr(px(8.0))
                .cursor_pointer()
                .child(mark)
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.open_outline(Some(target), cx);
                        this.outline_menu_mut().highlighted = Some(target);
                        cx.notify();
                    } else {
                        this.schedule_outline_close(cx);
                    }
                }))
                .on_click(cx.listener(move |this, _, window, cx| {
                    window.focus(&this.outline_menu().focus, cx);
                    this.open_outline(Some(target), cx);
                    this.outline_menu_mut().highlighted = Some(target);
                    this.scroll_to_row(row, cx);
                }))
        });
        let preview: Option<AnyElement> = highlighted.and_then(|ix| {
            pairs.get(ix).map(|(tick, _)| {
                let popup = theme.for_popup();
                let title = SharedString::from(crate::transcript::single_line(&tick.prompt));
                let reply = SharedString::from(
                    tick.reply
                        .as_deref()
                        .map(crate::transcript::single_line)
                        .filter(|text| !text.is_empty())
                        .unwrap_or_default(),
                );
                // Fixed footprint: the title is one line, slightly larger than
                // the description, which always occupies exactly two lines.
                let card = popover::popover_card(&popup)
                    .id("transcript-outline-card")
                    .w(px(OUTLINE_LIST_WIDTH))
                    .px(px(OUTLINE_CARD_PAD_X))
                    .py(px(OUTLINE_CARD_PAD_Y))
                    .flex()
                    .flex_col()
                    .gap(px(OUTLINE_CARD_GAP))
                    .overflow_hidden()
                    .child(
                        div()
                            .w(px(OUTLINE_TEXT_W))
                            .h(px(OUTLINE_TITLE_LH))
                            .line_height(px(OUTLINE_TITLE_LH))
                            .text_size(crate::typography::text_md())
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(popup.text)
                            .line_clamp(1)
                            .child(title),
                    )
                    .child(
                        div()
                            .w(px(OUTLINE_TEXT_W))
                            .h(px(OUTLINE_BODY_LH * 2.0))
                            .line_height(px(OUTLINE_BODY_LH))
                            .text_size(crate::typography::text_xs())
                            .text_color(popup.text_muted)
                            .line_clamp(2)
                            .child(reply),
                    )
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.outline_menu_mut().close_task = None;
                        } else {
                            this.schedule_outline_close(cx);
                        }
                    }));
                let card = crate::frost::frosted(12.0, crate::frost::MENU_BLUR, card);
                let card = if reduced_motion {
                    card.into_any_element()
                } else {
                    motion::menu_in(
                        SharedString::from(format!("rail-preview-{ix}")),
                        div().child(card),
                    )
                    .into_any_element()
                };
                let bucket = bucket_of(&buckets, ix).unwrap_or(0);
                div()
                    .absolute()
                    .top(px(bucket as f32 * OUTLINE_TICK_PITCH))
                    .left(px(-OUTLINE_SIDE_OFFSET))
                    .size_0()
                    .child(
                        gpui::deferred(
                            gpui::anchored()
                                .anchor(gpui::Anchor::TopRight)
                                .snap_to_window_with_margin(px(8.0))
                                .child(div().occlude().child(card)),
                        )
                        .priority(1),
                    )
                    .into_any_element()
            })
        });
        let rows_for_keys: Vec<usize> = pairs.iter().map(|(_, row)| *row).collect();
        div()
            .id("transcript-outline")
            .debug_selector(|| "transcript-outline".into())
            .track_focus(&focus)
            .on_key_down(
                cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    match event.keystroke.key.as_str() {
                        "up" | "down" => {
                            let current = this.outline_menu().highlighted.or(active).unwrap_or(0);
                            let next = if event.keystroke.key == "up" {
                                current.saturating_sub(1)
                            } else {
                                (current + 1).min(rows_for_keys.len() - 1)
                            };
                            this.open_outline(Some(next), cx);
                            this.outline_menu_mut().highlighted = Some(next);
                            cx.notify();
                        }
                        "enter" | "space" => {
                            if let Some(row) = this
                                .outline_menu()
                                .highlighted
                                .and_then(|ix| rows_for_keys.get(ix))
                                .copied()
                            {
                                this.scroll_to_row(row, cx);
                            }
                        }
                        "escape" => {
                            this.close_outline(cx);
                            window.blur();
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
            .absolute()
            .top(px(Theme::TITLEBAR_HEIGHT + OUTLINE_TOP))
            .right(px(OUTLINE_RIGHT))
            .child(
                div()
                    .relative()
                    .child(div().flex().flex_col().children(marks))
                    .children(preview),
            )
            .into_any_element()
    }
}

/// Preview and keyboard navigation state.
pub struct OutlineMenu {
    pub(crate) open: bool,
    pub(crate) highlighted: Option<usize>,
    pub(crate) close_task: Option<gpui::Task<()>>,
    pub(crate) focus: gpui::FocusHandle,
}

#[cfg(feature = "browser-fixture")]
impl Transcript {
    /// Review fixture: the preview open on the first prompt.
    pub fn fixture_open_outline(&mut self, cx: &mut Context<Self>) {
        let menu = self.outline_menu_mut();
        menu.open = true;
        menu.highlighted = Some(0);
        cx.notify();
    }
}

impl OutlineMenu {
    pub fn new(cx: &mut gpui::App) -> Self {
        Self {
            open: false,
            highlighted: None,
            close_task: None,
            focus: cx.focus_handle(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zuse_doc::MessageStatus;

    fn entry(id: &str, role: MessageRole, text: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.into(),
            }],
            created_at: 0,
            device_id: "d".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    #[test]
    fn capacity_counts_slots_that_fit() {
        // 880px viewport − 48 margin = 832 usable → (832+3)/13 = 64 slots.
        assert_eq!(rail_capacity(880.0), 64);
        // Tiny (or unmeasured) heights still hand out one slot.
        assert_eq!(rail_capacity(0.0), 1);
        assert!(rail_capacity(200.0) >= 10);
        // The rail itself is hard-capped: compact on any window height.
        assert_eq!(rail_slots(880.0), MAX_RAIL_TICKS);
        assert_eq!(rail_slots(2000.0), MAX_RAIL_TICKS);
        // Short rails still shrink below the cap.
        assert!(rail_slots(100.0) < MAX_RAIL_TICKS);
    }

    #[test]
    fn buckets_are_identity_under_capacity() {
        // n <= capacity: one tick per prompt — the old per-prompt rail.
        let b = tick_buckets(5, 64);
        assert_eq!(b.len(), 5);
        assert!(
            b.iter()
                .enumerate()
                .all(|(k, &(s, e))| s == k && e == k + 1)
        );
    }

    #[test]
    fn buckets_partition_evenly_over_capacity() {
        // 100 prompts into 8 slots: every tick in exactly one bucket, in
        // order, first starts at 0, last ends at n, sizes within ±1 of even.
        let n = 100;
        let b = tick_buckets(n, 8);
        assert_eq!(b.len(), 8);
        assert_eq!(b[0].0, 0);
        assert_eq!(b.last().unwrap().1, n);
        for w in b.windows(2) {
            assert_eq!(w[0].1, w[1].0, "contiguous");
        }
        for &(s, e) in &b {
            assert!((e - s) == 12 || (e - s) == 13, "even split, got {}", e - s);
        }
    }

    #[test]
    fn bucket_of_maps_ticks_to_their_bucket() {
        let b = tick_buckets(10, 3); // [0,3) [3,6) [6,10)
        assert_eq!(bucket_of(&b, 0), Some(0));
        assert_eq!(bucket_of(&b, 3), Some(1));
        assert_eq!(bucket_of(&b, 9), Some(2));
        assert_eq!(bucket_of(&b, 10), None);
        // Degenerate inputs.
        assert!(tick_buckets(0, 8).is_empty());
        assert_eq!(tick_buckets(3, 0), vec![(0, 3)]);
    }

    #[test]
    fn ticks_map_user_prompts_with_reply_openings() {
        let entries = vec![
            entry("u1", MessageRole::User, "first question"),
            entry("a1", MessageRole::Assistant, "first answer"),
            entry("u2", MessageRole::User, "second question"),
            entry("a2", MessageRole::Assistant, "second answer"),
        ];
        let ticks = rail_ticks(&entries, &[]);
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0].message_id, "u1");
        assert_eq!(ticks[0].prompt, "first question");
        assert_eq!(ticks[0].reply.as_deref(), Some("first answer"));
        assert_eq!(ticks[1].reply.as_deref(), Some("second answer"));
    }

    #[test]
    fn outline_targets_preserve_prompt_order_and_first_matching_row() {
        let entries = vec![
            entry("first", MessageRole::User, "first question"),
            entry("hidden", MessageRole::User, "no visible row"),
            entry("last", MessageRole::User, "last question"),
        ];
        let ticks = rail_ticks(&entries, &[]);
        let expected = vec![(ticks[0].clone(), 7), (ticks[2].clone(), 3)];
        assert_eq!(
            position_ticks(ticks, [(3, "last"), (7, "first"), (9, "first")]),
            expected
        );
        assert!(position_ticks(rail_ticks(&entries, &[]), []).is_empty());
        assert!(position_ticks(vec![], [(0, "first")]).is_empty());
    }

    #[test]
    fn ticks_include_echoes_deduped() {
        let entries = vec![entry("u1", MessageRole::User, "sent")];
        let echoes = vec![
            entry("u1", MessageRole::User, "sent"), // confirmed already → deduped
            entry("u2", MessageRole::User, "pending"),
        ];
        let ticks = rail_ticks(&entries, &echoes);
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[1].message_id, "u2");
        assert_eq!(ticks[1].reply, None);
    }

    #[test]
    fn preview_does_not_borrow_reply_from_next_turn() {
        let entries = vec![
            entry("u1", MessageRole::User, "unanswered"),
            entry("u2", MessageRole::User, "next"),
            entry("a2", MessageRole::Assistant, "answer to next"),
        ];
        let ticks = rail_ticks(&entries, &[]);
        assert_eq!(ticks[0].reply, None);
        assert_eq!(ticks[1].reply.as_deref(), Some("answer to next"));
        assert!(rail_visible(400.0));
    }

    #[test]
    fn tick_without_reply_yet() {
        let entries = vec![
            entry("u1", MessageRole::User, "q"),
            entry("a1", MessageRole::Assistant, "reply to first"),
            entry("u2", MessageRole::User, "latest"),
        ];
        let ticks = rail_ticks(&entries, &[]);
        // The last prompt has no assistant entry after it.
        assert_eq!(ticks[1].reply, None);
        // Empty transcript → no ticks.
        assert!(rail_ticks(&[], &[]).is_empty());
    }

    #[test]
    fn active_tick_tracks_viewport_top() {
        let tick_rows = [0, 5, 9];
        assert_eq!(active_tick(&tick_rows, 0), Some(0));
        assert_eq!(active_tick(&tick_rows, 4), Some(0));
        assert_eq!(active_tick(&tick_rows, 5), Some(1));
        assert_eq!(active_tick(&tick_rows, 8), Some(1));
        assert_eq!(active_tick(&tick_rows, 100), Some(2));
        // Above the first tick row → first tick still active.
        assert_eq!(active_tick(&[3, 7], 1), Some(0));
        assert_eq!(active_tick(&[], 4), None);
    }

    /// Consuming `(e'−e)/(1−e)` of the current remainder telescopes to exactly
    /// the absolute eased timeline `start + e(t)·total` when the distance
    /// estimate is stable — the glide is timeline-driven, not
    /// percent-of-remaining.
    #[test]
    fn glide_timeline_matches_absolute_eased_interpolation() {
        let curve = motion::SCROLL_GLIDE.curve;
        let mut timeline = GlideTimeline::new();
        let (start, target) = (1000.0f32, 0.0f32);
        let mut pos = start;
        for i in 1..=60 {
            let t = i as f32 / 60.0;
            let eased = curve.eval(t);
            let frac = timeline.step(eased);
            pos -= frac * (pos - target);
            let absolute = start + eased * (target - start);
            assert!(
                (pos - absolute).abs() < 0.05,
                "frame {i}: pos {pos} != absolute {absolute}"
            );
        }
        assert_eq!(pos, target); // eased hits 1.0 → frac 1.0 → exact landing.
    }

    /// A mid-flight distance re-estimate (anchor re-glued / row measured)
    /// continues the SAME timeline over the corrected remainder: no restart,
    /// no compensating jump, exact landing.
    #[test]
    fn glide_timeline_survives_remaining_distance_reestimate() {
        let curve = motion::SCROLL_GLIDE.curve;
        let mut timeline = GlideTimeline::new();
        let mut pos = 500.0f32;
        let mut prev_frac = 0.0f32;
        for i in 1..=60 {
            let t = i as f32 / 60.0;
            let frac = timeline.step(curve.eval(t));
            if i == 30 {
                // The layout re-glued the anchor: remaining distance doubles.
                pos *= 2.0;
            }
            pos -= frac * pos;
            // Fractions depend only on the timeline — the re-estimate cannot
            // make a step consume a larger share than the curve dictates.
            assert!((0.0..=1.0).contains(&frac));
            if i > 1 && i < 55 {
                assert!(frac >= prev_frac - 0.05, "frame {i}: frac regressed");
            }
            prev_frac = frac;
        }
        assert_eq!(pos, 0.0);
    }

    /// Timeline steps clamp: regressions in eased input yield zero movement,
    /// and completion always yields the full remainder.
    #[test]
    fn glide_timeline_step_clamps() {
        let mut timeline = GlideTimeline::new();
        assert_eq!(timeline.step(0.4), 0.4);
        assert_eq!(timeline.step(0.3), 0.0); // non-monotone input → no move
        assert_eq!(timeline.step(1.0), 1.0); // done → land exactly
        assert_eq!(timeline.step(1.0), 1.0); // idempotent at the end
    }

    fn context_item(label: &str) -> crate::context_items::ContextItem {
        crate::context_items::ContextItem {
            id: "c".into(),
            source_key: "k".into(),
            label: label.into(),
            body: "marked heading".into(),
            browser_annotation: None,
        }
    }

    #[test]
    fn prompt_stays_primary_when_an_annotation_is_attached() {
        let sent = crate::context_items::with_contexts(
            "I'll make the YouTube heading larger and red",
            &[context_item("localhost · 1 element")],
        );
        assert!(sent.contains("<context"));
        assert_eq!(
            rail_prompt_label(&sent),
            "I'll make the YouTube heading larger and red"
        );
    }

    #[test]
    fn annotation_only_send_names_the_pills() {
        let sent =
            crate::context_items::with_contexts("", &[context_item("localhost · 1 element")]);
        assert_eq!(rail_prompt_label(&sent), "localhost · 1 element");

        let with_file = crate::context_items::with_contexts(
            &crate::attachments::with_attachments("", &["/a/shot.png".to_string()]),
            &[context_item("localhost · 1 element")],
        );
        assert_eq!(
            rail_prompt_label(&with_file),
            "localhost · 1 element · Attached image"
        );

        let comment = crate::comments::with_comments(
            "",
            &[crate::comments::ReviewComment::new(
                "a.rs",
                crate::comments::CommentSide::New,
                4,
                "fix",
            )],
        );
        assert_eq!(rail_prompt_label(&comment), "1 comment");
        assert_eq!(rail_prompt_label("plain prompt"), "plain prompt");
    }

    #[test]
    fn preview_truncation() {
        assert_eq!(truncate_preview("short", 10), "short");
        assert_eq!(truncate_preview("  padded  ", 10), "padded");
        let long = "x".repeat(50);
        let cut = truncate_preview(&long, 10);
        assert!(cut.chars().count() <= 10);
        assert!(cut.ends_with('…'));
        // Multi-byte safety.
        let uni = "héllo wörld attaché case overflowing";
        let cut = truncate_preview(uni, 12);
        assert!(cut.ends_with('…'));
    }
}
