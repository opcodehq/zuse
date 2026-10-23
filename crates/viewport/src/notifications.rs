//! Completion cues consume durable markers even while muted or awaiting delivery.
use super::*;
use crate::{
    assets::icon,
    settings_style as s,
    shell::{BORDER, MUTED, TEXT, column, row},
};
use zuse_protocol::{
    Session,
    completion_sound::CompletionSoundPreset,
    notifications::{NotificationSound, SessionNotificationState},
};
impl WorkspaceView {
    pub(super) fn play_completion_sound(&self) {
        let _ = self.requests.unbounded_send(
            json!({"ui":"playCompletionSound", "preset":self.preferences.completion_sound.preset}),
        );
    }
    pub(super) fn receive_session_notifications(&mut self, value: &Value) {
        let Ok(sessions) = serde_json::from_value::<Vec<Session>>(value.clone()) else {
            return;
        };
        let now = chrono::Utc::now();
        self.notification_baselines
            .retain(|chat, _| sessions.iter().any(|session| &session.chat_id == chat));
        let mut completed = false;
        for session in sessions {
            let next = SessionNotificationState::new(&session, now);
            let previous = self
                .notification_baselines
                .insert(session.chat_id.clone(), next.clone());
            let pending = self
                .send_draft
                .values()
                .chain(self.uncertain_drafts.iter())
                .any(|draft| draft.chat_id.as_deref() == Some(session.chat_id.as_str()));
            let top_level = self
                .chats
                .iter()
                .any(|chat| chat["id"] == session.chat_id && chat["parentChatId"].is_null());
            if top_level && let Some(previous) = previous {
                completed |= next.sound_since(&previous, pending) == Some(NotificationSound::Done);
            }
        }
        // Coalesce a watch snapshot's simultaneous completions into one cue.
        if completed && self.preferences.completion_sound.enabled {
            self.play_completion_sound();
        }
    }
    fn on_completion_preset_key(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.preferences.completion_sound.enabled {
            return;
        }
        let key = event.keystroke.key.as_str();
        let count = CompletionSoundPreset::ALL.len();
        match key {
            "escape" if self.completion_preset_menu => self.completion_preset_menu = false,
            "tab" => {
                self.completion_preset_menu = false;
                cx.notify();
                return;
            }
            "up" | "down" | "home" | "end" => {
                if !self.completion_preset_menu {
                    self.completion_preset_active = match key {
                        "home" => 0,
                        "end" => count - 1,
                        _ => CompletionSoundPreset::ALL
                            .iter()
                            .position(|preset| *preset == self.preferences.completion_sound.preset)
                            .unwrap_or(0),
                    };
                } else {
                    self.completion_preset_active = match key {
                        "up" => (self.completion_preset_active + count - 1) % count,
                        "down" => (self.completion_preset_active + 1) % count,
                        "home" => 0,
                        _ => count - 1,
                    };
                }
                self.completion_preset_menu = true;
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
    pub(super) fn completion_sound_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.preferences.completion_sound.enabled;
        let preset = self.preferences.completion_sound.preset;
        let toggle = s::toggle(
            "general-completion-sound",
            enabled,
            "Agent completion sound",
        )
        .track_focus(&self.completion_switch_focus)
        .tab_index(0)
        .on_click(cx.listener(move |this, _, window, cx| {
            window.focus(&this.completion_switch_focus, cx);
            this.preferences.completion_sound.enabled = !this.preferences.completion_sound.enabled;
            this.completion_preset_menu = false;
            this.save_preferences();
            cx.notify();
        }));
        let selector = column()
            .id("completion-preset-select")
            .relative()
            .child(
                s::button("general-completion-preset", preset.label())
                    .w(px(160.))
                    .h(px(24.))
                    .px(px(7.))
                    .justify_between()
                    .role(Role::ComboBox)
                    .track_focus(&self.completion_preset_focus.clone().tab_stop(enabled))
                    .tab_index(0)
                    .on_key_down(cx.listener(Self::on_completion_preset_key))
                    .aria_label("Completion sound preset")
                    .aria_expanded(self.completion_preset_menu)
                    .when(!enabled, |button| button.opacity(0.64).cursor_default())
                    .child(icon("alt-arrow-down").size(px(14.)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        if !this.preferences.completion_sound.enabled {
                            return;
                        }
                        window.focus(&this.completion_preset_focus, cx);
                        if event.is_keyboard() && this.completion_preset_menu {
                            this.preferences.completion_sound.preset =
                                CompletionSoundPreset::ALL[this.completion_preset_active];
                            this.save_preferences();
                            this.completion_preset_menu = false;
                        } else {
                            let just_closed = !event.is_keyboard()
                                && this.completion_preset_closed.is_some_and(|time| {
                                    time.elapsed() < std::time::Duration::from_millis(300)
                                });
                            this.completion_preset_active = CompletionSoundPreset::ALL
                                .iter()
                                .position(|choice| {
                                    *choice == this.preferences.completion_sound.preset
                                })
                                .unwrap_or(0);
                            this.completion_preset_menu =
                                !this.completion_preset_menu && !just_closed;
                        }
                        this.completion_preset_closed = None;
                        cx.notify();
                    })),
            )
            .when(enabled && self.completion_preset_menu, |view| {
                view.child(
                    deferred(
                        column()
                            .id("completion-preset-list")
                            .role(Role::ListBox)
                            .aria_label("Completion sound presets")
                            .absolute()
                            .top_full()
                            .mt_1()
                            .right_0()
                            .w(px(160.))
                            .p_1()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .bg(crate::palette::rgb(crate::shell::RAISED))
                            .shadow_lg()
                            .occlude()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.completion_preset_menu = false;
                                this.completion_preset_closed = Some(web_time::Instant::now());
                                cx.notify();
                            }))
                            .children(CompletionSoundPreset::ALL.into_iter().enumerate().map(
                                |(index, choice)| {
                                    row()
                                        .id(("completion-sound-preset", index))
                                        .role(Role::ListBoxOption)
                                        .aria_selected(choice == preset)
                                        .cursor_pointer()
                                        .h(px(28.))
                                        .px_2()
                                        .gap_2()
                                        .rounded(px(7.))
                                        .text_size(px(12.))
                                        .text_color(rgb(TEXT))
                                        .when(index == self.completion_preset_active, |view| {
                                            view.bg(s::frame_bg())
                                        })
                                        .hover(|view| view.bg(s::frame_bg()))
                                        .child(div().flex_1().child(choice.label()))
                                        .when(choice == preset, |view| {
                                            view.child(
                                                icon("check").size(px(14.)).text_color(rgb(MUTED)),
                                            )
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.preferences.completion_sound.preset = choice;
                                            this.completion_preset_menu = false;
                                            this.save_preferences();
                                            cx.notify();
                                        }))
                                },
                            )),
                    )
                    .priority(2),
                )
            });
        let controls = row()
            .gap_2()
            .flex_none()
            .flex_wrap()
            .when(!enabled, |controls| controls.opacity(0.6))
            .child(icon("volume-loud").size(px(16.)).text_color(rgb(MUTED)))
            .child(selector)
            .child(
                s::button("general-preview-sound", "Preview")
                    .tab_index(0)
                    .tab_stop(enabled)
                    .when(!enabled, |button| button.opacity(0.64).cursor_default())
                    .on_click(cx.listener(move |this, _, _, _| {
                        if this.preferences.completion_sound.enabled {
                            this.play_completion_sound();
                        }
                    })),
            );
        let updates = self.preferences.provider_update_notifications;
        let updates_switch = s::toggle(
            "general-provider-updates",
            updates,
            "Provider update notifications",
        )
        .track_focus(&self.provider_notice_focus)
        .tab_index(0)
        .on_click(cx.listener(|this, _, window, cx| {
            window.focus(&this.provider_notice_focus, cx);
            this.preferences.provider_update_notifications =
                !this.preferences.provider_update_notifications;
            this.save_preferences();
            cx.notify();
        }));
        let settings_row = s::setting_row(false).flex_col().flex_nowrap().items_stretch().gap_2()
            .child(row().items_start().gap(px(10.)).child(s::details("Agent completion sound", "Play a short sound when any agent turn finishes, including agents working in background chats.")).child(row().h(px(28.)).child(toggle)))
            .child(controls);
        s::frame()
            .child(s::header("Notifications", ""))
            .child(
                s::card()
                    .map(|mut card| {
                        card.style().overflow = Default::default();
                        card
                    })
                    .child(s::setting_row(true).child(s::details("Provider update notifications","Show a notification when a newer version of an agent provider is available. Off by default.")).child(row().h(px(28.)).child(updates_switch)))
                    .child(settings_row),
            )
            .into_any_element()
    }
}
