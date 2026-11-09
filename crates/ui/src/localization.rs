//! Native persistence for the localization context shared with browser-compatible views.
use gpui::{App, Global};
use zuse_i18n::LanguagePreference;

pub use zuse_viewport::localization::{Localization, element, localizer, message, text};

pub struct LanguageSaveError(pub Option<String>);
impl Global for LanguageSaveError {}

pub fn init(preference: LanguagePreference, cx: &mut App) {
    zuse_viewport::localization::configure(preference, zuse_i18n::system_languages(), cx);
    cx.set_global(LanguageSaveError(None));
}
pub fn set(preference: LanguagePreference, cx: &mut App) {
    if crate::settings::set_language(preference, cx).is_err() {
        let error = message(cx, zuse_i18n::messages::common::saveLanguageError);
        cx.set_global(LanguageSaveError(Some(error)));
        cx.refresh_windows();
        return;
    }
    init(preference, cx);
    let menus = crate::app_menus::app_menus(cx);
    cx.set_menus(menus);
    cx.refresh_windows();
}

/// System preferences can change while the application is inactive.
pub fn refresh_system(cx: &mut App) {
    let preference = crate::settings::current(cx).language;
    if preference != LanguagePreference::System {
        return;
    }
    let locale = preference.resolve(&zuse_i18n::system_languages());
    if localizer(cx).locale() == locale {
        return;
    }
    init(preference, cx);
    let menus = crate::app_menus::app_menus(cx);
    cx.set_menus(menus);
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;
    #[gpui::test]
    fn persistence_failure_retains_language_and_other_settings(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let blocked = directory.path().join("file-instead-of-directory");
        std::fs::write(&blocked, "blocked").unwrap();
        cx.update(|cx| {
            let mut settings = crate::settings::UiSettings::default();
            settings.display_name = "Keep this name".into();
            crate::settings::init(settings, blocked, cx);
            init(LanguagePreference::System, cx);
            let before = localizer(cx).locale();
            set(LanguagePreference::Explicit(zuse_i18n::Locale::ZhHans), cx);
            assert_eq!(localizer(cx).locale(), before);
            assert_eq!(crate::settings::current(cx).display_name, "Keep this name");
            assert!(cx.global::<LanguageSaveError>().0.is_some());
        });
    }
    #[gpui::test]
    fn native_and_shared_views_use_one_context_and_persist_together(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), directory.path(), cx);
            init(LanguagePreference::System, cx);
            set(LanguagePreference::Explicit(zuse_i18n::Locale::ZhHant), cx);
            assert_eq!(message(cx, zuse_i18n::messages::common::save), "儲存");
            assert_eq!(
                zuse_viewport::localization::message(cx, zuse_i18n::messages::common::save),
                "儲存"
            );
            assert_eq!(
                crate::settings::UiSettings::load(directory.path()).language,
                LanguagePreference::Explicit(zuse_i18n::Locale::ZhHant)
            );
        });
    }
}
