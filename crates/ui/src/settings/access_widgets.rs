//! Shared native remote-access card layout.
use crate::{composer::ComposerInput, settings::widgets, theme::Theme};
use gpui::{prelude::*, *};
pub(super) fn field(theme: &Theme, input: Entity<ComposerInput>) -> Div {
    div()
        .flex_1()
        .min_w(px(180.))
        .px(px(10.))
        .py(px(4.))
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.input_glass_bg())
        .child(input)
}
pub(super) fn details(theme: &Theme, title: &str, description: &str) -> Div {
    div()
        .flex_1()
        .min_w(px(160.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(widgets::row_title(theme, title.to_owned()))
        .child(
            div()
                .text_size(crate::typography::ui_rems(11.))
                .line_height(crate::typography::ui_rems(16.))
                .text_color(theme.text_muted)
                .child(description.to_owned()),
        )
}
