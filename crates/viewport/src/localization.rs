//! Browser language is a local presentation preference, independent of the engine.
use super::*;
use zuse_i18n::{LanguagePreference, Localizer};

pub struct Localization {
    pub localizer: Localizer,
    pub preference: LanguagePreference,
    pub languages: Vec<String>,
}
impl Global for Localization {}
pub fn initialize(cx: &mut App) {
    if !cx.has_global::<Localization>() {
        let languages = zuse_i18n::system_languages();
        cx.set_global(Localization {
            localizer: Localizer::new(LanguagePreference::System.resolve(&languages)),
            preference: LanguagePreference::System,
            languages,
        });
    }
}
/// Install the application context before the first frame; persistence stays with the host.
pub fn configure(preference: LanguagePreference, languages: Vec<String>, cx: &mut App) {
    let locale = preference.resolve(&languages);
    cx.set_global(Localization {
        localizer: Localizer::new(locale),
        preference,
        languages,
    });
}
pub fn text(id: &str, cx: &App) -> String {
    cx.try_global::<Localization>().map_or_else(
        || Localizer::default().text(id),
        |state| state.localizer.text(id),
    )
}
impl WorkspaceView {
    pub(super) fn receive_localization(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(value) = frame.get("localization") else {
            return false;
        };
        initialize(cx);
        let state = cx.global_mut::<Localization>();
        if let Some(languages) = value["languages"].as_array() {
            state.languages = languages
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
        }
        if let Some(preference) = value.get("preference") {
            state.preference = serde_json::from_value(preference.clone()).unwrap_or_default();
        }
        state.localizer = Localizer::new(state.preference.resolve(&state.languages));
        let locale = state.localizer.locale().code();
        let _ = self
            .requests
            .unbounded_send(json!({"ui":"documentLanguage","language":locale}));
        if value["saveFailed"] == true {
            self.error = Some(state.localizer.text("common-saveLanguageError"));
        }
        if !self.connected && !self.paired && !self.account_remote {
            self.status = zuse_i18n::messages::native::reconnecting(&state.localizer);
        }
        let placeholder = zuse_i18n::messages::native::composer_placeholder(&state.localizer);
        self.input
            .update(cx, |input, cx| input.set_placeholder(placeholder, cx));
        cx.refresh_windows();
        cx.notify();
        true
    }
    fn choose_language(&mut self, preference: LanguagePreference, cx: &mut Context<Self>) {
        // Persistence acknowledgement is applied through receive_localization;
        // storage failures leave the previous language selected.
        let _ = self
            .requests
            .unbounded_send(json!({"ui":"saveLanguage","preference":preference}));
        cx.notify();
    }
    pub(super) fn language_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::{
            settings_style as s,
            shell::{column, row},
        };
        let state = cx.global::<Localization>();
        let selected = state.preference;
        let options = std::iter::once((
            LanguagePreference::System,
            state.localizer.text("common-system"),
        ))
        .chain(
            zuse_i18n::Locale::ALL
                .into_iter()
                .map(|locale| (LanguagePreference::Explicit(locale), locale.name().into())),
        )
        .enumerate()
        .map(|(index, (preference, name))| {
            s::button(("language-option", index), name)
                .role(Role::RadioButton)
                .aria_selected(selected == preference)
                .when(selected == preference, |button| {
                    button
                        .bg(s::frame_bg())
                        .border_color(crate::palette::rgb(0x89ca21))
                })
                .aria_toggled(if selected == preference {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .tab_index(0)
                .on_click(cx.listener(move |this, _, _, cx| this.choose_language(preference, cx)))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        this.choose_language(preference, cx);
                        cx.stop_propagation();
                    }
                }))
        });
        s::frame()
            .child(s::header(
                message(cx, zuse_i18n::messages::native::language_settings),
                "",
            ))
            .child(
                s::card().child(
                    column()
                        .gap_2()
                        .p_3()
                        .child(
                            row()
                                .id("language-choices")
                                .flex_wrap()
                                .gap_2()
                                .role(Role::RadioGroup)
                                .aria_label(message(
                                    cx,
                                    zuse_i18n::messages::native::language_settings,
                                ))
                                .children(options),
                        )
                        .when(state.localizer.locale() != zuse_i18n::Locale::En, |view| {
                            view.child(s::description_text(message(
                                cx,
                                zuse_i18n::messages::native::language_settings_description,
                            )))
                        }),
                ),
            )
            .into_any_element()
    }
}

/// Typed message accessor; required interpolation arguments stay at the call site.
pub fn message(cx: &App, format: impl FnOnce(&Localizer) -> String) -> String {
    match cx.try_global::<Localization>() {
        Some(state) => format(&state.localizer),
        None => format(&Localizer::default()),
    }
}

