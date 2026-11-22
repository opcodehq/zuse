//! Compact status card for the sidebar update control.

use crate::theme::Theme;
use gpui::{
    Context, FontWeight, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::*, px,
};

pub(super) struct UpdateNotesCard {
    pub title: SharedString,
    pub status: SharedString,
    pub version: String,
}

impl Render for UpdateNotesCard {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let url = zuse_update::release_page(&self.version);
        let hover_text = theme.text;
        let card = div()
            .occlude()
            .w(px(260.0))
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(theme.border)
            .bg(crate::popover::surface_bg(theme))
            .text_size(crate::typography::text_xs())
            .text_color(theme.text_muted)
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(self.title.clone()),
            )
            .child(self.status.clone())
            .child(
                div()
                    .id("update-notes-release-page")
                    .mt(px(4.0))
                    .cursor_pointer()
                    .hover(move |el| el.text_color(hover_text).underline())
                    .child(format!("What's new in v{} ↗", self.version))
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        crate::frost::frosted(6.0, crate::frost::MENU_BLUR, card)
    }
}
