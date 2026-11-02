//! Settings → Notifications: independently configurable session chimes plus
//! desktop banners on the same status transitions (`shell::on_state_changed`).
//!
//! The ShortcutsPage arrangement: the page holds a working copy, every flip
//! emits [`NotificationsEvent::Changed`], and the shell persists it. Nothing
//! here talks RPC — all preferences are device-local UI settings.

use crate::localization::message as tr;
use gpui::{Context, EventEmitter, SharedString, Window, div, prelude::*, px};
use zuse_i18n::messages as msg;

use crate::popover;
use crate::settings::widgets;
use crate::theme::Theme;

#[derive(Debug, Clone)]
pub enum NotificationsEvent {
    /// A toggle flipped — persist the complete notification preference set.
    Changed {
        sound: bool,
        completion_sound: bool,
        input_sound: bool,
        attention_sound: bool,
        desktop: bool,
        background_only: bool,
        agent_updates: bool,
    },
}

pub struct NotificationsPage {
    scroll: widgets::PageScroll,
    sound: bool,
    completion_sound: bool,
    input_sound: bool,
    attention_sound: bool,
    desktop: bool,
    background_only: bool,
    agent_updates: bool,
}

impl EventEmitter<NotificationsEvent> for NotificationsPage {}

#[derive(Clone, Copy)]
enum NotificationPreference {
    Sound,
    CompletionSound,
    InputSound,
    AttentionSound,
    Desktop,
    BackgroundOnly,
    AgentUpdates,
}

fn interactive_switch(
    element: gpui::Stateful<gpui::Div>,
    accent: gpui::Hsla,
    preference: NotificationPreference,
    cx: &mut Context<NotificationsPage>,
) -> gpui::Stateful<gpui::Div> {
    element
        .tab_index(0)
        .focus_visible(move |style| style.border_2().border_color(accent))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, _, cx| {
            this.toggle(preference, cx);
        }))
        .on_key_down(|event: &gpui::KeyDownEvent, _, cx| {
            // GPUI activates the focused switch on key-up. Contain the key
            // without also toggling on key-down.
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
            }
        })
}

