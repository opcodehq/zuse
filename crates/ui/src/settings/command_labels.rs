//! Localized labels for the new conversation and composer shortcuts.
use super::{ShortcutId, keybindings::Command};
use gpui::App;
use zuse_i18n::{Localizer, messages::native};

impl ShortcutId {
    pub fn localized_label(self, cx: &App) -> String {
        let Self::Desktop(command) = self else {
            return self.label().into();
        };
        let format: fn(&Localizer) -> String = match command {
            Command::ComposerPreviousMessage => native::shortcut_previous_message,
            Command::ComposerNextMessage => native::shortcut_next_message,
            Command::ComposerQueueSubmit => native::shortcut_queue_message,
            Command::ComposerDeleteToStart => native::shortcut_delete_line_start,
            Command::ComposerDeleteToEnd => native::shortcut_delete_line_end,
            Command::ComposerDeleteWord => native::shortcut_delete_word,
            Command::ComposerInterrupt => native::shortcut_stop_agent,
            Command::ShowShortcuts => native::shortcut_show_shortcuts,
            Command::FindInChat => native::shortcut_find_in_chat,
            Command::OpenModelPicker => native::shortcut_model_picker,
            Command::CycleReasoningEffort => native::shortcut_reasoning,
            Command::ToggleFastMode => native::shortcut_fast_mode,
            Command::ToggleToolDetails => native::shortcut_tool_details,
            Command::ToggleTaskList => native::shortcut_task_list,
            Command::StopBackgroundTasks => native::shortcut_stop_background,
            Command::ComposerYank => native::shortcut_yank,
            Command::ComposerYankPop => native::shortcut_yank_pop,
            Command::ComposerStash => native::shortcut_stash,
            Command::ComposerSearchHistory => native::shortcut_history,
            _ => return self.label().into(),
        };
        crate::localization::message(cx, format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn new_command_labels_follow_the_interface_language(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            zuse_viewport::localization::configure(
                zuse_i18n::LanguagePreference::Explicit(zuse_i18n::Locale::Fr),
                vec![],
                cx,
            );
            assert_eq!(
                ShortcutId::Desktop(Command::ComposerStash).localized_label(cx),
                "Mettre le brouillon de côté"
            );
            assert_eq!(
                ShortcutId::Desktop(Command::FindInChat).localized_label(cx),
                "Rechercher dans le chat"
            );
        });
    }
}
