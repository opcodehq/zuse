//! Shared compact controls for selecting where a new chat runs.
use crate::{
    assets::icon,
    palette, settings_style as s,
    shell::{BORDER, MUTED, TEXT},
};
use gpui::{prelude::*, *};
pub fn chip(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    glyph: &str,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .border_1()
        .border_color(transparent_black())
        .h(px(28.))
        .min_w_0()
        .max_w(px(224.))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .rounded_full()
        .text_size(px(11.))
        .line_height(px(14.85))
        .font_weight(FontWeight::NORMAL)
        .text_color(palette::rgb(TEXT))
        .cursor_pointer()
        .hover(|v| v.bg(s::frame_bg()))
        .focus_visible(|v| {
            v.bg(s::frame_bg())
                .border_1()
                .border_color(palette::rgb(0x89ca21))
        })
        .on_key_down(|e: &KeyDownEvent, _, cx| {
            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation()
            }
        })
        .child(icon(glyph).size(px(if glyph == "cloud-dither" { 16. } else { 14. })))
        .child(div().min_w_0().truncate().child(label))
        .child(icon("alt-arrow-down").size(px(12.)).opacity(0.6))
}
pub fn selection(selected: bool) -> Div {
    div()
        .size(px(14.))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(if selected {
            palette::rgb(0x89ca21).opacity(0.7)
        } else {
            palette::rgb(TEXT).opacity(0.2)
        })
        .bg(if selected {
            Hsla::from(palette::rgb(0x89ca21)).opacity(0.1)
        } else {
            s::card_bg().opacity(0.6)
        })
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |v| {
            v.child(div().size(px(6.)).rounded_full().bg(palette::rgb(0x89ca21)))
        })
}
pub fn option(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    glyph: &str,
    selected: bool,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .role(Role::RadioButton)
        .aria_label(label.clone())
        .aria_selected(selected)
        .tab_index(0)
        .h(px(28.))
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(8.))
        .rounded(px(7.))
        .text_size(px(12.))
        .line_height(px(20.))
        .text_color(palette::rgb(TEXT))
        .when(selected, |v| v.bg(s::frame_bg().opacity(0.4)))
        .cursor_pointer()
        .hover(|v| v.bg(s::frame_bg().opacity(0.6)))
        .focus_visible(|v| v.bg(s::frame_bg()))
        .on_key_down(|e: &KeyDownEvent, _, cx| {
            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation()
            }
        })
        .when(!glyph.is_empty(), |v| {
            v.child(
                icon(glyph)
                    .size(px(if glyph == "cloud-dither" { 16. } else { 14. }))
                    .opacity(0.8),
            )
        })
        .child(div().flex_1().min_w_0().truncate().child(label))
}
pub fn menu(width: f32) -> Div {
    div()
        .w(px(width))
        .max_h(px(400.))
        .p(px(7.))
        .rounded(px(10.))
        .border_1()
        .border_color(palette::rgb(BORDER))
        .bg(palette::tone(hsla(132. / 360., 0.05, 0.105, 1.), white()))
        .shadow_lg()
        .flex()
        .flex_col()
        .text_color(palette::rgb(TEXT))
}
pub fn separator() -> Div {
    div().h(px(1.)).my(px(4.)).bg(palette::rgb(BORDER))
}
pub fn meta(label: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .text_size(px(10.))
        .line_height(px(13.5))
        .text_color(palette::rgb(MUTED))
        .child(label.into())
}

pub fn workspace_heading() -> Div {
    meta("WORKSPACE")
        .px(px(8.))
        .pt(px(6.))
        .pb(px(4.))
        .font_weight(FontWeight::MEDIUM)
}

pub fn workspace_option(
    id: impl Into<ElementId>,
    worktree: bool,
    selected: bool,
    active: bool,
) -> Stateful<Div> {
    let (title, description, glyph) = if worktree {
        ("Worktree", "Fresh isolated branch", "git-branch")
    } else {
        ("Local", "Use the main checkout", "folder")
    };
    div()
        .id(id)
        .role(Role::RadioButton)
        .aria_label(title)
        .aria_selected(selected)
        .tab_index(0)
        .min_h(px(28.))
        .px(px(8.))
        .py(px(4.))
        .rounded(px(7.))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_color(palette::rgb(TEXT))
        .when(active, |v| v.bg(s::frame_bg()))
        .hover(|v| v.bg(s::frame_bg()))
        .focus_visible(|v| v.bg(s::frame_bg()))
        .child(icon(glyph).size(px(14.)).flex_none())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(meta(description).line_height(px(100. / 7.)).truncate()),
        )
        .child(selection(selected))
}

pub fn workspace_popup() -> Div {
    menu(224.).p(px(8.)).border_0().rounded(px(10.))
}
