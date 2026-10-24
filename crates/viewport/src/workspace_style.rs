//! Panel dimensions and surface wrappers used by the workspace shell.
use gpui::{Div, Styled, div, px};

pub const SIDEBAR_DEFAULT: f32 = 232.;
pub const SIDEBAR_MIN: f32 = 200.;
pub const SIDEBAR_MAX: f32 = 300.;
pub const GUTTER: f32 = 4.;
pub const RADIUS: f32 = 8.;

pub fn column() -> Div {
    div()
        .h_full()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .py(px(GUTTER))
        .gap(px(GUTTER))
}

pub fn surface() -> Div {
    div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .rounded(px(RADIUS))
        .overflow_hidden()
        .bg(crate::palette::rgb(crate::shell::BG))
}
