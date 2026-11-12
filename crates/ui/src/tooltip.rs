//! Shared tooltip appearance for the native host.

use crate::theme::Theme;
use gpui::{AnyElement, App, Context, Div, SharedString, Window, div, prelude::*, px};

pub use zuse_viewport::tooltip::{
    GAP, MAX_WIDTH, PAD_X, PAD_Y, Placement, RADIUS, TEXT_SIZE, TooltipExt, text_tooltip,
};

/// The bare chip surface — border, fill, radius, padding and type — for
/// tooltips that lay out richer content than one line of text. Wrap the
/// finished card in [`frost`].
pub fn surface(theme: &Theme) -> Div {
    div()
        .px(px(PAD_X))
        .py(px(PAD_Y))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(theme.border)
        .bg(crate::popover::surface_bg(theme))
        .font_family(theme.font_sans.clone())
        .text_size(px(TEXT_SIZE))
        .text_color(theme.text_strong.opacity(0.92))
}

/// Backdrop blur behind a [`surface`] card.
pub fn frost(card: impl IntoElement) -> AnyElement {
    crate::frost::frosted(RADIUS, crate::frost::MENU_BLUR, card).into_any_element()
}

/// A finished one-line chip reading `text`.
pub fn chip(text: impl Into<SharedString>, theme: &Theme) -> AnyElement {
    frost(
        surface(theme)
            .max_w(px(MAX_WIDTH))
            .font_weight(gpui::FontWeight::MEDIUM)
            .child(text.into()),
    )
}

/// A one-line hover note in the shared chip.
pub struct TextTooltip {
    text: SharedString,
}

impl TextTooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl Render for TextTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        chip(self.text.clone(), Theme::of(cx))
    }
}

/// Install the native theme-aware renderer for tooltips in shared views.
pub fn install(cx: &mut App) {
    cx.set_global(zuse_viewport::tooltip::TooltipRenderer(|text, _, cx| {
        cx.new(|_| TextTooltip::new(text)).into()
    }));
}
