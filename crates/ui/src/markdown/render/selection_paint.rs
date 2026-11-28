//! Draw selection and find highlights, and register shaped text for pointer drags.

use std::{
    cell::{Cell, RefCell},
    ops::Range,
};

use gpui::{
    AnyElement, BorderStyle, Bounds, CursorStyle, Div, Hsla, SharedString, StyledText, TextRun,
    Window, canvas, div, point, prelude::*, px, quad, size,
};

/// Paint and register selection for the composer's plain-text test surface.
#[cfg(test)]
pub(crate) fn paint_text_selection(
    window: &mut Window,
    hitbox: gpui::Hitbox,
    key: &std::sync::Arc<str>,
    text: &SharedString,
    layout: &gpui::TextLayout,
    theme: &crate::theme::Theme,
) {
    paint_text_selection_with_wash(window, hitbox, key, text, layout, theme.selection, None);
}

/// Paint selection and register this element for document-ordered drags.
/// Offsets map source text to display-only glyphs such as code padding.
pub(super) fn paint_text_selection_with_wash(
    window: &mut Window,
    hitbox: gpui::Hitbox,
    key: &std::sync::Arc<str>,
    text: &SharedString,
    layout: &gpui::TextLayout,
    wash: Hsla,
    offsets: Option<&crate::markdown::link_presentation::OffsetMap>,
) {
    let surface = PAINTING_SURFACE.with(Cell::get);
    paint_find_matches(window, surface, key, text, layout, offsets);
    if let Some(range) = crate::markdown::selection::wash_range(surface, key) {
        let range = offsets.map_or_else(|| range.clone(), |map| map.displayed_range(range.clone()));
        for rect in range_rects(layout, &range, 0.0, 0.0) {
            window.paint_quad(quad(
                rect,
                px(0.0),
                wash,
                px(0.0),
                gpui::transparent_black(),
                BorderStyle::default(),
            ));
        }
    }
    REGISTRY.with(|r| {
        r.borrow_mut().push(RegEntry {
            surface,
            key: key.clone(),
            text: text.clone(),
            layout: layout.clone(),
            offsets: offsets.cloned(),
        })
    });
    register_selection_listeners(window, hitbox, key, surface, text, layout, offsets.cloned());
}

/// Find-in-chat washes for one painted text element, beneath the selection
/// wash. The active match is the row's Nth occurrence, so the elements of
/// the same row painted earlier this frame tell which one is local here.
fn paint_find_matches(
    window: &mut Window,
    surface: u64,
    key: &str,
    text: &str,
    layout: &gpui::TextLayout,
    offsets: Option<&crate::markdown::link_presentation::OffsetMap>,
) {
    use crate::chat_find::{Highlight, current_highlight, match_ranges};
    let Some(find) = current_highlight(surface) else {
        return;
    };
    let ranges = match_ranges(text, &find.needle);
    if ranges.is_empty() {
        return;
    }
    let active = find
        .active
        .as_ref()
        .filter(|(row, _)| Highlight::key_in_row(key, row))
        .and_then(|(row, ordinal)| {
            let before: usize = REGISTRY.with(|registry| {
                registry
                    .borrow()
                    .iter()
                    .filter(|e| e.surface == surface && Highlight::key_in_row(&e.key, row))
                    .map(|e| match_ranges(&e.text, &find.needle).len())
                    .sum()
            });
            ordinal.checked_sub(before)
        });
    for (ix, range) in ranges.into_iter().enumerate() {
        let range = offsets.map_or_else(|| range.clone(), |map| map.displayed_range(range.clone()));
        let wash = if Some(ix) == active {
            find.active_wash
        } else {
            find.wash
        };
        for rect in range_rects(layout, &range, 0.0, 0.0) {
            window.paint_quad(quad(
                rect,
                px(2.0),
                wash,
                px(0.0),
                gpui::transparent_black(),
                BorderStyle::default(),
            ));
        }
    }
}

/// The wrapping div shared by every selectable text region: markdown
/// paragraphs (and their table/list/quote kin), code lines, and the
/// transcript's user bubbles. `relative` so the selection underlay can
/// paint beneath the text, I-beam so hovering the region reads as text.
/// Link hitboxes overlay the wrapper with `.cursor_pointer()` and the
/// topmost hitbox under the mouse wins, so links inside keep the hand
/// cursor.
pub(crate) fn selectable_text_wrap() -> Div {
    div().relative().cursor(CursorStyle::IBeam)
}

