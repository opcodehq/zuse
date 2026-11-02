//! Original Zuse General groups; settings writes use the existing shared store.
use super::*;
use crate::appearance::{self, AppearanceMode};
impl ShortcutsPage {
    pub(super) fn general_header(&self, theme: &Theme) -> gpui::Div {
        widgets::section_title(
            theme,
            "General",
            "Workspace defaults and your personal app preferences.",
        )
    }

    pub(super) fn general_preferences(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let mode = appearance::mode(cx);
        let preferences = crate::settings::current(cx);
        let modes = div()
            .flex()
            .p(px(2.))
            .rounded(px(8.))
            .border_1()
            .border_color(theme.border.opacity(0.6))
            .bg(theme.settings_frame_bg())
            .children(
                AppearanceMode::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, choice)| {
                        div()
                            .id(("general-theme", index))
                            .role(gpui::Role::Button)
                            .tab_index(0)
                            .aria_label(choice.label())
                            .aria_toggled(if mode == choice {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .on_key_down(contain_activation)
                            .cursor_pointer()
                            .h(px(28.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(6.))
                            .text_size(crate::typography::text_xs())
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if mode == choice {
                                theme.text
                            } else {
                                theme.text_muted
                            })
                            .when(mode == choice, |view| view.bg(theme.bg))
                            .child(choice.label())
                            .on_click(cx.listener(move |_, _, _, cx| {
                                appearance::set_mode(choice, cx);
                                cx.refresh_windows();
                            }))
                    }),
            );
        let appearance = widgets::group(
            theme,
            "Appearance",
            "Choose the app theme, or follow your system setting.",
        )
        .child(
            widgets::section_card(theme).mt_0().child(
                widgets::card_row(theme, true)
                    .items_start()
                    .child(general_details(
                        theme,
                        "Theme",
                        "Choose the app theme, or follow your system setting.",
                    ))
                    .child(
                        div()
                            .h(px(28.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .child(modes),
                    ),
            ),
        );
        let updates = preferences.agent_update_notifications;
        let sound = preferences.sound_enabled && preferences.sound_completion_enabled;
        let update_switch = widgets::toggle_switch(theme, updates, "general-provider-updates")
            .id("general-provider-updates")
            .role(gpui::Role::Switch)
            .tab_index(0)
            .on_key_down(contain_activation)
            .aria_label("Provider update notifications")
            .aria_toggled(if updates {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| {
                crate::settings::update(SavePolicy::Immediate, cx, |settings| {
                    settings.agent_update_notifications = !settings.agent_update_notifications
                });
                cx.refresh_windows();
            }));
        let sound_switch = widgets::toggle_switch(theme, sound, "general-completion-sound")
            .id("general-completion-sound")
            .role(gpui::Role::Switch)
            .tab_index(0)
            .on_key_down(contain_activation)
            .aria_label("Agent completion sound")
            .aria_toggled(if sound {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| {
                crate::settings::update(SavePolicy::Immediate, cx, |settings| {
                    let enable = !(settings.sound_enabled && settings.sound_completion_enabled);
                    settings.sound_completion_enabled = enable;
                    if enable {
                        settings.sound_enabled = true;
                    }
                });
                cx.refresh_windows();
            }));
        use zuse_protocol::completion_sound::CompletionSoundPreset as Preset;
        let preset = preferences.completion_sound_preset;
        let selected = Preset::ALL
            .iter()
            .position(|choice| *choice == preset)
            .unwrap_or(0);
        let selector = if sound {
            widgets::select(
                "general-completion-preset",
                "Completion sound preset",
                theme,
                |page: &mut Self| &mut page.completion_preset_select,
            )
            .options(
                Preset::ALL
                    .into_iter()
                    .map(|preset| widgets::SelectOption::new(preset.label())),
                selected,
            )
            .width(160.)
            .on_select(|_, selected, _, cx| {
                if let Some(preset) = Preset::ALL.get(selected) {
                    crate::settings::update(SavePolicy::Immediate, cx, |settings| {
                        settings.completion_sound_preset = *preset
                    });
                    cx.refresh_windows();
                }
            })
            .render(&self.completion_preset_select, cx)
            .h(px(24.))
            .px(px(7.))
            .bg(theme.surface_card)
            .border_color(theme.border)
            .text_size(crate::typography::ui_rems(11.))
            .into_any_element()
        } else {
            widgets::text_action(theme, widgets::ActionTone::Outlined, preset.label())
                .w(px(160.))
                .h(px(24.))
                .px(px(7.))
                .justify_between()
                .text_size(crate::typography::ui_rems(11.))
                .child(crate::icons::icon(crate::icons::ALT_ARROW_DOWN).size(px(14.)))
                .opacity(0.64)
                .cursor_default()
                .into_any_element()
        };
        let notifications=widgets::group(theme,"Notifications","")
            .child(widgets::section_card(theme).mt_0()
                .child(widgets::card_row(theme,true).items_start().child(general_details(theme,"Provider update notifications","Show a notification when a newer version of an agent provider is available. Off by default.")).child(div().h(px(28.)).flex_none().flex().items_center().child(update_switch)))
                .child(widgets::card_row(theme,false).flex_col().flex_nowrap().items_stretch().gap(px(8.))
                    .child(div().flex().items_start().gap(px(10.)).child(general_details(theme,"Agent completion sound","Play a short sound when any agent turn finishes, including agents working in background chats.")).child(div().flex().items_center().h(px(28.)).child(sound_switch)))
                    .child(div().flex().items_center().flex_wrap().gap(px(8.)).when(!sound,|controls|controls.opacity(0.6))
                        .child(crate::icons::icon(crate::icons::VOLUME_LOUD).size(px(16.)).text_color(theme.text_muted))
                        .child(selector)
                        .child(widgets::text_action(theme,widgets::ActionTone::Outlined,"Preview").id("general-preview-sound").tab_index(0).tab_stop(sound).when(!sound, |button| button.opacity(0.64)).on_click(move|_,_,_|{if sound{crate::sound::play_with_completion_preset(crate::sound::Sound::Done, preset);}})))));
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(appearance)
            .child(notifications)
    }

    pub(super) fn general_app_updates(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        widgets::group(theme, "About Zuse", "").child(
            widgets::section_card(theme).mt_0().child(
                widgets::card_row(theme, true)
                    .child(general_details(
                        theme,
                        "App updates",
                        "Check for a newer version of Zuse.",
                    ))
                    .child(
                        widgets::text_action(
                            theme,
                            widgets::ActionTone::Outlined,
                            "Check for updates",
                        )
                        .id("general-check-updates")
                        .debug_selector(|| "general-check-updates".into())
                        .track_focus(&self.general_updates_focus)
                        .tab_index(0)
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                window.focus(&this.general_updates_focus, cx);
                            }),
                        )
                        .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                crate::app_update::check_for_updates(cx);
                                cx.stop_propagation();
                            }
                        }))
                        .on_click(|_, _, cx| crate::app_update::check_for_updates(cx)),
                    ),
            ),
        )
    }
}
fn contain_activation(event: &gpui::KeyDownEvent, _: &mut gpui::Window, cx: &mut gpui::App) {
    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
        cx.stop_propagation();
    }
}
fn general_details(theme: &Theme, title: &str, description: &str) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(widgets::row_title(theme, title.to_owned()))
        .child(
            div()
                .text_size(crate::typography::ui_rems(11.))
                .line_height(crate::typography::ui_rems(16.))
                .text_color(theme.text_muted)
                .child(description.to_owned()),
        )
}
