//! Automatic thread titles reuse the model picker and the connected host's catalog.
use super::*;
use crate::localization::message as tr;
use crate::{
    assets::icon,
    model_picker::PickerTarget,
    settings_style as s,
    shell::{column, row},
};
use zuse_i18n::messages as msg;
use zuse_protocol::TitleSettings;

pub(super) struct TitleSettingsView {
    pub settings: Option<TitleSettings>,
    pub focus: FocusHandle,
    window: AnyWindowHandle,
    confirmed: bool,
    read: Option<u64>,
    save: Option<u64>,
    error: Option<String>,
    attempt: Option<TitleSettings>,
}
impl TitleSettingsView {
    pub(super) fn new(window: &Window, cx: &mut Context<WorkspaceView>) -> Self {
        Self {
            settings: None,
            focus: cx.focus_handle(),
            window: window.window_handle(),
            confirmed: false,
            read: None,
            save: None,
            error: None,
            attempt: None,
        }
    }
    pub(super) fn provider(&self) -> Option<String> {
        serde_json::to_value(self.settings.as_ref()?.harness?)
            .ok()?
            .as_str()
            .map(str::to_owned)
    }
}
impl WorkspaceView {
    pub(super) fn refresh_title_settings(&mut self) {
        if !self.connected
            || self.title_settings.read.is_some()
            || self.title_settings.save.is_some()
        {
            return;
        }
        self.title_settings.read = Some(self.call(methods::GET_TITLE_SETTINGS, json!({})));
    }
    pub(super) fn interrupt_title_settings(&mut self) {
        if self.title_settings.save.take().is_some() {
            self.title_settings.error = Some("The naming preference may have saved before disconnection. It will be read after reconnecting; the change will not be repeated automatically.".into());
        }
        self.title_settings.read = None;
        self.title_settings.confirmed = false;
        if self.picker_target == PickerTarget::ThreadNaming {
            self.model_picker = false;
            self.picker_target = PickerTarget::Composer;
        }
    }
    pub(super) fn save_title_model(
        &mut self,
        provider: Option<String>,
        model: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if !self.connected || !self.title_settings.confirmed || self.title_settings.save.is_some() {
            return;
        }
        let harness = if let Some(provider) = provider {
            if !self.providers.iter().any(|row| {
                row["id"] == provider && row["installed"] != false && row["enabled"] != false
            }) {
                self.title_settings.error =
                    Some("Choose an installed, enabled agent on this computer.".into());
                cx.notify();
                return;
            }
            match serde_json::from_value(json!(provider)) {
                Ok(harness) => Some(harness),
                Err(_) => {
                    self.title_settings.error =
                        Some("The agent returned an invalid identity.".into());
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        let settings = TitleSettings {
            harness,
            model: model.filter(|id| !id.trim().is_empty() && id != "default"),
        };
        if let Some(id) = self.title_settings.read.take() {
            self.cancel(id);
        }
        self.title_settings.error = None;
        self.title_settings.attempt = Some(settings.clone());
        self.title_settings.save = Some(self.call(
            methods::SET_TITLE_SETTINGS,
            serde_json::to_value(settings).unwrap(),
        ));
        cx.notify();
    }
    pub(super) fn receive_title_settings(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let read = self.title_settings.read == Some(id);
        let save = self.title_settings.save == Some(id);
        if !read && !save {
            return false;
        }
        let value = frame.get("ok").or_else(|| frame.get("item"));
        if value.is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        self.pending.remove(&id);
        if read {
            self.title_settings.read = None;
        }
        if save {
            self.title_settings.save = None;
        }
        let result = value
            .ok_or_else(|| {
                frame["err"]
                    .as_str()
                    .unwrap_or(
                        "The naming preference could not be confirmed. Refresh before retrying.",
                    )
                    .to_owned()
            })
            .and_then(|value| {
                serde_json::from_value::<TitleSettings>(value.clone())
                    .map_err(|_| "The engine returned invalid thread naming settings.".to_owned())
            });
        match result {
            Ok(settings) => {
                let attempt = self.title_settings.attempt.take();
                let reconciled = attempt.as_ref().is_some_and(|attempt| {
                    attempt.harness == settings.harness && attempt.model == settings.model
                });
                self.title_settings.settings = Some(settings);
                self.title_settings.confirmed = true;
                if save || attempt.is_none() || reconciled {
                    self.title_settings.error = None;
                }
                if (save || attempt.is_some()) && self.picker_target == PickerTarget::ThreadNaming {
                    self.model_picker = false;
                    self.picker_target = PickerTarget::Composer;
                    let focus = self.title_settings.focus.clone();
                    let window = self.title_settings.window;
                    cx.defer(move |cx| {
                        let _ = cx.update_window(window, |_, window, cx| window.focus(&focus, cx));
                    });
                }
            }
            Err(error) => {
                self.title_settings.error = Some(error);
                self.title_settings.confirmed = false;
                if save {
                    self.refresh_title_settings();
                }
            }
        }
        cx.notify();
        true
    }
    pub(super) fn thread_naming_settings_card(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.title_settings;
        let provider = state.provider();
        let model = state
            .settings
            .as_ref()
            .and_then(|settings| settings.model.as_deref());
        let label = if state.save.is_some() {
            "Saving…".to_owned()
        } else if state.settings.is_none() {
            "Choose title model".into()
        } else if provider.is_none() {
            "Follow thread agent".into()
        } else {
            provider
                .as_ref()
                .and_then(|provider| self.picker_catalog.providers.get(provider))
                .and_then(|catalog| {
                    catalog
                        .models
                        .iter()
                        .find(|row| Some(row.id.as_str()) == model)
                })
                .map(|model| model.label.clone())
                .or_else(|| model.map(str::to_owned))
                .unwrap_or("Agent default".into())
        };
        let enabled = self.connected && state.confirmed && state.save.is_none();
        let bounds = self.picker_menu.trigger.clone();
        let measure = bounds.clone();
        let open = self.model_picker && self.picker_target == PickerTarget::ThreadNaming;
        let trigger = column()
            .relative()
            .w(px(256.))
            .child(
                canvas(
                    move |bounds, _, _| measure.set(Some(bounds)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(
                s::button("thread-naming-model", "")
                    .w_full()
                    .justify_between()
                    .role(Role::ComboBox)
                    .aria_label(format!("Thread naming model: {label}"))
                    .aria_expanded(open)
                    .track_focus(&state.focus)
                    .tab_index(0)
                    .opacity(if enabled { 1. } else { 0.45 })
                    .child(
                        row()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .when_some(provider.clone(), |view, provider| {
                                view.child(
                                    icon(crate::shell::provider_icon(&provider)).size(px(14.)),
                                )
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(label),
                            ),
                    )
                    .child(icon("alt-arrow-down").size(px(14.)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !enabled {
                            return;
                        }
                        this.open_settings_model_picker(PickerTarget::ThreadNaming, window, cx);
                        this.default_permission_menu = false;
                        this.completion_preset_menu = false;
                    })),
            )
            .when(open, |view| {
                view.child(deferred(self.composer_picker(cx, Some(bounds))).priority(1))
            });
        s::frame().child(s::header(tr(cx, msg::native::thread_naming), tr(cx, msg::native::choose_which_agent_and_model_automatically_name_new_threads_on_this_computer)))
            .child(s::card()
                .child(s::setting_row(true).child(s::details(tr(cx, msg::native::title_model), tr(cx, msg::native::follow_the_thread_s_agent_or_choose_a_specific_model_this_does_not_change_your_chat_model) )).child(trigger))
                .child(s::setting_row(false).justify_end()
                    .child(s::button("refresh-thread-naming", tr(cx, msg::settings::organizations_refresh)).on_click(cx.listener(|this, _, _, cx| { this.refresh_title_settings(); cx.notify(); })))
                    .child(s::button("reset-thread-naming", tr(cx, msg::native::follow_thread_agent)).opacity(if enabled && provider.is_some() { 1. } else { 0.45 })
                        .on_click(cx.listener(move |this, _, _, cx| { if enabled && provider.is_some() { this.save_title_model(None, None, cx); } })))))
            .when_some(state.error.clone(), |view, error| view.child(s::description_text(error).text_color(rgb(0xec9292)).px_3().py_2()))
            .into_any_element()
    }
}