impl NotificationsPage {
    pub fn new(
        sound: bool,
        completion_sound: bool,
        input_sound: bool,
        attention_sound: bool,
        desktop: bool,
        background_only: bool,
        agent_updates: bool,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            scroll: widgets::PageScroll::default(),
            sound,
            completion_sound,
            input_sound,
            attention_sound,
            desktop,
            background_only,
            agent_updates,
        }
    }

    pub(crate) fn sync_settings(
        &mut self,
        settings: &crate::settings::UiSettings,
        cx: &mut Context<Self>,
    ) {
        let current = (
            self.sound,
            self.completion_sound,
            self.input_sound,
            self.attention_sound,
            self.desktop,
            self.background_only,
            self.agent_updates,
        );
        let next = (
            settings.sound_enabled,
            settings.sound_completion_enabled,
            settings.sound_input_enabled,
            settings.sound_attention_enabled,
            settings.notifications_enabled,
            settings.notifications_background_only,
            settings.agent_update_notifications,
        );
        if current == next {
            return;
        }
        (
            self.sound,
            self.completion_sound,
            self.input_sound,
            self.attention_sound,
            self.desktop,
            self.background_only,
            self.agent_updates,
        ) = next;
        cx.notify();
    }

    fn emit(&self, cx: &mut Context<Self>) {
        cx.emit(NotificationsEvent::Changed {
            sound: self.sound,
            completion_sound: self.completion_sound,
            input_sound: self.input_sound,
            attention_sound: self.attention_sound,
            desktop: self.desktop,
            background_only: self.background_only,
            agent_updates: self.agent_updates,
        });
    }

    fn toggle(&mut self, preference: NotificationPreference, cx: &mut Context<Self>) {
        let value = match preference {
            NotificationPreference::Sound => &mut self.sound,
            NotificationPreference::CompletionSound => &mut self.completion_sound,
            NotificationPreference::InputSound => &mut self.input_sound,
            NotificationPreference::AttentionSound => &mut self.attention_sound,
            NotificationPreference::Desktop => &mut self.desktop,
            NotificationPreference::BackgroundOnly => &mut self.background_only,
            NotificationPreference::AgentUpdates => &mut self.agent_updates,
        };
        *value = !*value;
        self.emit(cx);
        cx.notify();
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for NotificationsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for NotificationsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let accent = theme.accent;
        let sound = self.sound;
        let completion_sound = self.completion_sound;
        let input_sound = self.input_sound;
        let attention_sound = self.attention_sound;
        let desktop = self.desktop;
        let background_only = self.background_only;
        let agent_updates = self.agent_updates;
        let toggle = |id: &'static str, label: &'static str, enabled: bool, interactive: bool| {
            // Keep the visual inside its 56×40 activation target.
            // Disabled subordinate controls remain named switches in the
            // accessibility tree, but have no focus or input handlers.
            div()
                .id(id)
                .flex_none()
                .w(px(widgets::SWITCH_WIDTH))
                .h(px(40.0))
                // Lay out at the row's 28px control height; the extra target
                // overhangs the row padding instead of growing the row.
                .my(px(-6.0))
                .flex()
                .items_center()
                .justify_center()
                .role(gpui::Role::Switch)
                .aria_label(label)
                .aria_toggled(if enabled {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .when(!interactive, |el| {
                    el.aria_description("Unavailable while its parent setting is off")
                })
                .child(widgets::toggle_switch(&theme, enabled, id))
        };
        let card = widgets::section_card(&theme)
            .mt_0()
            .child(
                widgets::card_row(&theme, true)
                    .child(div().flex_1().min_w(px(160.0)).flex().flex_col().child(
                        widgets::row_title(&theme, tr(cx, msg::native::session_sounds)),
                    ))
                    .child(interactive_switch(
                        toggle("notifications-sound-toggle", "Session sounds", sound, true),
                        accent,
                        NotificationPreference::Sound,
                        cx,
                    )),
            )
            .child(
                widgets::card_row(&theme, false)
                    .when(!sound, |el| el.opacity(0.55))
                    .child(div().flex_1().min_w(px(160.0)).flex().flex_col().child(
                        widgets::row_title(&theme, tr(cx, msg::native::task_completed)),
                    ))
                    .child(
                        toggle(
                            "notifications-completion-sound-toggle",
                            "Task completed sound",
                            completion_sound,
                            sound,
                        )
                        .when(sound, |el| {
                            interactive_switch(
                                el,
                                accent,
                                NotificationPreference::CompletionSound,
                                cx,
                            )
                        }),
                    ),
            )
            .child(
                widgets::card_row(&theme, false)
                    .when(!sound, |el| el.opacity(0.55))
                    .child(div().flex_1().min_w(px(160.0)).flex().flex_col().child(
                        widgets::row_title(&theme, tr(cx, msg::native::input_required)),
                    ))
                    .child(
                        toggle(
                            "notifications-input-sound-toggle",
                            "Input required sound",
                            input_sound,
                            sound,
                        )
                        .when(sound, |el| {
                            interactive_switch(el, accent, NotificationPreference::InputSound, cx)
                        }),
                    ),
            )
            .child(
                widgets::card_row(&theme, false)
                    .when(!sound, |el| el.opacity(0.55))
                    .child(div().flex_1().min_w(px(160.0)).flex().flex_col().child(
                        widgets::row_title(&theme, tr(cx, msg::native::errors_and_disconnections)),
                    ))
                    .child(
                        toggle(
                            "notifications-attention-sound-toggle",
                            "Errors and disconnections sound",
                            attention_sound,
                            sound,
                        )
                        .when(sound, |el| {
                            interactive_switch(
                                el,
                                accent,
                                NotificationPreference::AttentionSound,
                                cx,
                            )
                        }),
                    ),
            );
        let desktop_card = widgets::section_card(&theme)
            .mt_0()
            .child(
                widgets::card_row(&theme, true)
                    .child(div().flex_1().min_w(px(160.0)).flex().flex_col().child(
                        widgets::row_title(&theme, tr(cx, msg::native::desktop_notifications)),
                    ))
                    .child(interactive_switch(
                        toggle(
                            "notifications-desktop-toggle",
                            "Desktop notifications",
                            desktop,
                            true,
                        ),
                        accent,
                        NotificationPreference::Desktop,
                        cx,
                    )),
            )
            .child(
                widgets::card_row(&theme, false)
                    .when(!desktop, |el| el.opacity(0.55))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(
                                &theme,
                                tr(cx, msg::native::agent_updates),
                            ))
                            .child(widgets::meta_line(
                                &theme,
                                vec![
                                    div()
                                        .child(SharedString::from(
                                            "Show a banner when monitored agent CLIs have updates.",
                                        ))
                                        .into_any_element(),
                                ],
                            )),
                    )
                    .child(
                        toggle(
                            "notifications-agent-updates-toggle",
                            "Agent update notifications",
                            agent_updates,
                            desktop,
                        )
                        .when(desktop, |el| {
                            interactive_switch(el, accent, NotificationPreference::AgentUpdates, cx)
                        }),
                    ),
            )
            .child(
                // Sub-option of the banner row: dimmed + inert while banners
                // are off (the harnesses not-installed treatment).
                widgets::card_row(&theme, false)
                    .when(!desktop, |el| el.opacity(0.55))
                    .child(div().flex_1().min_w(px(160.0)).flex().flex_col().child(
                        widgets::row_title(
                            &theme,
                            tr(cx, msg::native::only_when_in_the_background),
                        ),
                    ))
                    .child(
                        toggle(
                            "notifications-background-toggle",
                            "Only notify when Zuse is in the background",
                            background_only,
                            desktop,
                        )
                        .when(desktop, |el| {
                            interactive_switch(
                                el,
                                accent,
                                NotificationPreference::BackgroundOnly,
                                cx,
                            )
                        }),
                    ),
            );

        let scrollbar = popover::rail(self, "notifications-page-scrollbar", &theme, cx);
        div()
            .id("notifications-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("notifications-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::settings_page_column(window)
                                .child(widgets::section_title(
                                    &theme,
                                    &tr(cx, msg::settings::settings_page_notifications),
                                    &tr(cx, msg::native::choose_when_zuse_alerts_you_about_agent_activity),
                                ))
                                .child(
                                    widgets::section_stack()
                                        .child(widgets::section(&theme, crate::localization::text("chat-browser_pane_desktop", cx), desktop_card))
                                        .child(widgets::section(&theme, "Sounds", card)),
                                ),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}
