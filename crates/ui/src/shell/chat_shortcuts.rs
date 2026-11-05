//! App-level chat shortcuts that need shell state: the keyboard shortcuts
//! panel and the two-press stop of a chat's background tasks.
use super::*;
use crate::settings::display_combo;
use settings::keybindings::Command;
use std::time::Instant;

/// How long the first stop-background-tasks press waits for the second.
pub(super) const STOP_CONFIRM_WINDOW: Duration = Duration::from_secs(3);

/// Confirmation belongs to exactly the chat and tasks the first press named.
pub(super) struct StopConfirmation {
    chat_id: String,
    tasks: Vec<String>,
    at: Instant,
}

pub(super) fn stop_press_confirms(
    armed: Option<&StopConfirmation>,
    chat_id: &str,
    tasks: &[String],
    now: Instant,
) -> bool {
    armed.is_some_and(|armed| {
        armed.chat_id == chat_id
            && armed.tasks == tasks
            && now.saturating_duration_since(armed.at) <= STOP_CONFIRM_WINDOW
    })
}

const COMPOSER_COMMANDS: [Command; 15] = [
    Command::ComposerSubmit,
    Command::ComposerNewline,
    Command::ComposerForceSubmit,
    Command::ComposerTogglePlanMode,
    Command::ComposerPreviousMessage,
    Command::ComposerNextMessage,
    Command::ComposerQueueSubmit,
    Command::ComposerDeleteToStart,
    Command::ComposerDeleteToEnd,
    Command::ComposerDeleteWord,
    Command::ComposerInterrupt,
    Command::ComposerYank,
    Command::ComposerYankPop,
    Command::ComposerStash,
    Command::ComposerSearchHistory,
];

const CHAT_COMMANDS: [Command; 7] = [
    Command::OpenModelPicker,
    Command::CycleReasoningEffort,
    Command::ToggleFastMode,
    Command::ToggleToolDetails,
    Command::ToggleTaskList,
    Command::FindInChat,
    Command::StopBackgroundTasks,
];

const APP_COMMANDS: [Command; 7] = [
    Command::NewChat,
    Command::OpenChatSwitcher,
    Command::SearchFiles,
    Command::FocusComposer,
    Command::ToggleLeftSidebar,
    Command::ToggleTerminal,
    Command::ShowShortcuts,
];

/// `(label, combo)` rows for the live keymap; unbound commands are left out
/// so a rebind or a cleared shortcut shows up immediately.
pub(super) fn panel_rows(keymap: &KeymapConfig, commands: &[Command]) -> Vec<(Command, String)> {
    commands
        .iter()
        .filter_map(|&command| {
            let combo = keymap.get(ShortcutId::Desktop(command));
            (!combo.is_empty()).then(|| (command, display_combo(combo)))
        })
        .collect()
}

impl Shell {
    pub(super) fn toggle_shortcuts_panel(&mut self, cx: &mut Context<Self>) {
        self.shortcuts_panel_open = !self.shortcuts_panel_open;
        cx.notify();
    }

    pub(super) fn close_shortcuts_panel(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.shortcuts_panel_open {
            return false;
        }
        self.shortcuts_panel_open = false;
        cx.notify();
        true
    }

    /// The first press says how many tasks would stop; a second press within
    /// [`STOP_CONFIRM_WINDOW`] stops them.
    pub(super) fn stop_background_tasks_shortcut(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let target = self.shortcut_conversation(window, cx).0;
        let (chat_id, stoppable) = {
            let state = target.read(cx);
            match state.selected_chat.clone() {
                Some(chat_id) if matches!(self.route, Route::Chat) => {
                    let tasks = zuse_protocol::view::background_tasks(
                        state.session_for(&chat_id),
                        Utc::now(),
                    );
                    let stoppable = self.stoppable_background_tasks(tasks);
                    (Some(chat_id), stoppable)
                }
                _ => (None, Vec::new()),
            }
        };
        let Some(chat_id) = chat_id.filter(|_| !stoppable.is_empty()) else {
            self.background_stop_armed_at = None;
            let message = tr(cx, msg::commands::commands_no_background_tasks);
            crate::toast::info(cx, message);
            return;
        };
        let count = stoppable.len() as i64;
        if !stop_press_confirms(
            self.background_stop_armed_at.as_ref(),
            &chat_id,
            &stoppable,
            now,
        ) {
            self.background_stop_armed_at = Some(StopConfirmation {
                chat_id,
                tasks: stoppable,
                at: now,
            });
            let shortcut = display_combo(
                self.settings
                    .keymap
                    .get(ShortcutId::Desktop(Command::StopBackgroundTasks)),
            );
            let message = tr(cx, |l10n| {
                msg::commands::commands_stop_background_tasks_confirm(l10n, count, shortcut)
            });
            crate::toast::info(cx, message);
            return;
        }
        self.background_stop_armed_at = None;
        for task_id in &stoppable {
            self.stop_background_task(&chat_id, task_id, cx);
        }
        let message = tr(cx, |l10n| {
            msg::commands::commands_stopping_background_tasks(l10n, count)
        });
        crate::toast::info(cx, message);
    }