pub(crate) fn selectable_text_element(
    key: std::sync::Arc<str>,
    text: SharedString,
    runs: Vec<TextRun>,
    wash: Hsla,
) -> AnyElement {
    let styled = StyledText::new(text.clone()).with_runs(runs);
    let layout = styled.layout().clone();
    let underlay = canvas(
        |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            paint_text_selection_with_wash(window, hitbox, &key, &text, &layout, wash, None);
        },
    )
    .absolute()
    .size_full();
    selectable_text_wrap()
        .child(underlay)
        .child(styled)
        .into_any_element()
}

/// One painted text element, registered per frame in document order — the
/// continuity model that lets a drag span paragraphs/list items (Zed gets
/// this for free from its single-element markdown; our tree rebuilds it).
pub(super) struct RegEntry {
    /// Which painted transcript this element belongs to (see
    /// `selection_frame_reset_for`); 0 for surfaces that never share a frame.
    pub(super) surface: u64,
    pub(super) key: std::sync::Arc<str>,
    pub(super) text: SharedString,
    pub(super) layout: gpui::TextLayout,
    pub(super) offsets: Option<crate::markdown::link_presentation::OffsetMap>,
}

thread_local! {
    pub(super) static REGISTRY: RefCell<Vec<RegEntry>> = const { RefCell::new(Vec::new()) };
    /// The surface currently painting: set by its reset canvas, which paints
    /// before any of its text, and stamped onto each entry it registers.
    static PAINTING_SURFACE: Cell<u64> = const { Cell::new(0) };
    /// Surfaces whose reset painted since the latest one's previous reset.
    static PAINTED_SURFACES: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn selection_test_entry_count(prefix: &str) -> usize {
    REGISTRY.with(|registry| {
        registry
            .borrow()
            .iter()
            .filter(|entry| entry.key.starts_with(prefix))
            .count()
    })
}

#[cfg(test)]
pub(crate) fn selection_test_bounds(key: &str) -> gpui::Bounds<gpui::Pixels> {
    REGISTRY.with(|r| {
        r.borrow()
            .iter()
            .find(|entry| entry.key.as_ref() == key)
            .expect("text must be registered")
            .layout
            .bounds()
    })
}

#[cfg(test)]
pub(crate) fn selection_test_snapshot(
    key: &str,
) -> (
    SharedString,
    gpui::TextLayout,
    Option<crate::markdown::link_presentation::OffsetMap>,
) {
    REGISTRY.with(|registry| {
        let registry = registry.borrow();
        let entry = registry
            .iter()
            .find(|entry| entry.key.as_ref() == key)
            .expect("painted text");
        (
            entry.text.clone(),
            entry.layout.clone(),
            entry.offsets.clone(),
        )
    })
}

/// A zero-size canvas that clears the selection registry — paint it FIRST in
/// the transcript root (before any markdown), so each frame's registry holds
/// exactly that frame's visible text elements in paint order.
pub fn selection_frame_reset() -> impl IntoElement {
    selection_frame_reset_for(0)
}

/// `selection_frame_reset` for one of several surfaces painted in the same
/// frame (the main chat beside a side chat or subagent tab). It clears only
/// its own surface's entries and stamps the text painted after it, so
/// another transcript's reset can't wipe this one's drag anchor.
///
/// A surface that stops painting (closed side chat or subagent tab) never
/// resets again, so its entries are dropped once a full cycle of this
/// surface's resets passes without it.
pub fn selection_frame_reset_for(surface: u64) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, _, _| {
            PAINTING_SURFACE.with(|s| s.set(surface));
            PAINTED_SURFACES.with(|painted| {
                let mut painted = painted.borrow_mut();
                // Seeing this surface again closes a cycle: `painted` now
                // holds every surface still on screen.
                let cycle = painted.contains(&surface);
                REGISTRY.with(|r| {
                    r.borrow_mut().retain(|e| {
                        !selection_scope(&e.key).is_empty()
                            || (e.surface != surface && (!cycle || painted.contains(&e.surface)))
                    })
                });
                if cycle {
                    painted.clear();
                }
                painted.push(surface);
            });
        },
    )
    .absolute()
    .w(px(0.0))
    .h(px(0.0))
}

fn selection_scope(key: &str) -> &str {
    if key.starts_with("md-preview-") {
        key.split_once('|').map_or("", |(scope, _)| scope)
    } else {
        ""
    }
}

pub(crate) fn clear_selection_surface(prefix: &str) {
    REGISTRY.with(|r| {
        r.borrow_mut()
            .retain(|entry| !entry.key.starts_with(prefix))
    });
    if let Some(anchor) =
        crate::markdown::selection::anchor_key().filter(|key| key.starts_with(prefix))
    {
        // Closing a surface can happen before mouse-up. Retire its active
        // drag too, without disturbing a selection owned by another view.
        crate::markdown::selection::end_active_drag();
        crate::markdown::selection::clear_if_owner(&anchor);
    }
}

