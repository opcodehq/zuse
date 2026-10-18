//! Small building blocks shared by the catalog surfaces: the Plugins page,
//! Settings → Plugins and the Add ACP agent dialog. Sizes follow the settings
//! spec (28px controls, 7px radii, 12px button text).
use crate::theme::Theme;
use gpui::{AnyElement, SharedString, Stateful, div, prelude::*, px};

/// The muted fill behind search fields, segmented tabs and secondary buttons.
pub fn control_fill(theme: &Theme) -> gpui::Hsla {
    theme.settings_frame_bg().opacity(0.55)
}

/// A 28px search field: magnifier, then the caller's single-line input.
pub fn search_field(theme: &Theme, width: Option<f32>, input: AnyElement) -> gpui::Div {
    div()
        .h(px(28.))
        .when_some(width, |el, width| el.w(px(width)))
        .when(width.is_none(), |el| el.flex_1())
        .min_w_0()
        .px(px(10.))
        .rounded(px(7.))
        .bg(control_fill(theme))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(crate::typography::ui_rems(12.))
        .child(
            crate::icons::icon(crate::icons::MAGNIFER)
                .size(px(14.))
                .flex_none()
                .text_color(theme.text_muted),
        )
        .child(div().flex_1().min_w_0().overflow_hidden().child(input))
}

/// Secondary button: 28px, card tone, 12px medium label.
pub fn secondary_button(theme: &Theme, id: impl Into<gpui::ElementId>) -> Stateful<gpui::Div> {
    let hover = theme.glass_hover();
    div()
        .id(id)
        .flex_none()
        .h(px(28.))
        .px(px(14.))
        .rounded(px(7.))
        .bg(control_fill(theme))
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(crate::typography::ui_rems(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text)
        .cursor_pointer()
        .role(gpui::Role::Button)
        .tab_index(0)
        .hover(move |s| s.bg(hover))
}

/// Primary (accent) button: 28px, accent fill.
pub fn primary_button(theme: &Theme, id: impl Into<gpui::ElementId>) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .h(px(28.))
        .px(px(12.))
        .rounded(px(7.))
        .bg(theme.accent)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .text_size(crate::typography::ui_rems(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.on_accent)
        .cursor_pointer()
        .role(gpui::Role::Button)
        .tab_index(0)
        .hover(|s| s.opacity(0.9))
}

/// A square 28px ghost icon button.
pub fn icon_button(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    icon: &'static str,
    label: &'static str,
) -> Stateful<gpui::Div> {
    let text = theme.text;
    let hover = theme.glass_hover();
    let id: gpui::ElementId = id.into();
    let group = SharedString::from(format!("icon-button-{id}"));
    div()
        .id(id)
        .group(group.clone())
        .flex_none()
        .size(px(28.))
        .rounded(px(7.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme.text_muted)
        .cursor_pointer()
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(0)
        .hover(move |s| s.bg(hover))
        .child(
            crate::icons::icon(icon)
                .size(px(14.))
                .text_color(theme.text_muted)
                .group_hover(group, |s| s.text_color(text)),
        )
}

/// Segmented tabs: a 28px muted track holding 24px tabs, each a label and an
/// optional muted count. The caller wires each tab's click by index.
pub fn segmented_tabs(
    theme: &Theme,
    id: &'static str,
    tabs: &[(&str, Option<usize>)],
    selected: usize,
    mut on_select: impl FnMut(usize, Stateful<gpui::Div>) -> Stateful<gpui::Div>,
) -> gpui::Div {
    div()
        .flex_none()
        .h(px(28.))
        .p(px(2.))
        .gap(px(2.))
        .rounded(px(7.))
        .bg(control_fill(theme))
        .flex()
        .items_center()
        .children(tabs.iter().enumerate().map(|(index, (label, count))| {
            let active = index == selected;
            let tab = div()
                .id((id, index))
                .h(px(24.))
                .px(px(10.))
                .rounded(px(5.))
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(crate::typography::ui_rems(11.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .cursor_pointer()
                .role(gpui::Role::Tab)
                .aria_selected(active)
                .tab_index(0)
                .when(active, |tab| tab.bg(theme.bg).text_color(theme.text))
                .when(!active, |tab| {
                    let text = theme.text;
                    tab.text_color(theme.text_muted)
                        .hover(move |s| s.text_color(text))
                })
                .child(SharedString::from(label.to_string()))
                .when_some(*count, |tab, count| {
                    tab.child(
                        div()
                            .text_color(theme.text_muted)
                            .child(SharedString::from(count.to_string())),
                    )
                });
            on_select(index, tab)
        }))
}

/// A rounded logo tile: the plugin's bundled brand image, else its initial.
pub fn plugin_logo(theme: &Theme, domain: &str, name: &str, size: f32) -> gpui::Div {
    let path = format!("plugin-icons/{domain}");
    div()
        .size(px(size))
        .flex_none()
        .rounded(px(10.))
        .overflow_hidden()
        .bg(theme.settings_frame_bg())
        .flex()
        .items_center()
        .justify_center()
        .text_size(crate::typography::ui_rems(13.))
        .text_color(theme.text_muted)
        .child(if zuse_icons::load_plugin_icon(&path).is_some() {
            gpui::img(SharedString::from(path))
                .size_full()
                .rounded(px(10.))
                .into_any_element()
        } else {
            div()
                .child(
                    name.chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .to_string(),
                )
                .into_any_element()
        })
}

/// Title and one muted, truncated description line.
pub fn title_block(
    theme: &Theme,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    title_size: f32,
    description_size: f32,
) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .truncate()
                .text_size(crate::typography::ui_rems(title_size))
                .line_height(crate::typography::ui_rems(title_size + 4.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(title.into()),
        )
        .child(
            div()
                .truncate()
                .text_size(crate::typography::ui_rems(description_size))
                .line_height(crate::typography::ui_rems(description_size + 4.))
                .text_color(theme.text_muted)
                .child(description.into()),
        )
}
