//! Sizes the single Threads list so its "Show more" lands right above the
//! sidebar footer at any window height: as many threads as fit, no scroll,
//! and the button pushed down onto the footer line.
use std::cell::Cell;

use gpui::{Pixels, ScrollHandle, Window, px};

/// Fewest threads the preview shows, however short the window.
const MIN_PREVIEW: usize = 3;

/// The last frame's single list and the preview it settled on.
#[derive(Default)]
pub(super) struct SidebarFit {
    limit: Cell<Option<usize>>,
    /// Space above "Show more" that pins it to the bottom of the list.
    toggle_margin: Cell<f32>,
    shown: Cell<usize>,
    total: Cell<usize>,
    expanded: Cell<bool>,
}

impl SidebarFit {
    /// Threads the collapsed list shows; `fallback` until the first measure.
    pub(super) fn limit(&self, fallback: usize) -> usize {
        self.limit.get().unwrap_or(fallback)
    }

    /// Space to leave above "Show more" so it sits on the footer line.
    pub(super) fn toggle_margin(&self) -> f32 {
        self.toggle_margin.get()
    }

    /// What the list drew this frame, for [`Self::measure`] to compare.
    pub(super) fn record(&self, shown: usize, total: usize, expanded: bool) {
        self.shown.set(shown);
        self.total.set(total);
        self.expanded.set(expanded);
    }

    /// Settles the limit and the button's margin from where the list ended
    /// (`list_bottom`, window coordinates) against the bottom of the scroll
    /// viewport. Asks for one more frame only when either moved, so it
    /// converges and stops.
    pub(super) fn measure(
        &self,
        list_bottom: Pixels,
        scroll: &ScrollHandle,
        row_pitch: f32,
        toggle_pitch: f32,
        window: &mut Window,
    ) {
        let (shown, total) = (self.shown.get(), self.total.get());
        let viewport = scroll.bounds();
        if self.expanded.get() || total == 0 || viewport.size.height <= px(0.) {
            if self.toggle_margin.replace(0.) != 0. {
                window.refresh();
            }
            return;
        }
        // Free space under the list, as if unscrolled and with no margin.
        let bottom = f32::from(list_bottom - scroll.offset().y) - self.toggle_margin.get();
        let mut free = f32::from(viewport.bottom()) - bottom;
        if shown >= total {
            // Hiding a row would add the button.
            free -= toggle_pitch;
        }
        let fit = (shown as isize + (free / row_pitch).floor() as isize).max(MIN_PREVIEW as isize)
            as usize;
        let current = self.limit.get();
        let (limit, margin) = if fit >= total {
            // Everything fits: keep a larger limit so it never creeps.
            (current.map_or(total, |limit| limit.max(total)), 0.)
        } else {
            let leftover = free - (fit as f32 - shown as f32) * row_pitch;
            (fit, leftover.max(0.).floor())
        };
        let moved = current != Some(limit) || (self.toggle_margin.get() - margin).abs() >= 1.;
        if moved {
            self.limit.set(Some(limit));
            self.toggle_margin.set(margin);
            window.refresh();
        }
    }
}