    /// The cheat sheet of the shortcuts in effect right now.
    pub(super) fn render_shortcuts_panel(
        &mut self,
        viewport: gpui::Size<Pixels>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.shortcuts_panel_open {
            return None;
        }
        let theme = Theme::of(cx).for_popup();
        let keymap = &self.settings.keymap;
        let row = |label: SharedString, keys: SharedString| {
            div()
                .h(px(28.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .text_size(px(12.0))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(theme.text)
                        .child(label),
                )
                .child(
                    div()
                        .flex_none()
                        .px(px(6.0))
                        .py(px(1.0))
                        .rounded(px(4.0))
                        .bg(theme.element_hover)
                        .text_color(theme.text_muted)
                        .child(keys),
                )
                .into_any_element()
        };
        let section = |title: String, rows: Vec<AnyElement>| {
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .mb(px(4.0))
                        .text_size(px(11.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(title),
                )
                .children(rows)
        };
        let rows = |commands: &[Command]| -> Vec<AnyElement> {
            panel_rows(keymap, commands)
                .into_iter()
                .map(|(command, keys)| {
                    row(
                        ShortcutId::Desktop(command).localized_label(cx).into(),
                        keys.into(),
                    )
                })
                .collect()
        };
        let triggers = vec![
            row(
                tr(cx, msg::commands::shortcuts_panel_trigger_command).into(),
                "/".into(),
            ),
            row(
                tr(cx, msg::commands::shortcuts_panel_trigger_file).into(),
                "@".into(),
            ),
            row(
                tr(cx, msg::commands::shortcuts_panel_trigger_skill).into(),
                "$".into(),
            ),
            row(tr(cx, msg::native::emoji).into(), ":".into()),
        ];
        let left = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(section(
                tr(cx, msg::commands::command_group_composer),
                rows(&COMPOSER_COMMANDS),
            ))
            .child(section(
                tr(cx, msg::commands::shortcuts_panel_triggers),
                triggers,
            ));
        let right = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .child(section(
                tr(cx, msg::commands::shortcuts_panel_chat),
                rows(&CHAT_COMMANDS),
            ))
            .child(section(
                tr(cx, msg::commands::command_group_application),
                rows(&APP_COMMANDS),
            ));
        let card = popover::dialog_card(&theme)
            .id("shortcuts-panel")
            .w(px(672.0).min((viewport.width - px(32.0)).max(px(1.0))))
            .max_h(viewport.height * 0.8)
            .overflow_y_scroll()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_shortcuts_panel(cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(popover::dialog_title(
                        &theme,
                        &tr(cx, msg::commands::shortcuts_panel_title),
                    ))
                    .child(
                        div()
                            .id("shortcuts-panel-close")
                            .size(px(28.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .role(gpui::Role::Button)
                            .aria_label(tr(cx, msg::commands::shortcuts_panel_close))
                            .hover(|style| style.bg(theme.element_hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_shortcuts_panel(cx);
                            }))
                            .child(
                                crate::icons::icon(crate::icons::CLOSE)
                                    .size(px(16.0))
                                    .text_color(theme.text_muted),
                            ),
                    ),
            )
            .child(div().mt(px(4.0)).child(popover::dialog_body(
                &theme,
                tr(cx, msg::commands::shortcuts_panel_description),
            )))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .when(viewport.width >= px(640.0), |el| {
                        el.flex_row().gap(px(32.0))
                    })
                    .when(viewport.width < px(640.0), |el| el.flex_col().gap(px(16.0)))
                    .child(left)
                    .child(right),
            )
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .flex_row()
                    .justify_between()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(
                            &theme,
                            &tr(cx, msg::commands::shortcuts_panel_customize),
                            "shortcuts-panel-customize",
                        )
                        .id("shortcuts-panel-customize")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.close_shortcuts_panel(cx);
                            this.open_settings(SettingsSection::Shortcuts, cx);
                        })),
                    ),
            )
            .into_any_element();
        Some(popover::modal("shortcuts-panel-dialog", viewport, card))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_press_confirms_only_inside_the_window() {
        let now = Instant::now();
        let tasks = vec!["task-a".to_owned()];
        let armed = StopConfirmation {
            chat_id: "chat-a".into(),
            tasks: tasks.clone(),
            at: now,
        };
        assert!(!stop_press_confirms(None, "chat-a", &tasks, now));
        assert!(stop_press_confirms(
            Some(&armed),
            "chat-a",
            &tasks,
            now + Duration::from_secs(2)
        ));
        assert!(!stop_press_confirms(Some(&armed), "chat-b", &tasks, now));
        assert!(!stop_press_confirms(
            Some(&armed),
            "chat-a",
            &["new-task".into()],
            now
        ));
        assert!(!stop_press_confirms(
            Some(&armed),
            "chat-a",
            &tasks,
            now + STOP_CONFIRM_WINDOW + Duration::from_millis(1)
        ));
    }

    #[test]
    fn the_panel_lists_the_live_keymap() {
        let mut keymap = KeymapConfig::default();
        let rows = panel_rows(&keymap, &CHAT_COMMANDS);
        assert_eq!(rows.len(), CHAT_COMMANDS.len());
        assert!(rows.contains(&(Command::OpenModelPicker, display_combo("alt-p"))));
        // A rebind shows its new combo, and a cleared binding drops out.
        keymap.set(
            ShortcutId::Desktop(Command::FindInChat),
            "mod-shift-f".into(),
        );
        keymap.set(ShortcutId::Desktop(Command::ToggleFastMode), String::new());
        let rows = panel_rows(&keymap, &CHAT_COMMANDS);
        assert!(rows.contains(&(Command::FindInChat, display_combo("mod-shift-f"))));
        assert!(
            !rows
                .iter()
                .any(|(label, _)| *label == Command::ToggleFastMode)
        );
        let app = panel_rows(&keymap, &APP_COMMANDS);
        assert!(app.contains(&(Command::ShowShortcuts, display_combo("mod-/"))));
    }
}
