//! Shared dimensions for Zuse's settings frames, cards and compact rows.
use crate::palette::{rgb, tone};
use crate::shell::{BORDER, MUTED, TEXT};
use gpui::{prelude::*, *};
// Values come directly from Zuse's app-palette.css and settings-panel.tsx.
pub const FRAME_BG: Hsla = Hsla {
    h: 132. / 360.,
    s: 0.05,
    l: 0.14,
    a: 1.,
};
pub const CARD_BG: Hsla = Hsla {
    h: 136. / 360.,
    s: 0.05,
    l: 0.075,
    a: 1.,
};
pub const SIDEBAR_BG: Hsla = Hsla {
    h: 138. / 360.,
    s: 0.05,
    l: 0.0575,
    a: 1.,
};
pub const SIDEBAR_ACTIVE: Hsla = Hsla {
    h: 130. / 360.,
    s: 0.06,
    l: 0.135,
    a: 1.,
};
pub const SIDEBAR_BORDER: Hsla = Hsla {
    h: 136. / 360.,
    s: 0.05,
    l: 0.085,
    a: 1.,
};
pub const CARD_BORDER: Hsla = Hsla {
    h: 0.,
    s: 0.,
    l: 1.,
    a: 0.08,
};
pub const CARD_RADIUS: f32 = 10.;
pub const FRAME_RADIUS: f32 = 12.;
pub const ROW_PAD_X: f32 = 12.;
pub const ROW_PAD_Y: f32 = 10.;
pub const ROW_GAP: f32 = 10.;
pub const TITLE_SIZE: f32 = 12.;
pub const DESCRIPTION_SIZE: f32 = 11.;
pub const CONTROL_HEIGHT: f32 = 28.;
pub fn frame_bg() -> Hsla {
    tone(FRAME_BG, hsla(105. / 360., 0.18, 0.94, 1.))
}
pub fn card_bg() -> Hsla {
    tone(CARD_BG, hsla(0., 0., 1., 1.))
}
pub fn sidebar_bg() -> Hsla {
    tone(SIDEBAR_BG, hsla(105. / 360., 0.22, 0.95, 1.))
}
pub fn sidebar_active() -> Hsla {
    tone(SIDEBAR_ACTIVE, hsla(106. / 360., 0.18, 0.89, 1.))
}
pub fn sidebar_border() -> Hsla {
    tone(SIDEBAR_BORDER, hsla(110. / 360., 0.13, 0.89, 1.))
}
pub fn card_border() -> Hsla {
    tone(CARD_BORDER, hsla(110. / 360., 0.13, 0.89, 1.))
}
pub fn frame() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(FRAME_RADIUS))
        .bg(frame_bg())
        .p(px(4.))
}
pub fn header(title: impl Into<SharedString>, description: impl Into<SharedString>) -> Div {
    let description = description.into();
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
                .when(!description.is_empty(), |view| {
                    view.child(description_text(description))
                }),
        )
}
pub fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(CARD_RADIUS))
        .border_1()
        .border_color(card_border())
        .bg(card_bg())
        .overflow_hidden()
}
pub fn setting_row(first: bool) -> Div {
    div()
        .flex()
        .items_start()
        .flex_wrap()
        .gap(px(ROW_GAP))
        .px(px(ROW_PAD_X))
        .py(px(ROW_PAD_Y))
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
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(title_text(title))
        .child(description_text(description))
}
// GPUI activates focused clickable controls on key release. Contain the key
// here so parent menu handlers cannot also apply it on key down.
fn contain_activation(event: &KeyDownEvent, _: &mut Window, cx: &mut App) {
    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
        cx.stop_propagation();
    }
}
pub fn button(id: impl Into<ElementId>, name: impl Into<SharedString>) -> Stateful<Div> {
    let name = name.into();
    div()
        .id(id)
        .role(Role::Button)
        .on_key_down(contain_activation)
        .aria_label(name.clone())
        .cursor_pointer()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(CONTROL_HEIGHT))
        .px(px(16.))
        .rounded(px(7.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(card_bg())
        .text_size(px(11.))
        .text_color(rgb(TEXT))
        .hover(|button| button.bg(frame_bg()))
        .child(name)
}
pub fn field() -> Div {
    div()
        .flex_1()
        .min_w(px(140.))
        .flex()
        .items_center()
        .h(px(28.))
        .px(px(16.))
        .rounded(px(7.))
        .border_1()
        .border_color(hsla(128. / 360., 0.05, 0.17, 1.))
        .bg(card_bg())
}

pub fn toggle(id: impl Into<ElementId>, checked: bool, label: &str) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Switch)
        .on_key_down(contain_activation)
        .aria_label(label.to_owned())
        .aria_toggled(if checked {
            Toggled::True
        } else {
            Toggled::False
        })
        .relative()
        .flex_none()
        .w(px(32.))
        .h(px(20.))
        .cursor_pointer()
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded_full()
                .border_1()
                .border_color(rgb(if checked { 0x89ca21 } else { BORDER }))
                .bg(rgb(if checked {
                    0x89ca21
                } else {
                    crate::shell::RAISED
                })),
        )
        .child(
            div()
                .absolute()
                .top(px(3.))
                .left(px(if checked { 16. } else { 2. }))
                .size(px(14.))
                .rounded_full()
                .bg(if crate::palette::is_light() {
                    gpui::white()
                } else {
                    gpui::Hsla::from(rgb(TEXT))
                })
                .border_1()
                .border_color(gpui::black().opacity(if crate::palette::is_light() {
                    0.1
                } else {
                    0.3
                })),
        )
}

/// Compact choice used by appearance and motion settings.
pub fn choice(id: impl Into<ElementId>, label: &'static str, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::RadioButton)
        .aria_label(label)
        .aria_selected(selected)
        .aria_toggled(if selected {
            Toggled::True
        } else {
            Toggled::False
        })
        .tab_index(0)
        .on_key_down(contain_activation)
        .cursor_pointer()
        .h(px(28.))
        .px(px(10.))
        .rounded(px(7.))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(if selected { TEXT } else { MUTED }))
        .when(selected, |view| view.bg(frame_bg()))
        .focus_visible(|style| style.bg(frame_bg()))
        .child(label)
}
