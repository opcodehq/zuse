//! Original Zuse General groups; settings writes use the existing shared store.
use super::*;
use crate::appearance::{self, AppearanceMode};
impl ShortcutsPage {
    pub(super) fn original_general_header(&self, theme: &Theme) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .pb(px(16.))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .text_size(crate::typography::ui_rems(20.))
                    .line_height(crate::typography::ui_rems(28.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child("General"),
            )
            .child(
                div()
                    .text_size(crate::typography::ui_rems(12.))
                    .line_height(crate::typography::ui_rems(20.))
                    .text_color(theme.text_muted)
                    .child("Workspace defaults and your personal app preferences."),
            )
    }
    pub(super) fn original_general_preferences(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mode = appearance::mode(cx);
        let preferences = crate::settings::current(cx);
        let modes = div()
            .flex()
            .p(px(2.))
            .rounded(px(10.))
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
                            .cursor_pointer()
                            .h(px(28.))
                            .px(px(10.))
                            .flex()
                            .items_center()
                            .rounded(px(7.))
                            .text_size(crate::typography::ui_rems(12.))
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
            "Choose the app theme or follow your system setting.",
        )
        .child(
            widgets::section_card(theme).mt_0().child(
                widgets::card_row(theme, true)
                    .child(general_details(
                        theme,
                        "Theme",
                        "Choose the app theme or follow your system setting.",
                    ))
                    .child(modes),
            ),
        );
        let updates = preferences.agent_update_notifications;
        let sound = preferences.sound_completion_enabled;
        let update_switch = widgets::toggle_switch(theme, updates, "general-provider-updates")
            .id("general-provider-updates")
            .role(gpui::Role::Switch)
            .tab_index(0)
            .aria_label("Provider update notifications")
            .aria_toggled(if updates {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| {
                crate::settings::update(SavePolicy::Immediate, cx, |settings| {
                    settings.agent_update_notifications = !updates
                });
                cx.refresh_windows();
            }));
        let sound_switch = widgets::toggle_switch(theme, sound, "general-completion-sound")
            .id("general-completion-sound")
            .role(gpui::Role::Switch)
            .tab_index(0)
            .aria_label("Agent completion sound")
            .aria_toggled(if sound {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .cursor_pointer()
            .on_click(cx.listener(move |_, _, _, cx| {
                crate::settings::update(SavePolicy::Immediate, cx, |settings| {
                    settings.sound_completion_enabled = !sound
                });
                cx.refresh_windows();
            }));
        let notifications=widgets::group(theme,"Notifications","")
            .child(widgets::section_card(theme).mt_0()
                .child(widgets::card_row(theme,true).child(general_details(theme,"Provider update notifications","Show a notification when a newer version of an agent provider is available. Off by default.")).child(update_switch))
                .child(widgets::card_row(theme,false).child(general_details(theme,"Agent completion sound","Play a short sound when any agent turn finishes, including agents working in background chats.")).child(sound_switch)
                    .child(widgets::text_action(theme,widgets::ActionTone::Outlined,"Preview").id("general-preview-sound").on_click(move|_,_,_|{if sound{crate::sound::play(crate::sound::Sound::Done);}}))));
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .mt(px(16.))
            .child(appearance)
            .child(notifications)
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