pub fn selection_surface_reset(prefix: String) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, _, _| REGISTRY.with(|r| r.borrow_mut().retain(|e| !e.key.starts_with(&prefix))),
    )
    .absolute()
    .w(px(0.0))
    .h(px(0.0))
}

/// `(element index, byte offset)` for a window position: the registered
/// element whose bounds contain it, else the nearest horizontally within the
/// closest vertical band. Vertical distance stays primary so dragging beyond
/// a short paragraph's edge keeps selecting that line. Horizontal distance
/// distinguishes side-by-side text such as table cells and their padding.
fn registry_point(position: gpui::Point<gpui::Pixels>) -> Option<(usize, usize)> {
    REGISTRY.with(|r| {
        let reg = r.borrow();
        let anchor = crate::markdown::selection::anchor_key().unwrap_or_default();
        // A drag only resolves against its own surface, so a second
        // transcript on screen can't capture it.
        let anchor_surface = crate::markdown::selection::anchor_surface();
        let mut best: Option<(usize, f32, f32)> = None;
        for (ei, entry) in reg.iter().enumerate() {
            if selection_scope(&entry.key) != selection_scope(&anchor)
                || entry.surface != anchor_surface
            {
                continue;
            }
            let b = entry.layout.bounds();
            let dx = if position.x < b.left() {
                f32::from(b.left() - position.x)
            } else if position.x > b.right() {
                f32::from(position.x - b.right())
            } else {
                0.0
            };
            let dy = if position.y < b.top() {
                f32::from(b.top() - position.y)
            } else if position.y > b.bottom() {
                f32::from(position.y - b.bottom())
            } else {
                0.0
            };
            if best.is_none_or(|(_, best_dy, best_dx)| (dy, dx) < (best_dy, best_dx)) {
                best = Some((ei, dy, dx));
            }
            if dy == 0.0 && dx == 0.0 {
                break;
            }
        }
        let (ei, _, _) = best?;
        let ix = match reg[ei].layout.index_for_position(position) {
            Ok(ix) | Err(ix) => ix,
        };
        Some((
            ei,
            reg[ei].offsets.as_ref().map_or(ix, |map| map.original(ix)),
        ))
    })
}

/// Resolve the drag head into document-ordered spans over the frame's registry
/// and store them; true if the selection changed. The selection model retains
/// spans across overlapping virtualized frames once its anchor scrolls away.
fn resolve_drag(head: (usize, usize)) -> bool {
    REGISTRY.with(|r| {
        let reg = r.borrow();
        let Some(entry) = reg.get(head.0) else {
            return false;
        };
        let scope = selection_scope(&entry.key);
        let filtered: Vec<_> = reg
            .iter()
            .enumerate()
            .filter(|(_, e)| selection_scope(&e.key) == scope && e.surface == entry.surface)
            .collect();
        let Some(index) = filtered.iter().position(|(ix, _)| *ix == head.0) else {
            return false;
        };
        let elements: Vec<_> = filtered
            .iter()
            .map(|(_, e)| (e.key.as_ref(), e.text.as_ref()))
            .collect();
        crate::markdown::selection::update_drag(&elements, (index, head.1))
    })
}

/// Continue the active drag at a window position. The transcript's edge-scroll
/// driver calls this between scroll steps, so a stationary pointer keeps
/// extending through newly painted rows.
pub(crate) fn update_drag_at(position: gpui::Point<gpui::Pixels>) -> bool {
    let Some(head) = registry_point(position) else {
        return false;
    };
    resolve_drag(head)
}

