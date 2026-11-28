//! Native preference adoption through the active engine's shared consent.
use crate::{settings::UiSettings, state::AppState};
use gpui::{App, Entity, Global, WeakEntity};
use zuse_analytics::desktop::{Event, Preference};

#[derive(Default)]
struct NativeAnalytics {
    active: Option<(
        WeakEntity<AppState>,
        std::sync::Arc<std::sync::Mutex<Option<String>>>,
    )>,
    states: Vec<WeakEntity<AppState>>,
}
impl Global for NativeAnalytics {}

pub(crate) fn register(state: &Entity<AppState>, cx: &mut App) {
    if cx.try_global::<NativeAnalytics>().is_none() {
        cx.set_global(NativeAnalytics::default());
    }
    let registry = cx.global_mut::<NativeAnalytics>();
    registry.states.retain(|entry| entry.upgrade().is_some());
    if !registry
        .states
        .iter()
        .any(|entry| *entry == state.downgrade())
    {
        registry.states.push(state.downgrade());
    }
}
pub(crate) fn activate(state: &Entity<AppState>, cx: &mut App) {
    register(state, cx);
    let epoch = state.read(cx).desktop_epoch.clone();
    cx.global_mut::<NativeAnalytics>().active = Some((state.downgrade(), epoch));
}
pub(crate) fn bind(state: &Entity<AppState>, cx: &mut App) {
    activate(state, cx);
}
pub(crate) fn states(cx: &App) -> Vec<Entity<AppState>> {
    cx.try_global::<NativeAnalytics>()
        .map(|registry| {
            registry
                .states
                .iter()
                .filter_map(WeakEntity::upgrade)
                .collect()
        })
        .unwrap_or_default()
}

fn changes(before: &UiSettings, after: &UiSettings) -> Vec<Event> {
    let mut events = Vec::new();
    macro_rules! changed {
        ($category:ident, $($field:ident),+ $(,)?) => {
            if false $(|| before.$field != after.$field)+ {
                events.push(Event::PreferenceChanged { preference: Preference::$category });
            }
        };
    }
    changed!(FileWordWrap, files_word_wrap);
    changed!(ShowAllFiles, files_show_all);
    changed!(
        Typography,
        ui_font_family,
        ui_font_size,
        terminal_font_family,
        terminal_font_size,
        code_font_family,
        code_font_size
    );
    changed!(Appearance, appearance, theme_selection, accent, surface);
    changed!(Language, language);
    changed!(
        Composer,
        composer_send_behavior,
        escape_stops_active_agent,
        direct_steer_enabled,
        compact_model_picker,
        runner_game_enabled,
        skills_in_slash_menu,
        skill_completion_by_harness
    );
    changed!(
        Notifications,
        notifications_enabled,
        notifications_background_only,
        agent_update_notifications
    );
    changed!(
        Sound,
        sound_enabled,
        sound_completion_enabled,
        sound_input_enabled,
        sound_attention_enabled,
        completion_sound_preset
    );
    changed!(Dictation, dictation_enabled);
    changed!(Shortcuts, keymap);
    changed!(
        Sidebar,
        sidebar_organization,
        sidebar_sort,
        sidebar_compact,
        sidebar_project_density,
        sidebar_show_project_label,
        sidebar_show_project_icon,
        sidebar_show_harness,
        sidebar_show_branch,
        sidebar_show_pull_request,
        sidebar_show_project_filter,
        sidebar_show_archived
    );
    changed!(
        Browser,
        browser_appearance,
        browser_zoom,
        open_web_links_in_zuse
    );
    changed!(Diff, diff_split, diff_wrap);
    changed!(
        Transcript,
        code_fences_fit_content,
        transcript_width,
        transcript_compact_mode
    );
    changed!(
        FileAutosave,
        files_autosave_enabled,
        files_autosave_delay_ms
    );
    changed!(Appshots, appshots_enabled, appshot_sound_enabled);
    changed!(DefaultModel, default_model);
    changed!(DefaultPermissions, default_permission_mode);
    events
}

pub(crate) fn preferences_changed(before: &UiSettings, after: &UiSettings, cx: &mut App) {
    let Some((state, epoch)) = cx
        .try_global::<NativeAnalytics>()
        .and_then(|binding| binding.active.clone())
    else {
        return;
    };
    let observed_epoch = epoch.lock().unwrap().clone();
    let events = changes(before, after);
    if !events.is_empty() {
        // Capture the originating window before deferring to avoid nested entity updates.
        cx.defer(move |cx| {
            let _ = state.update(cx, |state, cx| {
                if observed_epoch.is_some()
                    && *state.desktop_epoch.lock().unwrap() == observed_epoch
                {
                    state.record_desktop_events(events, cx);
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_report_categories_without_values_or_geometry_noise() {
        let before = UiSettings::default();
        let mut after = before.clone();
        after.display_name = "private name".into();
        after.wallpaper_folder = Some("/private/folder".into());
        after.sidebar_width += 10.0;
        assert!(changes(&before, &after).is_empty());
        after.files_word_wrap = !before.files_word_wrap;
        assert_eq!(
            changes(&before, &after),
            vec![Event::PreferenceChanged {
                preference: Preference::FileWordWrap
            }]
        );
        assert!(changes(&after, &after).is_empty());
    }
}
