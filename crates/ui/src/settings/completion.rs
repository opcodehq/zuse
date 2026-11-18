//! Per-agent composer completion preferences inside Settings → Providers.

use super::HarnessesPage;
use crate::{settings, settings::widgets, theme::Theme};
use gpui::{AnyElement, Context, div, prelude::*};
use zuse_protocol::HarnessId;

/// One row of the Completion block; the variant picks the preference it flips.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CompletionToggle {
    Dollar,
    SeparateSlash,
    ProviderCommands,
}

impl CompletionToggle {
    fn key(self) -> &'static str {
        match self {
            Self::Dollar => "dollar",
            Self::SeparateSlash => "separate",
            Self::ProviderCommands => "provider-commands",
        }
    }
}

impl HarnessesPage {
    fn toggle_completion(
        &mut self,
        harness: HarnessId,
        toggle: CompletionToggle,
        cx: &mut Context<Self>,
    ) {
        settings::update(settings::SavePolicy::Immediate, cx, |settings| {
            let mut preferences = settings.skill_completion(harness);
            match toggle {
                CompletionToggle::Dollar => preferences.dollar = !preferences.dollar,
                CompletionToggle::SeparateSlash => {
                    preferences.separate_from_slash = !preferences.separate_from_slash;
                }
                CompletionToggle::ProviderCommands => {
                    preferences.provider_commands = !preferences.provider_commands;
                }
            }
            settings
                .skill_completion_by_harness
                .insert(harness, preferences);
        });
        cx.notify();
    }

    pub(super) fn render_completion_for(
        &self,
        harness: HarnessId,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = settings::current(cx);
        let preferences = current.skill_completion(harness);
        let rows = [
            (
                CompletionToggle::Dollar,
                "Use $ for skills",
                "Type $ in the composer to pick a skill.",
                preferences.dollar,
            ),
            (
                CompletionToggle::SeparateSlash,
                "Separate / commands",
                "Keep skills out of the / menu.",
                preferences.separate_from_slash,
            ),
            (
                CompletionToggle::ProviderCommands,
                "Provider commands",
                "Show this agent's own / commands in the composer.",
                preferences.provider_commands,
            ),
        ]
        .into_iter()
        .enumerate()
        .map(|(ix, (toggle, label, description, enabled))| {
            let key = toggle.key();
            widgets::details_row(theme, ix == 0)
                .id(format!("completion-{harness:?}-{key}"))
                .role(gpui::Role::Switch)
                .aria_label(format!("{harness:?}: {label}"))
                .aria_toggled(if enabled {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .tab_index(0)
                .cursor_pointer()
                .focus_visible(|s| s.border_2().border_color(theme.accent))
                .on_click(
                    cx.listener(move |page, _, _, cx| page.toggle_completion(harness, toggle, cx)),
                )
                .on_key_down(cx.listener(move |page, event: &gpui::KeyDownEvent, _, cx| {
                    if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        page.toggle_completion(harness, toggle, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(widgets::details_text(theme, label, description))
                .child(widgets::toggle_switch(
                    theme,
                    enabled,
                    format!("completion-switch-{harness:?}-{key}"),
                ))
        });
        div()
            .flex()
            .flex_col()
            .child(widgets::details_label(theme, "Completion"))
            .children(rows)
            .into_any_element()
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    #[gpui::test]
    fn completion_preferences_save_independently_per_agent(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| settings::init(Default::default(), dir.path(), cx));
        let state = cx.new(|_| crate::state::AppState::new());
        let page = cx.new(|cx| HarnessesPage::new(state, cx));
        page.update(cx, |page, cx| {
            page.toggle_completion(HarnessId::ClaudeCode, CompletionToggle::Dollar, cx);
            page.toggle_completion(HarnessId::ClaudeCode, CompletionToggle::SeparateSlash, cx);
            page.toggle_completion(HarnessId::Opencode, CompletionToggle::Dollar, cx);
            page.toggle_completion(HarnessId::Opencode, CompletionToggle::ProviderCommands, cx);
        });
        // Both default on, so each flip turns its own preference off.
        let loaded = settings::UiSettings::load(dir.path());
        assert!(!loaded.skill_completion(HarnessId::ClaudeCode).dollar);
        assert!(
            !loaded
                .skill_completion(HarnessId::ClaudeCode)
                .separate_from_slash
        );
        assert!(!loaded.skill_completion(HarnessId::Opencode).dollar);
        assert!(
            loaded
                .skill_completion(HarnessId::Opencode)
                .separate_from_slash
        );
        // The new switch is independent per agent, like the older two.
        assert!(
            !loaded
                .skill_completion(HarnessId::Opencode)
                .provider_commands
        );
        assert!(
            loaded
                .skill_completion(HarnessId::ClaudeCode)
                .provider_commands
        );
        assert!(
            loaded
                .skill_completion_by_harness
                .contains_key(&HarnessId::ClaudeCode)
        );
    }
}
