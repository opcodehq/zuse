//! Cloud settings navigation shared by desktop and browser.
use crate::localization::message as tr;
use zuse_i18n::messages as msg;
pub(crate) mod style;
use crate::{palette, settings_style as s};
use gpui::{prelude::*, *};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Setup,
    Usage,
}

pub fn tabs<T: 'static>(
    selected: Tab,
    focus: &[FocusHandle; 2],
    cx: &mut Context<T>,
    select: impl Fn(&mut T, Tab, &mut Window, &mut Context<T>) + Copy + 'static,
) -> Stateful<Div> {
    div()
        .id("cloud-settings-tabs")
        .role(Role::TabList)
        .aria_label(tr(cx, msg::common::cloud_workspace_settings))
        .flex()
        .items_center()
        .w(px(224.))
        .max_w_full()
        .h(px(28.))
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(7.))
        .bg(s::frame_bg().opacity(0.55))
        .children(
            [(Tab::Setup, "Setup"), (Tab::Usage, "Usage")].map(|(tab, label)| {
                let handles = focus.clone();
                div()
                    .id(if tab == Tab::Setup {
                        "cloud-tab-setup"
                    } else {
                        "cloud-tab-usage"
                    })
                    .role(Role::Tab)
                    .aria_label(label)
                    .aria_selected(selected == tab)
                    .track_focus(
                        &focus[if tab == Tab::Setup { 0 } else { 1 }]
                            .clone()
                            .tab_stop(selected == tab),
                    )
                    .tab_index(if selected == tab { 0 } else { -1 })
                    .cursor_pointer()
                    .flex_1()
                    .min_w_0()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(10.))
                    .rounded(px(5.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette::rgb(if selected == tab {
                        0xe8e9e7
                    } else {
                        0xa4a8a3
                    }))
                    .when(selected == tab, |view| {
                        view.bg(palette::rgb(0x151515)).shadow_xs()
                    })
                    .hover(|view| view.text_color(palette::rgb(0xe8e9e7)))
                    .focus_visible(|view| view.border_1().border_color(palette::rgb(0x89ca21)))
                    .child(label)
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        let next = match event.keystroke.key.as_str() {
                            "left" | "right" => {
                                if selected == Tab::Setup {
                                    Tab::Usage
                                } else {
                                    Tab::Setup
                                }
                            }
                            "home" => Tab::Setup,
                            "end" => Tab::Usage,
                            "enter" | "space" => {
                                cx.stop_propagation();
                                return;
                            }
                            _ => return,
                        };
                        select(this, next, window, cx);
                        window.focus(&handles[if next == Tab::Setup { 0 } else { 1 }], cx);
                        cx.stop_propagation();
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| select(this, tab, window, cx)))
            }),
        )
}

/// The reference account gate is shared across both Cloud settings hosts.
pub fn account_gate(action: impl IntoElement, error: Option<&str>, cx: &gpui::App) -> Div {
    use crate::shell::MUTED;
    let mut copy = div().flex_1().min_w_0().flex().flex_col()
        .child(s::title_text(tr(cx, msg::settings::cloud_workspace_pool_sign_in_to_set_up_cloud_workspaces)))
        .child(div().mt(px(2.)).max_w(px(576.)).text_size(px(11.))
            .line_height(px(16.)).text_color(palette::rgb(MUTED))
            .child(tr(cx, msg::settings::cloud_workspace_pool_choose_a_plan_connect_repositories_and_authorize_your_coding_agents_fr)))
        .child(div().mt(px(4.)).text_size(px(10.)).line_height(px(13.5))
            .text_color(palette::rgb(MUTED).opacity(0.75))
            .child(tr(cx, msg::settings::cloud_workspace_pool_local_chats_stay_on_this_computer_and_remain_available_without_an_acco)));
    if let Some(error) = error {
        copy = copy.child(
            div()
                .mt(px(4.))
                .text_size(px(11.))
                .text_color(s::destructive_text())
                .child(error.to_owned()),
        );
    }
    div()
        .w_full()
        .min_h(px(76.))
        .flex()
        .items_center()
        .gap(px(16.))
        // The reference uses an inset ring; compensate for the layout border.
        .p(px(11.))
        .rounded(px(10.))
        .bg(s::card_bg())
        .border_1()
        .border_color(
            palette::tone(
                hsla(132. / 360., 0.04, 0.125, 1.),
                hsla(110. / 360., 0.13, 0.89, 1.),
            )
            .opacity(0.7),
        )
        .child(copy)
        .child(div().flex_shrink_0().child(action))
}

pub fn sign_in_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let lime = palette::tone(
        hsla(83. / 360., 0.72, 0.46, 1.),
        hsla(83. / 360., 0.74, 0.43, 1.),
    );
    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .cursor_pointer()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.))
        .px(px(11.))
        .rounded(px(7.))
        .border_1()
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .line_height(px(13.3333))
        .bg(lime)
        .border_color(lime)
        .text_color(hsla(132. / 360., 0.30, 0.07, 1.))
        .on_key_down(|event: &KeyDownEvent, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
            }
        })
        .hover(move |button| button.bg(lime.opacity(0.9)))
        .child(label)
}
