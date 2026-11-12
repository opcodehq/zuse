//! The app's one tooltip: a small frosted chip with crisp sans text, centred
//! on the control it describes. The browser toolbar set the look; every
//! hover note across the app now shares it so tooltips read as one system.
//!
//! gpui hangs a tooltip off the pointer (below-right of wherever the mouse
//! happened to enter), which lets chips drift over neighbouring content. The
//! [`TooltipExt`] helpers record the owner's bounds while it paints and anchor
//! the chip to them instead: centred on the control, a fixed gap away, and
//! flipped to the other side when the window edge leaves no room.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyView, App, Bounds, Context, Pixels, Point, SharedString, Window, canvas, div, prelude::*, px,
};

/// Horizontal padding inside a chip.
pub const PAD_X: f32 = 5.5;
/// Vertical padding inside a chip.
pub const PAD_Y: f32 = 3.0;
/// Chip corner radius.
pub const RADIUS: f32 = 6.0;
/// Label size.
pub const TEXT_SIZE: f32 = 11.5;
/// Space between the control and the chip.
pub const GAP: f32 = 5.0;
/// Longest a one-line note grows before it wraps.
pub const MAX_WIDTH: f32 = 320.0;
/// Chips never touch the window edge.
const WINDOW_MARGIN: f32 = 8.0;
/// Rough single-line chip height, used to decide whether the preferred side
/// has room before flipping. Generous so a wrapped chip still fits.
const FLIP_ROOM: f32 = 34.0;

/// Which side of its control a chip prefers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Below,
    Above,
}

/// The host can supply its own theme-aware tooltip view. Other hosts use the
/// same geometry and a palette-based fallback.
pub struct TooltipRenderer(pub fn(SharedString, &mut Window, &mut App) -> AnyView);
impl gpui::Global for TooltipRenderer {}

struct FallbackTooltip(SharedString);
impl Render for FallbackTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .max_w(px(MAX_WIDTH))
            .px(px(PAD_X))
            .py(px(PAD_Y))
            .rounded(px(RADIUS))
            .border_1()
            .border_color(crate::settings_style::card_border())
            .bg(crate::settings_style::card_bg())
            .font_weight(gpui::FontWeight::LIGHT)
            .text_size(px(TEXT_SIZE))
            .text_color(crate::palette::rgb(crate::shell::TEXT))
            .child(self.0.clone())
    }
}

/// Positions any tooltip view against its owner's painted bounds.
struct AnchoredTooltip {
    content: AnyView,
    owner: Option<Bounds<Pixels>>,
    pointer: Point<Pixels>,
    placement: Placement,
}

impl Render for AnchoredTooltip {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        // Without the owner's bounds (it has not painted yet), treat the
        // pointer as a zero-size control so the chip still centres on it.
        let owner = self
            .owner
            .unwrap_or(Bounds::new(self.pointer, Default::default()));
        let gap = px(GAP);
        let room_below = viewport.height - (owner.bottom() + gap);
        let room_above = owner.top() - gap;
        let below = place_below(self.placement, room_above, room_below);
        let center_x = owner.center().x;
        let (position, anchor) = if below {
            (
                Point::new(center_x, owner.bottom() + gap),
                gpui::Anchor::TopCenter,
            )
        } else {
            (
                Point::new(center_x, owner.top() - gap),
                gpui::Anchor::BottomCenter,
            )
        };
        div().size_0().child(
            gpui::anchored()
                .position(position)
                .anchor(anchor)
                .snap_to_window_with_margin(px(WINDOW_MARGIN))
                .child(self.content.clone()),
        )
    }
}

// Prefer the requested side when it has room; otherwise use the side with
// more space. This also handles windows smaller than the tooltip itself.
fn place_below(placement: Placement, room_above: Pixels, room_below: Pixels) -> bool {
    match placement {
        Placement::Below => room_below >= px(FLIP_ROOM) || room_below >= room_above,
        Placement::Above => room_above < px(FLIP_ROOM) && room_below > room_above,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_respects_preference_and_window_edges() {
        assert!(place_below(Placement::Below, px(100.0), px(100.0)));
        assert!(!place_below(Placement::Above, px(100.0), px(100.0)));
        assert!(!place_below(Placement::Below, px(100.0), px(5.0)));
        assert!(place_below(Placement::Above, px(5.0), px(100.0)));
        assert!(place_below(Placement::Below, px(5.0), px(10.0)));
        assert!(!place_below(Placement::Above, px(10.0), px(5.0)));
        assert!(place_below(Placement::Below, px(-5.0), px(-5.0)));
    }
}

/// `.tooltip(...)` builder for a text tooltip that follows the pointer.
/// Prefer [`TooltipExt::tip`], which centres on the control instead.
pub fn text_tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |window, cx| {
        if let Some(renderer) = cx.try_global::<TooltipRenderer>() {
            (renderer.0)(text.clone(), window, cx)
        } else {
            cx.new(|_| FallbackTooltip(text.clone())).into()
        }
    }
}

/// Control-anchored tooltips for any stateful element that takes children.
pub trait TooltipExt: StatefulInteractiveElement + ParentElement + Sized {
    /// A one-line note centred below the control (above near the bottom
    /// edge of the window).
    fn tip(self, text: impl Into<SharedString>) -> Self {
        let text: SharedString = text.into();
        self.anchored_tooltip(Placement::Below, text_tooltip(text))
    }

    /// Like [`TooltipExt::tip`], preferring the side above the control, for
    /// controls with content underneath the chip would otherwise cover.
    fn tip_above(self, text: impl Into<SharedString>) -> Self {
        let text: SharedString = text.into();
        self.anchored_tooltip(Placement::Above, text_tooltip(text))
    }

    /// Anchor any tooltip view to this control rather than to the pointer.
    fn anchored_tooltip(
        self,
        placement: Placement,
        build: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::default();
        let probe = bounds.clone();
        self.child(
            canvas(move |owner, _, _| probe.set(Some(owner)), |_, _, _, _| {})
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
        )
        .tooltip(move |window, cx| {
            let content = build(window, cx);
            let owner = bounds.get();
            let pointer = window.mouse_position();
            cx.new(|_| AnchoredTooltip {
                content,
                owner,
                pointer,
                placement,
            })
            .into()
        })
    }
}

impl<E: StatefulInteractiveElement + ParentElement + Sized> TooltipExt for E {}