/// Resolves text during rendering, including image loading/fallback callbacks.
#[derive(gpui::IntoElement)]
pub struct MessageElement(fn(&Localizer) -> String);
impl gpui::RenderOnce for MessageElement {
    fn render(self, _: &mut gpui::Window, cx: &mut App) -> impl gpui::IntoElement {
        gpui::SharedString::from(message(cx, self.0))
    }
}
pub fn element(format: fn(&Localizer) -> String) -> MessageElement {
    MessageElement(format)
}

pub fn appearance_label(mode: zuse_protocol::presentation::AppearanceMode, cx: &App) -> String {
    use zuse_i18n::messages::{common, settings};
    use zuse_protocol::presentation::AppearanceMode;
    message(
        cx,
        match mode {
            AppearanceMode::System => common::system,
            AppearanceMode::Light => settings::settings_page_light,
            AppearanceMode::Dark => settings::settings_page_dark,
        },
    )
}

pub fn localizer(cx: &App) -> Localizer {
    cx.try_global::<Localization>()
        .map(|state| state.localizer.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::prelude::v1::test;
    #[gpui::test]
    fn language_changes_keep_unsent_input_and_failed_storage_keeps_selection(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::initialize);
        let (requests, mut outbound) = futures::channel::mpsc::unbounded();
        let (_events, inbound) = futures::channel::mpsc::unbounded();
        let workspace = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: inbound,
                },
                window,
                cx,
            )
        });
        workspace
            .update(cx, |view, _, cx| {
                view.input
                    .update(cx, |input, cx| input.set_value("未发送的草稿".into(), cx));
            })
            .unwrap();
        // Editor mutations are deferred until the window update completes.
        workspace
            .update(cx, |view, _, cx| {
                assert_eq!(view.input.read(cx).value(cx), "未发送的草稿");
                view.receive_localization(
                    &json!({"localization":{"languages":["zh-Hant-TW"],"preference":"system"}}),
                    cx,
                );
                assert_eq!(localizer(cx).locale(), zuse_i18n::Locale::ZhHant);
                assert_eq!(view.input.read(cx).value(cx), "未发送的草稿");
                view.choose_language(LanguagePreference::Explicit(zuse_i18n::Locale::Fr), cx);
                assert_eq!(
                    localizer(cx).locale(),
                    zuse_i18n::Locale::ZhHant,
                    "no switch before persistence acknowledgement"
                );
                view.receive_localization(&json!({"localization":{"saveFailed":true}}), cx);
                assert_eq!(localizer(cx).locale(), zuse_i18n::Locale::ZhHant);
                assert!(view.error.is_some());
                view.receive_localization(&json!({"localization":{"preference":"fr"}}), cx);
                assert_eq!(localizer(cx).locale(), zuse_i18n::Locale::Fr);
                assert_eq!(view.input.read(cx).value(cx), "未发送的草稿");
                view.receive_localization(&json!({"localization":{"languages":["ko-KR"]}}), cx);
                assert_eq!(
                    localizer(cx).locale(),
                    zuse_i18n::Locale::Fr,
                    "explicit choice wins over browser language changes"
                );
            })
            .unwrap();
        // The same channel carries local presentation actions, never engine RPCs.
        let mut saved = false;
        while let Ok(frame) = outbound.try_recv() {
            if frame["ui"] == "saveLanguage" {
                saved = true;
                assert_eq!(frame["preference"], "fr");
            }
        }
        assert!(saved);
    }
}

pub fn access_label(level: zuse_protocol::SandboxLevel, cx: &App) -> String {
    use zuse_i18n::messages::chat;
    match level {
        zuse_protocol::SandboxLevel::WorkspaceWrite => {
            message(cx, chat::runtime_mode_ask_for_approval)
        }
        zuse_protocol::SandboxLevel::Auto => message(cx, chat::runtime_mode_approve_for_me),
        zuse_protocol::SandboxLevel::DangerFullAccess => {
            message(cx, chat::runtime_mode_meta_full_access)
        }
        zuse_protocol::SandboxLevel::ReadOnly => "Read only".into(),
    }
}

pub fn motion_label(mode: zuse_protocol::presentation::motion::ReduceMotion, cx: &App) -> String {
    use zuse_i18n::messages::{common, settings};
    use zuse_protocol::presentation::motion::ReduceMotion;
    message(
        cx,
        match mode {
            ReduceMotion::System => common::system,
            ReduceMotion::On => settings::diagnostics_pane_on,
            ReduceMotion::Off => settings::settings_page_off,
        },
    )
}
