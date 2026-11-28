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
        .text_size(crate::typography::text_xs())
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
        .text_size(crate::typography::text_xs())
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_strong)
        .cursor_pointer()
        .role(gpui::Role::Button)
        .tab_index(0)
        .hover(move |s| s.bg(hover))
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
                .text_size(crate::typography::text_xs())
                .font_weight(gpui::FontWeight::MEDIUM)
                .cursor_pointer()
                .role(gpui::Role::Tab)
                .aria_selected(active)
                .tab_index(0)
                .when(active, |tab| {
                    tab.relative()
                        .bg(theme.emboss_fill())
                        .shadow(theme.emboss_shadow())
                        .text_color(theme.text)
                        .child(crate::surface_chrome::emboss_rim(5.0, theme))
                })
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

/// Catalog filters and page switches share the sidebar's selected tab plate.
pub fn workspace_tabs(
    theme: &Theme,
    id: &'static str,
    label: &'static str,
    tabs: &[(&str, Option<&'static str>, Option<usize>)],
    selected: usize,
    mut wire: impl FnMut(usize, Stateful<gpui::Div>) -> Stateful<gpui::Div>,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::Role::TabList)
        .aria_label(label)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .children(
            tabs.iter()
                .enumerate()
                .map(|(index, &(label, glyph, count))| {
                    let active = selected == index;
                    let color = if active { theme.text } else { theme.text_muted };
                    let tab = crate::surface_chrome::workspace_tab((id, index), active, theme)
                        .debug_selector(move || format!("{id}-{index}"))
                        .role(gpui::Role::Tab)
                        .aria_label(label.to_owned())
                        .aria_selected(active)
                        .tab_index(0)
                        .focus_visible(|tab| tab.border_1().border_color(theme.accent))
                        .cursor_pointer()
                        .h(px(28.))
                        .px(px(9.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .text_size(crate::typography::text_xs())
                        .text_color(color)
                        .when(active, |tab| tab.font_weight(gpui::FontWeight::MEDIUM))
                        .when_some(glyph, |tab, glyph| {
                            tab.child(crate::icons::icon(glyph).size(px(14.)).text_color(color))
                        })
                        .child(label.to_owned())
                        .when_some(count, |tab, count| {
                            tab.child(div().text_color(theme.text_muted).child(count.to_string()))
                        });
                    wire(index, tab)
                }),
        )
}

pub fn activate(
    control: Stateful<gpui::Div>,
    action: impl Fn(&mut gpui::Window, &mut gpui::App) + Clone + 'static,
) -> Stateful<gpui::Div> {
    let keyboard = action.clone();
    control
        .on_click(move |_, window, cx| action(window, cx))
        .on_key_down(move |event, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                keyboard(window, cx);
                cx.stop_propagation();
            }
        })
}

pub fn page_tabs(theme: &Theme, skills: bool) -> Option<Stateful<gpui::Div>> {
    zuse_viewport::cloud_gate::enabled().then(|| {
        workspace_tabs(
            theme,
            "catalog-page-tab",
            "Catalog",
            &[
                ("Plugins", Some(crate::icons::DITHER_PLUG), None),
                ("Skills", Some(crate::icons::SKILL), None),
            ],
            usize::from(skills),
            |index, tab| {
                if index == usize::from(skills) {
                    return tab;
                }
                activate(tab, move |window, cx| {
                    if index == 0 {
                        window.dispatch_action(
                            Box::new(crate::settings::plugins::OpenPluginsPage),
                            cx,
                        );
                    } else {
                        window
                            .dispatch_action(Box::new(crate::settings::skills::OpenSkillsPage), cx);
                    }
                })
            },
        )
    })
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
        .text_size(crate::typography::text_sm())
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
                .text_color(theme.text_strong)
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

/// Plugins and Skills share one column, so switching between them keeps the
/// heading, toolbar and tabs exactly in place.
pub const PAGE_MAX_WIDTH: f32 = 768.;

/// Side gutter for the Plugins and Skills pages at a given content width.
pub fn page_padding(content_width: f32) -> f32 {
    if content_width < 800. { 16. } else { 32. }
}

/// The page title and its one-line subtitle.
pub fn page_heading(
    theme: &Theme,
    title: impl Into<SharedString>,
    subtitle: impl Into<SharedString>,
) -> gpui::Div {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .text_size(crate::typography::ui_rems(20.))
                .line_height(crate::typography::ui_rems(28.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(title.into()),
        )
        .child(
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(theme.text_muted)
                .child(subtitle.into()),
        )
}

/// A section label over a catalog grid ("Featured", "Popular", "All …").
pub fn section_label(theme: &Theme, label: impl Into<SharedString>) -> gpui::Div {
    div()
        .px(px(8.))
        .pb(px(4.))
        .text_size(px(13.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_strong)
        .child(label.into())
}