/// Register this frame's window-level mouse listeners for one text element's
/// selection (Zed-markdown mechanics: window-level so a drag keeps tracking
/// outside the element's bounds; frame-scoped, so paint re-registers).
fn register_selection_listeners(
    window: &mut Window,
    hitbox: gpui::Hitbox,
    key: &std::sync::Arc<str>,
    surface: u64,
    text: &SharedString,
    layout: &gpui::TextLayout,
    offsets: Option<crate::markdown::link_presentation::OffsetMap>,
) {
    use gpui::{DispatchPhase, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};
    {
        let (key, text, layout) = (key.clone(), text.clone(), layout.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, _cx| {
            if phase != DispatchPhase::Bubble || e.button != MouseButton::Left {
                return;
            }
            // Geometry alone includes text hidden behind popups or clipping.
            // Only start a selection when this surface receives the press;
            // subsequent drag events stay window-wide to span text blocks.
            if hitbox.is_hovered(window) && layout.bounds().contains(&e.position) {
                let ix = match layout.index_for_position(e.position) {
                    Ok(ix) | Err(ix) => ix,
                };
                let ix = offsets.as_ref().map_or(ix, |map| map.original(ix));
                match e.click_count {
                    2 => {
                        let range = crate::markdown::selection::word_range(&text, ix);
                        crate::markdown::selection::begin_with_span_in(surface, &key, &text, range);
                    }
                    n if n >= 3 => {
                        crate::markdown::selection::begin_with_span_in(
                            surface,
                            &key,
                            &text,
                            0..text.len(),
                        );
                    }
                    _ => crate::markdown::selection::begin_in(surface, &key, ix),
                }
                window.refresh();
            } else if crate::markdown::selection::clear_if_owner(&key) {
                window.refresh();
            }
        });
    }
    {
        let key = key.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, _cx| {
            if phase != DispatchPhase::Bubble || !e.dragging() {
                return;
            }
            // Only the anchor element's listener drives the drag.
            if crate::markdown::selection::drag_anchor(&key).is_none() {
                return;
            }
            if update_drag_at(e.position) {
                window.refresh();
            }
        });
    }
    {
        let key = key.clone();
        window.on_mouse_event(move |_: &MouseUpEvent, phase, _window, _cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            if let Some(_text) = crate::markdown::selection::end_drag(&key) {
                // X11 middle-click paste parity (Zed does the same).
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                _cx.write_to_primary(gpui::ClipboardItem::new_string(_text));
            }
        });
    }
}

/// The wash boxes for one byte range: one box per visual line the range
/// covers (soft wraps split it), in window coordinates from the laid-out
/// text's own geometry. `pad_x` overhangs the box horizontally (inline code);
/// `inset_y` shrinks it vertically — both 0 for a selection wash, which wants
/// full-line-height boxes that tile seamlessly across wrapped rows.
pub(crate) fn range_rects(
    layout: &gpui::TextLayout,
    range: &Range<usize>,
    pad_x: f32,
    inset_y: f32,
) -> Vec<Bounds<gpui::Pixels>> {
    range_rects_with_positions(
        layout.bounds(),
        layout.line_height(),
        range,
        pad_x,
        inset_y,
        |ix| layout.position_for_index(ix),
    )
}

pub(super) fn range_rects_with_positions(
    bounds: Bounds<gpui::Pixels>,
    line_height: gpui::Pixels,
    range: &Range<usize>,
    pad_x: f32,
    inset_y: f32,
    position_for_index: impl Fn(usize) -> Option<gpui::Point<gpui::Pixels>>,
) -> Vec<Bounds<gpui::Pixels>> {
    let mut rects = Vec::new();
    let mut cur = range.start;
    // Walk the range one visual row at a time: find the furthest index that
    // still sits on the current row (binary search over glyph positions).
    let mut guard = 0;
    while cur < range.end && guard < 256 {
        guard += 1;
        let Some(mut p1) = position_for_index(cur) else {
            break;
        };
        // GPUI gives a soft-wrap boundary upstream affinity: the boundary
        // index is reported at the end of the preceding visual row. When the
        // following byte advances to a lower row, `cur` is also the start of
        // that row; use its downstream position for the range start.
        if let Some(after) = position_for_index(cur.saturating_add(1))
            && after.y > p1.y
        {
            p1 = point(bounds.left(), after.y);
        }
        // `seg_end` closes the wash on this row; `next` is the first index on
        // the following row. A soft-wrap boundary belongs to both rows, so it
        // closes this rectangle with upstream affinity and starts the next
        // rectangle with the downstream correction above.
        let (seg_end, next) = match position_for_index(range.end) {
            Some(pe) if pe.y == p1.y => (range.end, range.end),
            _ => {
                // Largest ix on this row (probes stay on char boundaries only
                // at the ends; intermediate probes just need a y).
                let (mut lo, mut hi) = (cur, range.end);
                while hi - lo > 1 {
                    let mid = lo + (hi - lo) / 2;
                    match position_for_index(mid) {
                        Some(pm) if pm.y == p1.y => lo = mid,
                        _ => hi = mid,
                    }
                }
                (lo, lo)
            }
        };
        if let Some(p2) = position_for_index(seg_end)
            && p2.x > p1.x
        {
            rects.push(Bounds::new(
                point(p1.x - px(pad_x), p1.y + px(inset_y)),
                size(
                    p2.x - p1.x + px(2.0 * pad_x),
                    line_height - px(2.0 * inset_y),
                ),
            ));
        }
        if next <= cur {
            break;
        }
        cur = next;
    }
    rects
}
