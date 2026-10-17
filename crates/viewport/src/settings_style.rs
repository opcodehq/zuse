//! Shared dimensions for Zuse's settings frames, cards and compact rows.
use crate::shell::{BORDER, MUTED, TEXT};
use gpui::{prelude::*, *};
pub const CARD_RADIUS: f32 = 8.;
pub const FRAME_RADIUS: f32 = 12.;
pub const ROW_PAD_X: f32 = 12.;
pub const ROW_PAD_Y: f32 = 10.;
pub const ROW_GAP: f32 = 10.;
pub const TITLE_SIZE: f32 = 12.;
pub const DESCRIPTION_SIZE: f32 = 11.;
pub const CONTROL_HEIGHT: f32 = 28.;
pub fn frame() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(FRAME_RADIUS))
        .bg(rgb(0x191919))
        .p(px(4.))
}
pub fn header(title: impl Into<SharedString>, description: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_start()
        .gap_3()
        .px(px(10.))
        .py(px(8.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(title_text(title))
                .child(description_text(description)),
        )
}
pub fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(0x101010))
        .overflow_hidden()
}
pub fn setting_row(first: bool) -> Div {
    div()
        .flex()
        .items_center()
        .flex_wrap()
        .gap(px(ROW_GAP))
        .px(px(ROW_PAD_X))
        .py(px(ROW_PAD_Y))
        .min_h(px(52.))
        .when(!first, |row| row.border_t_1().border_color(rgb(BORDER)))
}
pub fn title_text(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(TITLE_SIZE))
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(TEXT))
        .child(text.into())
}
pub fn description_text(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(DESCRIPTION_SIZE))
        .line_height(px(16.))
        .text_color(rgb(MUTED))
        .child(text.into())
}
pub fn details(title: impl Into<SharedString>, description: impl Into<SharedString>) -> Div {
    div()
        .flex_1()
        .min_w(px(160.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(title_text(title))
        .child(description_text(description))
}
pub fn button(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Stateful<Div> {
    let name = name.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(name.clone())
        .cursor_pointer()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(CONTROL_HEIGHT))
        .px(px(10.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(0x303030))
        .bg(rgb(0x222222))
        .text_size(px(11.))
        .text_color(rgb(TEXT))
        .hover(|button| button.bg(rgb(0x2c2c2c)))
        .child(name)
}
pub fn field() -> Div {
    div()
        .flex_1()
        .min_w(px(140.))
        .flex()
        .items_center()
        .h(px(32.))
        .px(px(10.))
        .rounded(px(6.))
        .border_1()
        .border_color(rgb(0x323232))
        .bg(rgb(0x171717))
}
