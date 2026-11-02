//! Combined provider/model picker, following Zuse's provider rail and model list.
pub(super) mod view;
use super::*;
use crate::assets::icon;
use crate::shell::{BORDER, MUTED, TEXT, column, control, label, provider_icon, row};
use zuse_protocol::{Model, presentation::ModelSelection};
const ACCENT: u32 = 0x89ca21;
pub(super) type TriggerBounds = std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum PickerTarget {
    #[default]
    Composer,
    Defaults,
    ThreadNaming,
}

impl WorkspaceView {
    pub(super) fn picker_in_settings(&self) -> bool {
        self.picker_target != PickerTarget::Composer
    }
    fn picker_current_provider(&self) -> String {
        match self.picker_target {
            PickerTarget::Composer => self
                .composer_provider()
                .and_then(|provider| provider["id"].as_str())
                .unwrap_or_default()
                .to_owned(),
            PickerTarget::Defaults => self
                .preferences
                .default_provider
                .clone()
                .or_else(|| {
                    self.providers
                        .iter()
                        .find(|p| p["installed"] != false && p["enabled"] != false)
                        .and_then(|p| p["id"].as_str())
                        .map(str::to_owned)
                })
                .unwrap_or_default(),
            PickerTarget::ThreadNaming => self.title_settings.provider().unwrap_or_default(),
        }
    }

