//! Shared provider rail, group headings and model rows for both compositor hosts.
use crate::{
    assets::icon,
    palette, settings_style as s,
    shell::{MUTED, TEXT},
};
use gpui::{prelude::*, *};

pub const WIDTH: f32 = 304.;
pub const MAX_HEIGHT: f32 = 320.;
pub const SEARCH_HEIGHT: f32 = 42.;
pub const LABEL_HEIGHT: f32 = 29.5;
pub const GROUP_HEADER_HEIGHT: f32 = 35.;
pub const ROW_HEIGHT: f32 = 28.;
pub const PROVIDER_ROW_HEIGHT: f32 = 45.16667;
pub const LIST_INSET: f32 = 6.;
const ACCENT: u32 = 0x89ca21;

pub fn section(label: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .h(px(LABEL_HEIGHT))
        .px(px(6.))
        .flex()
        .items_center()
        .text_size(px(12.))
        .line_height(px(16.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette::rgb(MUTED))
        .child(label.into())
}

pub fn provider_heading(
    name: impl Into<SharedString>,
    glyph: Svg,
    count: usize,
    current: bool,
) -> Div {
    div()
        .flex_none()
        .h(px(GROUP_HEADER_HEIGHT))
        .pt(px(8.))
        .border_t_1()
        .border_color(palette::rgb(0x1f211f).opacity(0.5))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(6.))
                .pt(px(6.))
                .pb(px(4.))
                .text_size(px(13.))
                .line_height(px(17.))
                .child(glyph.size(px(14.)).flex_none())
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(palette::rgb(TEXT))
                        .child(name.into()),
                )
                .when(current, |v| {
                    v.child(
                        div()
                            .rounded(px(4.))
                            .px(px(6.))
                            .py(px(1.))
                            .bg(palette::rgb(ACCENT).opacity(if palette::is_light() {
                                0.35
                            } else {
                                0.15
                            }))
                            .text_color(palette::rgb(ACCENT))
                            .text_size(px(9.))
                            .line_height(px(12.15))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::localization::element(
                                zuse_i18n::messages::native::current,
                            )),
                    )
                })
                .child(
                    div()
                        .ml_auto()
                        .text_size(px(10.))
                        .line_height(px(13.5))
                        .text_color(palette::rgb(MUTED))
                        .child(count.to_string()),
                ),
        )
}

pub fn provider_tab(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    active: bool,
    live: bool,
    glyph: Option<Svg>,
) -> Stateful<Div> {
    let label = label.into();
    let has_glyph = glyph.is_some();
    div()
        .id(id)
        .role(Role::Tab)
        .aria_label(label)
        .aria_selected(active)
        .tab_index(0)
        .relative()
        .size(px(32.))
        .border_1()
        .border_color(transparent_black())
        .flex_none()
        .rounded(px(7.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .line_height(px(14.85))
        .text_color(palette::rgb(if active { TEXT } else { MUTED }))
        .when(active, |v| selected(v))
        .when(!active, |v| {
            v.hover(|v| {
                v.bg(s::frame_bg().opacity(0.6))
                    .text_color(palette::rgb(TEXT))
            })
        })
        .when_some(glyph, |v, glyph| v.child(glyph.size(px(16.)).flex_none()))
        .when(!has_glyph, |v| {
            v.child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(crate::localization::element(
                        zuse_i18n::messages::providers::model_picker_all,
                    )),
            )
        })
        .when(live, |v| {
            v.child(
                div()
                    .absolute()
                    .right(px(4.))
                    .bottom(px(4.))
                    .size(px(6.))
                    .rounded_full()
                    .bg(palette::rgb(ACCENT)),
            )
        })
}

fn selected<E: Styled>(v: E) -> E {
    v.bg(s::frame_bg().opacity(0.6))
}

pub fn default_button(id: impl Into<ElementId>, is_default: bool) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(if is_default {
            "Default model"
        } else {
            "Make default model"
        })
        .tab_index(0)
        .size(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .text_color(palette::rgb(if is_default { ACCENT } else { MUTED }))
        .when(!is_default, |v| {
            v.opacity(0.)
                .group_hover("model-choice", |v| v.opacity(0.7))
        })
        .focus_visible(|v| v.opacity(1.))
        .hover(|v| v.opacity(1.).text_color(palette::rgb(TEXT)))
        .child(icon("star").size(px(14.)))
}

/// A persistent favorite control; starring a model is independent of selection.
pub fn favorite_button(id: impl Into<ElementId>, favorite: bool) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(if favorite {
            "Remove from favorites"
        } else {
            "Add to favorites"
        })
        .aria_toggled(if favorite {
            Toggled::True
        } else {
            Toggled::False
        })
        .tab_index(0)
        .size(px(22.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .text_color(palette::rgb(if favorite { ACCENT } else { MUTED }))
        .opacity(0.)
        .group_hover("model-choice", |v| v.opacity(1.))
        .focus_visible(|v| v.opacity(1.))
        .hover(|v| {
            v.bg(s::frame_bg())
                .text_color(palette::rgb(if favorite { ACCENT } else { TEXT }))
        })
        .child(
            icon(if favorite {
                "model-favorite-filled"
            } else {
                "star"
            })
            .size(px(14.)),
        )
}

pub fn model_row(
    id: impl Into<ElementId>,
    title: impl Into<SharedString>,
    provider: Option<SharedString>,
    glyph: Svg,
    is_selected: bool,
    active: bool,
    shortcut: Option<usize>,
    star: impl IntoElement,
) -> Stateful<Div> {
    div()
        .id(id)
        .group("model-choice")
        .role(Role::Button)
        .tab_index(0)
        .aria_selected(is_selected)
        .relative()
        .h(px(if provider.is_some() {
            PROVIDER_ROW_HEIGHT
        } else {
            ROW_HEIGHT
        }))
        .w_full()
        .border_1()
        .border_color(transparent_black())
        .px(px(6.))
        .py(px(0.))
        .rounded(px(7.))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(palette::rgb(TEXT))
        .when(!is_selected && active, |v| v.bg(s::frame_bg().opacity(0.6)))
        .hover(|v| {
            v.bg(if is_selected {
                palette::rgb(ACCENT).opacity(0.12).into()
            } else {
                s::frame_bg().opacity(0.6)
            })
        })
        .child(glyph.size(px(16.)).flex_none())
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::NORMAL)
                        .line_height(px(16.5))
                        .child(title.into()),
                )
                .when_some(provider, |v, name| {
                    v.child(
                        div()
                            .truncate()
                            .text_size(px(11.))
                            .line_height(px(14.66667))
                            .text_color(palette::rgb(MUTED))
                            .child(name),
                    )
                }),
        )
        .when(is_selected, |v| {
            v.child(
                icon("check")
                    .size(px(14.))
                    .flex_none()
                    .text_color(palette::rgb(TEXT)),
            )
        })
        .when_some(shortcut, |v, n| {
            v.child(
                div()
                    .flex_none()
                    .ml(px(2.))
                    .min_w(px(if cfg!(target_os = "macos") { 26. } else { 44. }))
                    .rounded(px(4.))
                    .px(px(4.))
                    .bg(s::frame_bg().opacity(0.7))
                    .text_size(px(10.))
                    .line_height(px(13.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_center()
                    .text_color(palette::rgb(MUTED))
                    .child(if cfg!(target_os = "macos") {
                        format!("⌘{n}")
                    } else {
                        format!("Ctrl+{n}")
                    }),
            )
        })
        .child(star)
}
