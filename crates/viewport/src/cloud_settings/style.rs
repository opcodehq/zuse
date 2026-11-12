//! Compact Cloud frames and controls shared by hosting, billing and setup.
use crate::tooltip::TooltipExt as _;
use crate::{
    assets::icon,
    palette, settings_style as s,
    shell::{BORDER, MUTED, TEXT},
};
use gpui::{prelude::*, *};

fn badge_base(warning: bool, outline: bool) -> Div {
    let bg = if outline {
        s::frame_bg().opacity(0.5)
    } else if warning {
        palette::tone(
            hsla(20.8 / 360., 0.289, 0.176, 1.),
            hsla(22.1 / 360., 1., 0.963, 1.),
        )
    } else {
        palette::tone(
            hsla(132.9 / 360., 0.359, 0.153, 1.),
            hsla(130. / 360., 0.5, 0.953, 1.),
        )
    };
    let text = if outline {
        palette::rgb(TEXT).into()
    } else if warning {
        hsla(21.6 / 360., 0.982, 0.567, 1.)
    } else {
        palette::tone(
            hsla(133.9 / 360., 0.606, 0.488, 1.),
            hsla(131.4 / 360., 0.734, 0.31, 1.),
        )
    };
    div()
        .flex_none()
        .h(px(16.))
        .min_w(px(16.))
        .px(px(6.))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.))
        .line_height(px(13.3333))
        .font_weight(FontWeight::MEDIUM)
        .bg(bg)
        .text_color(text)
}
pub(crate) fn badge(label: impl Into<SharedString>, warning: bool, outline: bool) -> Div {
    badge_base(warning, outline).child(div().flex_none().child(label.into()))
}
pub(crate) fn badge_with_icon(
    label: impl Into<SharedString>,
    glyph: &'static str,
    window: &Window,
) -> Div {
    let label = label.into();
    let mut font = window.text_style().font();
    font.weight = FontWeight::MEDIUM;
    let line = window.text_system().shape_line(
        label.clone(),
        px(10.),
        &[TextRun {
            len: label.len(),
            font,
            color: window.text_style().color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    // Reserve the shaped label, icon, gap and padding together in both layout hosts.
    badge_base(false, false)
        .w(line.width() + px(26.))
        .whitespace_nowrap()
        .gap(px(4.))
        .child(
            icon(glyph)
                .size(px(10.))
                .text_color(palette::tone(
                    hsla(133.9 / 360., 0.606, 0.488, 1.),
                    hsla(131.4 / 360., 0.734, 0.31, 1.),
                ))
                .opacity(0.8),
        )
        .child(div().flex_none().child(label))
}
pub(crate) fn group(
    title: &'static str,
    help: Option<&'static str>,
    action: Option<AnyElement>,
    body: Div,
) -> Div {
    let mut title_row = div()
        .flex()
        .items_center()
        .gap(px(4.))
        .child(s::title_text(title));
    if let Some(help) = help {
        title_row = title_row.child(
            div()
                .id(SharedString::from(format!("cloud-help-{title}")))
                .role(Role::Button)
                .aria_label(title)
                .aria_description(help)
                .tab_index(0)
                .size(px(28.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .text_color(palette::rgb(MUTED))
                .hover(|v| v.text_color(palette::rgb(TEXT)))
                .child(icon("information").size(px(12.)))
                .tip(help),
        );
    }
    let mut header = div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(12.))
        .px(px(10.))
        .py(px(8.))
        .child(div().min_w_0().flex_1().child(title_row));
    if let Some(action) = action {
        header = header.child(
            div()
                .h(px(28.))
                .flex_none()
                .flex()
                .items_center()
                .child(action),
        );
    }
    s::frame().child(header).child(s::card().child(body))
}
pub(crate) fn setting_row(
    first: bool,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    action: Option<AnyElement>,
) -> Div {
    let description = description.into();
    let mut copy = div().flex_1().min_w_0().child(s::title_text(title));
    if !description.is_empty() {
        copy = copy.child(div().mt(px(2.)).child(s::description_text(description)));
    }
    let mut row = div()
        .flex()
        .items_start()
        .gap(px(10.))
        .px(px(12.))
        .py(px(10.))
        .when(!first, |v| {
            v.border_t_1().border_color(palette::rgb(BORDER))
        })
        .child(copy);
    if let Some(action) = action {
        row = row.child(
            div()
                .h(px(28.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(action),
        );
    }
    row
}
pub(crate) fn button(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    primary: bool,
    disabled: bool,
    busy: bool,
) -> Stateful<Div> {
    button_base(id, text, primary, disabled, busy, false)
}
fn button_base(
    id: impl Into<ElementId>,
    text: impl Into<SharedString>,
    primary: bool,
    disabled: bool,
    busy: bool,
    icon_only: bool,
) -> Stateful<Div> {
    let text = text.into();
    let lime = palette::tone(
        hsla(83. / 360., 0.72, 0.46, 1.),
        hsla(83. / 360., 0.74, 0.43, 1.),
    );
    let mut view = div()
        .id(id)
        .role(Role::Button)
        .aria_label(text.clone())
        .tab_index(if disabled || busy { -1 } else { 0 })
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.))
        .px(px(11.))
        .rounded(px(7.))
        .border_1()
        .border_color(if primary { lime } else { transparent_black() })
        .text_size(px(10.))
        .line_height(px(13.3333))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if primary {
            hsla(132. / 360., 0.3, 0.07, 1.)
        } else {
            palette::rgb(TEXT).into()
        })
        .when(primary, |v| v.bg(lime))
        .when(disabled, |v| v.opacity(0.64))
        .when(!disabled && !busy, |v| {
            v.cursor_pointer().hover(move |v| {
                if primary {
                    v.bg(lime.opacity(0.9))
                } else {
                    v.bg(s::frame_bg())
                }
            })
        })
        .focus_visible(|v| {
            v.border_2()
                .border_color(palette::rgb(0x89ca21))
                .px(px(if icon_only { 0. } else { 10. }))
        })
        .on_key_down(|event: &KeyDownEvent, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
            }
        })
        .child(div().when(busy, |v| v.opacity(0.)).child(text));
    if busy {
        view = view.child(
            div()
                .absolute()
                .left(relative(0.5))
                .top(relative(0.5))
                .ml(px(-6.))
                .mt(px(-6.))
                .child(
                    icon("loading")
                        .size(px(12.))
                        .text_color(if primary {
                            hsla(132. / 360., 0.3, 0.07, 1.)
                        } else {
                            palette::rgb(TEXT).into()
                        })
                        .with_animation(
                            "cloud-button-spinner",
                            Animation::new(std::time::Duration::from_millis(1000)).repeat(),
                            |svg, t| svg.with_transformation(Transformation::rotate(percentage(t))),
                        ),
                ),
        );
    }
    view
}
pub(crate) fn note(text: &str, error: bool) -> Div {
    div()
        .px(px(12.))
        .py(px(10.))
        .text_size(px(11.))
        .line_height(px(16.))
        .text_color(if error {
            s::destructive_text()
        } else {
            palette::rgb(MUTED).into()
        })
        .child(text.to_owned())
}

pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    glyph: &'static str,
    label: impl Into<SharedString>,
    disabled: bool,
    busy: bool,
) -> Stateful<Div> {
    button_base(id, "", false, disabled, busy, true)
        .w(px(28.))
        .px(px(0.))
        .aria_label(label.into())
        .child(icon(glyph).size(px(14.)).when(busy, |v| v.opacity(0.)))
}
pub(crate) fn icon_label_button(
    id: impl Into<ElementId>,
    glyph: &'static str,
    label: impl Into<SharedString>,
    primary: bool,
    disabled: bool,
    busy: bool,
) -> Stateful<Div> {
    button(id, label, primary, disabled, busy)
        .flex_row_reverse()
        .gap(px(6.))
        .child(
            icon(glyph)
                .size(px(14.))
                .when(primary, |v| v.text_color(hsla(132. / 360., 0.3, 0.07, 1.)))
                .when(busy, |v| v.opacity(0.)),
        )
}
pub(crate) fn error_badge(label: impl Into<SharedString>) -> Div {
    badge(label, false, false)
        .bg(palette::tone(
            hsla(349.3 / 360., 0.311, 0.176, 1.),
            hsla(0., 1., 0.973, 1.),
        ))
        .text_color(s::destructive_text())
}

pub(crate) fn setting_row_content(
    first: bool,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    action: Option<AnyElement>,
    extra: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .px(px(12.))
        .py(px(10.))
        .gap(px(8.))
        .when(!first, |v| {
            v.border_t_1().border_color(palette::rgb(BORDER))
        })
        .child(
            setting_row(true, title, description, action)
                .px(px(0.))
                .py(px(0.)),
        )
        .child(extra)
}
