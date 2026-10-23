//! Cloud settings navigation shared by desktop and browser.
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
        .aria_label("Cloud workspace settings")
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
                        view.bg(palette::rgb(0x0b0c0b)).shadow_xs()
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
