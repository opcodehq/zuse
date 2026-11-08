//! Anchored dropdown menus and centered dialogs shared by browser controls.
use crate::{
    assets::icon,
    palette::rgb,
    shell::{BORDER, MUTED, RAISED, TEXT},
};
use gpui::{prelude::*, *};

pub(crate) const DESTRUCTIVE: u32 = 0xec9292;

/// The floating surface of a dropdown menu.
pub(crate) fn menu(id: impl Into<ElementId>, width: f32) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Menu)
        .w(px(width))
        .p(px(4.))
        .flex()
        .flex_col()
        .rounded(px(10.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(rgb(RAISED))
        .shadow_lg()
        .occlude()
}

/// One menu row: icon, label and an optional trailing hint.
pub(crate) fn item(
    id: impl Into<ElementId>,
    glyph: &'static str,
    label: impl Into<SharedString>,
    destructive: bool,
) -> Stateful<Div> {
    let label = label.into();
    let color = rgb(if destructive { DESTRUCTIVE } else { TEXT });
    div()
        .id(id)
        .role(Role::MenuItem)
        .aria_label(label.clone())
        .tab_index(0)
        .cursor_pointer()
        .h(px(30.))
        .px(px(8.))
        .flex()
        .items_center()
        .gap(px(9.))
        .rounded(px(7.))
        .text_size(px(13.))
        .text_color(color)
        .hover(|d| d.bg(crate::settings_style::frame_bg()))
        .focus_visible(|d| d.bg(crate::settings_style::frame_bg()))
        .child(
            icon(glyph)
                .size(px(14.))
                .flex_none()
                .text_color(if destructive { color } else { rgb(MUTED) }),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
}

pub(crate) fn separator() -> Div {
    div().h(px(1.)).mx(px(4.)).my(px(4.)).bg(rgb(BORDER))
}

/// Floats `content` at a window position above everything else.
pub(crate) fn anchored_at(position: Point<Pixels>, content: impl IntoElement) -> AnyElement {
    deferred(
        anchored()
            .anchor(Anchor::TopLeft)
            .position(position)
            .snap_to_window_with_margin(px(8.))
            .child(content),
    )
    .priority(3)
    .into_any_element()
}

/// Dimmed backdrop with a centered card; the caller adds its content.
pub(crate) fn dialog(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    width: f32,
) -> (Div, Stateful<Div>) {
    let backdrop = div()
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .p_4()
        .bg(black().opacity(0.5))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
    let card = div()
        .id(id)
        .role(Role::Dialog)
        .aria_label(label.into())
        .w(px(width))
        .max_w_full()
        .max_h(relative(0.9))
        .flex()
        .flex_col()
        .rounded(px(14.))
        .border_1()
        .border_color(rgb(BORDER))
        .bg(crate::settings_style::card_bg())
        .shadow_lg()
        .overflow_hidden();
    (backdrop, card)
}

/// Dialog buttons: filled primary, quiet secondary, or destructive outline.
pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: Kind,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .cursor_pointer()
        .h(px(30.))
        .px(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .map(|d| match kind {
            Kind::Primary => d
                .bg(rgb(TEXT))
                .text_color(crate::settings_style::card_bg())
                .hover(|d| d.opacity(0.88)),
            Kind::Secondary => d
                .border_1()
                .border_color(rgb(BORDER))
                .text_color(rgb(TEXT))
                .hover(|d| d.bg(crate::settings_style::frame_bg())),
            Kind::Destructive => d
                .border_1()
                .border_color(rgb(BORDER))
                .text_color(rgb(DESTRUCTIVE))
                .hover(|d| d.bg(crate::settings_style::frame_bg())),
        })
        .child(label)
}
#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Primary,
    Secondary,
    Destructive,
}