    pub(super) fn save_preferences(&self) {
        let _ = self
            .requests
            .unbounded_send(json!({"ui":"savePreferences","value":self.preferences}));
    }
    pub(super) fn apply_default_model(&mut self) {
        if self.selected.is_some() {
            return;
        }
        if self.cloud_models_active() {
            self.apply_cloud_default_model();
            return;
        }
        let preferred = self.preferences.starting_model();
        let available =
            |provider: &Value| provider["enabled"] != false && provider["installed"] != false;
        let chosen = preferred
            .as_ref()
            .and_then(|(provider, _)| {
                self.providers
                    .iter()
                    .position(|row| row["id"] == *provider && available(row))
            })
            .or_else(|| self.providers.iter().position(available));
        if let Some(index) = chosen {
            self.provider_index = index;
            self.model = preferred
                .filter(|(provider, _)| self.providers[index]["id"] == *provider)
                .and_then(|(_, model)| model);
            self.restore_model_choices();
        }
    }
    pub(super) fn selected_model(&self) -> Option<&Model> {
        let provider = self.composer_providers().get(self.provider_index)?["id"].as_str()?;
        let models = &self.composer_catalog().providers.get(provider)?.models;
        let id = self.model.as_deref().unwrap_or("default");
        models
            .iter()
            .find(|m| m.id == id)
            .or_else(|| (id == "default").then(|| models.first()).flatten())
    }
    pub(super) fn picker_label(&self) -> String {
        self.selected_model()
            .map(|m| m.label.clone())
            .or_else(|| self.model.clone().filter(|id| id != "default"))
            .unwrap_or_else(|| "Agent default".into())
    }
    pub(super) fn open_model_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings_model_picker(PickerTarget::Composer, window, cx);
    }
    pub(super) fn open_settings_model_picker(
        &mut self,
        target: PickerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.context_menu = false;
        self.model_picker = !(self.model_picker && self.picker_target == target);
        self.picker_target = if self.model_picker {
            target
        } else {
            PickerTarget::Composer
        };
        self.agent_picker = false;
        self.access_picker = false;
        self.add_menu = false;
        if self.model_picker {
            if target == PickerTarget::Composer
                && self.selected.is_some()
                && !self.model_confirmed
                && self.model_watch.is_none()
            {
                self.refresh_model_config_watch();
            }
            self.picker_scope = None;
            self.picker_menu.invalidate();
            self.model_search
                .update(cx, |input, cx| input.set_value(String::new(), cx));
            window.focus(&self.model_search.focus_handle(cx), cx);
            let this = cx.entity().downgrade();
            window.on_next_frame(move |window, cx| {
                let _ = this.update(cx, |this, cx| {
                    if this.model_picker {
                        window.focus(&this.model_search.focus_handle(cx), cx);
                    }
                });
            });
            self.picker_active = None;
            self.configure_reasoning_slider(cx);
            self.load_picker_catalogs(false);
            self.refresh_picker_placeholder(cx);
        }
        cx.notify();
    }
    pub(super) fn return_model_picker_focus(
        &self,
        target: PickerTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(
            &match target {
                PickerTarget::Defaults => self.default_model_focus.clone(),
                PickerTarget::ThreadNaming => self.title_settings.focus.clone(),
                PickerTarget::Composer => self.input.focus_handle(cx),
            },
            cx,
        );
    }
    pub(super) fn receive_picker_catalog(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.receive_host_catalog(id, frame, cx) {
            self.refresh_picker_placeholder(cx);
            return true;
        }
        let local = self.picker_catalog.contains_request(id);
        if !local && !self.model_host.catalog.contains_request(id) {
            return false;
        }
        let result = if let Some(error) = frame["err"].as_str() {
            Err(error.to_owned())
        } else if let Some(value) = frame.get("ok") {
            serde_json::from_value(value.clone())
                .map_err(|_| "The agent returned an invalid model catalog.".into())
        } else if frame["done"] == true {
            Err("The agent closed its model catalog without a result.".into())
        } else {
            return true;
        };
        if local {
            self.picker_catalog.complete(id, result);
        } else {
            self.model_host.catalog.complete(id, result);
        }
        self.invalidate_picker_menu();
        self.pending.remove(&id);
        if self.selected.is_none() {
            self.restore_model_choices();
        }
        self.configure_reasoning_slider(cx);
        self.refresh_picker_placeholder(cx);
        cx.notify();
        true
    }
    fn choose_model(&mut self, provider: String, model: Option<String>, cx: &mut Context<Self>) {
        let Some(index) = self
            .picker_providers()
            .iter()
            .position(|p| p["id"] == provider)
        else {
            return;
        };
        if self.picker_target == PickerTarget::ThreadNaming {
            self.save_title_model(Some(provider), model, cx);
            return;
        }
        if self.picker_target == PickerTarget::Defaults {
            self.preferences.default_provider = Some(provider.clone());
            if let Some(model) = model {
                self.preferences
                    .set_default(ModelSelection { provider, model });
            } else {
                self.preferences.default_models.remove(&provider);
            }
            self.save_preferences();
            self.model_picker = false;
            self.picker_target = model_picker::PickerTarget::Composer;
            cx.notify();
            return;
        }
        if !self.can_change_model(cx) {
            return;
        }
        if self.provider_index == index && self.model == model {
            self.model_picker = false;
            cx.notify();
            return;
        }
        self.provider_index = index;
        self.model = model.clone();
        self.restore_model_choices();
        if let Some(id) = model {
            self.preferences.record_model(ModelSelection {
                provider,
                model: id,
            });
            self.save_preferences();
        }
        self.model_picker = false;
        self.model_choices_changed(cx);
    }
    fn picker_matches(&self, cx: &App) -> Vec<ModelSelection> {
        self.picker_entries(cx)
            .into_iter()
            .map(|entry| ModelSelection {
                provider: entry.provider,
                model: entry.model.id,
            })
            .collect()
    }
    fn on_picker_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.model_picker {
            return;
        }
        let key = event.keystroke.key.as_str();
        if key == "escape" {
            let target = self.picker_target;
            self.model_picker = false;
            self.picker_target = model_picker::PickerTarget::Composer;
            self.return_model_picker_focus(target, window, cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.reasoning_focus.is_focused(window) || self.picker_menu.control_focused(window) {
            return;
        }
        let modifiers = event.keystroke.modifiers;
        let shortcut =
            (modifiers.control || modifiers.platform) && !modifiers.alt && !modifiers.shift;
        if !matches!(key, "up" | "down" | "enter")
            && !(shortcut && key.len() == 1 && key >= "1" && key <= "9")
        {
            return;
        }
        cx.stop_propagation();
        let matches = self.picker_matches(cx);
        if matches.is_empty() {
            return;
        }
        let current = self
            .picker_active
            .as_ref()
            .and_then(|active| matches.iter().position(|m| m == active));
        let index = match key {
            "up" => current.unwrap_or(0).saturating_sub(1),
            "down" => current.map_or(0, |i| (i + 1).min(matches.len() - 1)),
            "enter" => current.unwrap_or(0),
            _ => key.parse::<usize>().unwrap_or(1) - 1,
        };
        let Some(choice) = matches.get(index).cloned() else {
            return;
        };
        if key == "enter" || shortcut {
            let target = self.picker_target;
            self.choose_model(choice.provider, Some(choice.model), cx);
            if !self.model_picker {
                self.return_model_picker_focus(target, window, cx);
            }
        } else {
            self.scroll_picker_row(&choice, cx);
            self.picker_active = Some(choice);
            cx.notify();
        }
    }
    fn effort_control(&self) -> Option<zuse_protocol::presentation::ModelEffort> {
        if !self.cloud_model_configuration_available() {
            return None;
        }
        zuse_protocol::presentation::model_effort(
            self.selected_model()?,
            self.reasoning,
            &self.model_options,
        )
    }
    pub(super) fn picker_effort_label(&self) -> Option<String> {
        let control = self.effort_control()?;
        Some(control.stops.get(control.selected)?.label.clone())
    }
    pub(super) fn select_effort(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(control) = self.effort_control() {
            // Catalog/open synchronization sets the current stop too; it must
            // not silently rewrite a stored override without a user movement.
            if index != control.selected {
                if !self.can_change_model(cx) {
                    self.configure_reasoning_slider(cx);
                    return;
                }
                if control.select(index, &mut self.reasoning, &mut self.model_options) {
                    self.model_choices_changed(cx);
                }
            }
        }
    }
    pub(super) fn configure_reasoning_slider(&mut self, cx: &mut Context<Self>) {
        let Some(control) = self.effort_control() else {
            return;
        };
        let max = control.stops.len().saturating_sub(1).max(1) as f32;
        let index = control.selected as f32;
        let current = self.reasoning_slider.read(cx);
        if current.max_value() != max || current.value().end() != index {
            self.reasoning_slider.update(cx, |state, cx| {
                *state = gpui_base::slider::SliderState::new()
                    .min(0.)
                    .max(max)
                    .step(1.)
                    .default_value(index);
                cx.notify();
            });
        }
    }
    fn reasoning_slider_view(&self, cx: &mut Context<Self>) -> AnyElement {
        use gpui_base::slider::{Slider, SliderIndicator, SliderThumb, SliderTrack};
        let Some(control) = self.effort_control() else {
            return div().into_any_element();
        };
        let current_label = control
            .stops
            .get(control.selected)
            .map(|stop| stop.label.clone())
            .unwrap_or_default();
        let state = &self.reasoning_slider;
        let percentage = state.read(cx).percentage().end;
        let steps = state.read(cx).max_value() as usize;
        column()
            .gap_2()
            .child(
                row()
                    .child(
                        label(control.label.clone())
                            .text_size(px(12.))
                            .font_weight(FontWeight::MEDIUM),
                    )
                    .child(div().flex_1())
                    .child(
                        label(current_label)
                            .text_size(px(11.))
                            .text_color(rgb(TEXT)),
                    ),
            )
            .child(
                div()
                    .id("reasoning-control")
                    .track_focus(&self.reasoning_focus)
                    .tab_stop(true)
                    .role(Role::Group)
                    .aria_label(control.label)
                    .relative()
                    .h(px(32.))
                    .px_2()
                    // The thumb consumes its bubble-phase pointer event, even
                    // when it stays at the current value and emits no change.
                    .capture_any_mouse_down(cx.listener(
                        |this, event: &MouseDownEvent, window, cx| {
                            if event.button == MouseButton::Left {
                                window.focus(&this.reasoning_focus, cx);
                            }
                        },
                    ))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                        let current = this.reasoning_slider.read(cx).value().end();
                        let max = this.reasoning_slider.read(cx).max_value();
                        let value = match event.keystroke.key.as_str() {
                            "left" | "down" => (current - 1.).max(0.),
                            "right" | "up" => (current + 1.).min(max),
                            "home" => 0.,
                            "end" => max,
                            _ => return,
                        };
                        this.reasoning_slider
                            .update(cx, |state, cx| state.set_value(value, window, cx));
                        cx.stop_propagation();
                    }))
                    .child(
                        Slider::new(state).w_full().h(px(32.)).child(
                            SliderTrack::new(state)
                                .relative()
                                .w_full()
                                .h_full()
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .top(px(6.))
                                        .h(px(20.))
                                        .rounded_full()
                                        .bg(rgb(0x32372f)),
                                )
                                .child(
                                    SliderIndicator::new(state)
                                        .absolute()
                                        .left_0()
                                        .right_0()
                                        .top(px(6.))
                                        .h(px(20.))
                                        .child(
                                            div()
                                                .absolute()
                                                .inset_0()
                                                .right(relative(1. - percentage))
                                                .rounded_full()
                                                .bg(rgb(0x89ca21)),
                                        ),
                                )
                                .children((1..steps).map(|step| {
                                    div()
                                        .absolute()
                                        .left(relative(step as f32 / steps.max(1) as f32))
                                        .top(px(13.))
                                        .ml(px(-3.))
                                        .size(px(6.))
                                        .rounded_full()
                                        .bg(rgb(0x858e7c))
                                }))
                                .child(
                                    SliderThumb::new(state)
                                        .absolute()
                                        .left(relative(percentage))
                                        .top(px(4.))
                                        .ml(px(-12.))
                                        .size(px(24.))
                                        .rounded_full()
                                        .border_2()
                                        .border_color(rgb(0x89ca21))
                                        .bg(rgb(0xffffff)),
                                ),
                        ),
                    ),
            )
            .into_any_element()
    }
    fn model_options_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(model) = self.selected_model() else {
            return div().into_any_element();
        };
        let effort = self.effort_control();
        let hidden_option = effort.as_ref().and_then(|control| control.option_id());
        column()
            .flex_shrink_0()
            .p(px(10.))
            .gap_2()
            .border_t_1()
            .border_color(rgb(BORDER))
            .bg(crate::settings_style::frame_bg().opacity(0.15))
            .when(effort.is_some(), |d| {
                d.child(self.reasoning_slider_view(cx))
            })
            .children(
                model
                    .options
                    .iter()
                    .enumerate()
                    .filter(|(_, option)| hidden_option != Some(option.id.as_str()))
                    .map(|(i, option)| {
                        let id = option.id.clone();
                        let selected = self
                            .model_options
                            .get(&id)
                            .and_then(Value::as_str)
                            .unwrap_or(&option.default_choice);
                        if let Some((on, off)) =
                            zuse_protocol::presentation::model_option_switch_values(
                                &option.id,
                                option.choices.iter().map(|choice| choice.id.as_str()),
                            )
                        {
                            let checked = selected == on;
                            let value = if checked { off } else { on }.to_owned();
                            let current_label = option
                                .choices
                                .iter()
                                .find(|choice| choice.id == selected)
                                .map(|choice| choice.label.clone())
                                .unwrap_or_default();
                            return row()
                                .justify_between()
                                .gap_3()
                                .min_h(px(28.))
                                .child(label(option.label.clone()).text_size(px(11.)))
                                .child(
                                    row()
                                        .gap_2()
                                        .child(label(current_label).text_size(px(11.)))
                                        .child(
                                            crate::settings_style::toggle(
                                                SharedString::from(format!(
                                                    "model-option-switch-{i}"
                                                )),
                                                checked,
                                                &option.label,
                                            )
                                            .tab_index(0)
                                            .on_click(
                                                cx.listener({
                                                    let id = id.clone();
                                                    let value = value.clone();
                                                    move |this, _, _, cx| {
                                                        this.select_model_option(
                                                            id.clone(),
                                                            value.clone(),
                                                            cx,
                                                        );
                                                    }
                                                }),
                                            ),
                                        ),
                                )
                                .into_any_element();
                        }
                        column()
                            .gap_1()
                            .child(label(option.label.clone()).text_size(px(11.)))
                            .child(row().gap_1().flex_wrap().children(
                                option.choices.iter().enumerate().map(|(j, choice)| {
                                    let id = id.clone();
                                    let value = choice.id.clone();
                                    control(
                                        SharedString::from(format!("model-option-{i}-{j}")),
                                        SharedString::from(choice.label.clone()),
                                    )
                                    .h(px(26.))
                                    .text_size(px(10.))
                                    .when(selected == choice.id, |d| {
                                        d.bg(rgb(0x343d22)).text_color(rgb(ACCENT))
                                    })
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.select_model_option(id.clone(), value.clone(), cx);
                                        },
                                    ))
                                }),
                            ))
                            .into_any_element()
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod settings_workflow_tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn conversation_switches_providers_in_both_directions(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut received) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx, |view, _, cx| {
            view.connected = true;
            view.workspace_synced = true;
            view.model_confirmed = true;
            view.device_id = Some("local".into());
            view.providers = vec![json!({"id":"claude-code"}), json!({"id":"codex"})];
            let root = json!({"id":"thread","deviceId":"local","config":{"harness":"claude-code","model":"root-model"}});
            let fresh = json!({"id":"conversation","threadId":"thread","deviceId":"local","lastMessageAt":"2026-10-18T00:00:00Z","harnessSessionId":"previous-provider-session","config":{"harness":"claude-code","model":"root-model","sandbox":"read-only"}});
            view.chats = vec![root.clone(), fresh.clone()];
            view.selected = Some(fresh);
            view.model = Some("root-model".into());
            view.choose_model("codex".into(), Some("own-model".into()), cx);
            let writes = std::iter::from_fn(|| received.try_recv().ok())
                .filter(|request| request["method"] == methods::MUTATE).collect::<Vec<_>>();
            assert_eq!(writes.len(), 1);
            let write = &writes[0];
            assert_eq!(write["params"]["chatId"], "conversation");
            assert_eq!(write["params"]["config"]["harness"], "codex");
            assert_eq!(write["params"]["config"]["model"], "own-model");
            assert_eq!(write["params"]["config"]["sandbox"], "read-only");
            view.receive(json!({"id":write["id"],"ok":{"ok":true}}), cx);
            assert_eq!(view.chats[0], root);
            assert_eq!(view.selected.as_ref().unwrap()["config"]["harness"], "codex");
            view.choose_model("claude-code".into(), Some("opus-model".into()), cx);
            let back = std::iter::from_fn(|| received.try_recv().ok())
                .find(|request| request["method"] == methods::MUTATE).unwrap();
            assert_eq!(back["params"]["chatId"], "conversation");
            assert_eq!(back["params"]["config"]["harness"], "claude-code");
            assert_eq!(back["params"]["config"]["model"], "opus-model");
            assert_eq!(view.chats[0], root);
        }).unwrap();
    }

    #[gpui::test]
    fn workflow_title_picker_uses_connected_host_without_changing_chat_or_defaults(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut received) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx,|view,window,cx|{
            view.connected=true;view.device_id=Some("local".into());
            view.providers=vec![json!({"id":"codex","installed":true,"enabled":true})];
            view.selected=Some(json!({"id":"chat","deviceId":"remote","config":{"harness":"claude-code","model":"conversation-model"}}));
            view.model=Some("conversation-model".into());
            view.preferences.default_provider=Some("claude-code".into());
            view.model_host.target=Some("remote".into());
            view.model_host.rows=vec![json!({"id":"claude-code"})];
            view.refresh_title_settings();
            let read=std::iter::from_fn(||received.try_recv().ok()).find(|request|request["method"]==methods::GET_TITLE_SETTINGS).unwrap()["id"].as_u64().unwrap();
            view.receive(json!({"id":read,"ok":{"harness":null,"model":null}}),cx);
            view.open_settings_model_picker(PickerTarget::ThreadNaming,window,cx);
            assert_eq!(view.picker_providers()[0]["id"],"codex");
            view.choose_model("codex".into(),Some("title-model".into()),cx);
            let writes=std::iter::from_fn(||received.try_recv().ok()).filter(|request|request["method"]==methods::SET_TITLE_SETTINGS).collect::<Vec<_>>();
            assert_eq!(writes.len(),1);
            assert_eq!(writes[0]["params"],json!({"harness":"codex","model":"title-model"}));
            let write=writes[0]["id"].as_u64().unwrap();
            view.receive(json!({"id":write,"ok":{"harness":"codex","model":"title-model"}}),cx);
            assert!(!view.model_picker);
            assert_eq!(view.title_settings.provider().as_deref(),Some("codex"));
            assert_eq!(view.model.as_deref(),Some("conversation-model"));
            assert_eq!(view.preferences.default_provider.as_deref(),Some("claude-code"));
            assert_eq!(view.selected.as_ref().unwrap()["config"]["model"],"conversation-model");
            view.save_title_model(None,None,cx);
            let reset=std::iter::from_fn(||received.try_recv().ok()).find(|request|request["method"]==methods::SET_TITLE_SETTINGS).unwrap();
            assert_eq!(reset["params"],json!({"harness":null,"model":null}));
        }).unwrap();
    }
}
