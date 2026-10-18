//! Existing conversations persist choices; new conversations remember exact model defaults.
use super::*;
use zuse_protocol::presentation::RememberedModelConfig;

pub(super) struct PendingModelSave {
    pub id: u64,
    chat: String,
    previous: Value,
    config: Value,
}
impl WorkspaceView {
    pub(super) fn model_change_pending_for_selection(&self) -> bool {
        (self.selected.is_some() && !self.model_confirmed)
            || self.model_dirty
            || self.model_save.is_some()
            || self
                .selected
                .as_ref()
                .and_then(|chat| chat["id"].as_str())
                .is_some_and(|id| self.model_unconfirmed.contains(id))
    }
    pub(super) fn model_navigation_blocked(&mut self, cx: &mut Context<Self>) -> bool {
        if self.model_save.is_some() || self.model_dirty {
            self.error =
                Some("Wait for the model choices to finish saving before switching chats.".into());
            cx.notify();
            true
        } else {
            false
        }
    }
    pub(super) fn can_change_model(&mut self, cx: &mut Context<Self>) -> bool {
        if self.creating.is_some() {
            self.error =
                Some("Wait for the conversation to open before changing model choices.".into());
            cx.notify();
            return false;
        }
        if self.selected.is_some()
            && (!self.connected
                || !self.workspace_synced
                || !self.model_confirmed
                || self.access_change_pending_for_selection()
                || self
                    .selected
                    .as_ref()
                    .and_then(|chat| chat["id"].as_str())
                    .is_some_and(|id| self.model_unconfirmed.contains(id)))
        {
            self.error = Some(
                "Wait for the saved chat configuration to refresh before changing model choices."
                    .into(),
            );
            cx.notify();
            false
        } else {
            true
        }
    }
    pub(super) fn model_provider_allowed(&self, provider: &str) -> bool {
        self.model_picker_default
            || self
                .selected
                .as_ref()
                .and_then(|chat| chat["config"]["harness"].as_str())
                .is_none_or(|saved| saved == provider)
    }
    fn current_model_config(&self) -> Option<RememberedModelConfig> {
        Some(RememberedModelConfig {
            provider: self.composer_provider()?["id"].as_str()?.into(),
            model: self.model.clone(),
            reasoning: self.reasoning,
            model_options: self.model_options.clone(),
        })
    }
    pub(super) fn restore_model_choices(&mut self) {
        let Some(provider) = self
            .composer_providers()
            .get(self.provider_index)
            .and_then(|provider| provider["id"].as_str())
        else {
            return;
        };
        let mut config = self
            .preferences
            .model_config(provider, self.model.as_deref())
            .cloned();
        if let Some(config) = &mut config {
            if let Some(model) = self.selected_model() {
                config.normalize_for(model);
            }
        }
        self.reasoning = config.as_ref().and_then(|config| config.reasoning);
        self.model_options = config
            .map(|config| config.model_options)
            .unwrap_or_default();
    }
    pub(super) fn sync_model_from_chat(&mut self, chat: &Value) {
        if let Some(index) = self
            .composer_providers()
            .iter()
            .position(|provider| provider["id"] == chat["config"]["harness"])
        {
            self.provider_index = index;
        }
        self.model = chat["config"]["model"].as_str().map(str::to_owned);
        self.reasoning = serde_json::from_value(chat["config"]["reasoning"].clone()).ok();
        self.model_options = chat["config"]["modelOptions"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        self.prefetch_model_catalogs();
    }
    pub(super) fn reconcile_model_config(&mut self, chat: &Value) {
        if !self.model_confirmed && self.model_save.is_none() && !self.model_dirty {
            self.sync_model_from_chat(chat);
        }
    }
    pub(super) fn refresh_model_config_watch(&mut self) {
        self.model_confirmed = false;
        if let Some(watch) = self.model_watch.take() {
            self.cancel(watch);
        }
        if self.connected
            && let Some(chat) = self.selected.clone()
        {
            self.model_watch = Some(self.call(
                methods::WATCH_CHAT_CONFIG,
                zuse_protocol::presentation::chat_request(&chat, json!({})),
            ));
        }
    }
    fn receive_model_watch(&mut self, id: u64, frame: &Value, cx: &mut Context<Self>) {
        if let Some(error) = frame["err"].as_str() {
            self.model_confirmed = false;
            self.model_watch = None;
            self.pending.remove(&id);
            self.error = Some(format!(
                "Could not read this chat’s saved configuration: {error}. Open the model picker to refresh."
            ));
        } else if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
            let matches = self.selected.as_ref().is_some_and(|chat| {
                chat["id"] == value["chatId"] && chat["deviceId"] == value["deviceId"]
            });
            let config = value.get("config");
            let valid = config.is_some_and(|config| {
                config.is_null()
                    || serde_json::from_value::<zuse_protocol::ChatConfig>(config.clone()).is_ok()
            });
            if matches && valid {
                let chat = self.selected.as_mut().unwrap();
                chat["config"] = config.unwrap().clone();
                self.model_confirmed = true;
                if let Some(chat) = chat["id"].as_str() {
                    self.model_unconfirmed.remove(chat);
                    if self.access_unconfirmed.as_deref() == Some(chat) {
                        self.access_unconfirmed = None;
                    }
                }
                self.sandbox =
                    serde_json::from_value(chat["config"]["sandbox"].clone()).unwrap_or_default();
                if self.model_save.is_none() && !self.model_dirty {
                    let chat = self.selected.clone().unwrap();
                    self.sync_model_from_chat(&chat);
                    self.configure_reasoning_slider(cx);
                }
            } else {
                self.model_confirmed = false;
                self.error=Some("The chat configuration or host changed. Reopen the conversation to refresh it.".into());
            }
        }
        if frame["done"] == true {
            self.model_confirmed = false;
            self.model_watch = None;
            self.pending.remove(&id);
        }
        cx.notify();
    }
    pub(super) fn select_model_option(
        &mut self,
        id: String,
        value: String,
        cx: &mut Context<Self>,
    ) {
        if !self.can_change_model(cx) {
            return;
        }
        let Some(option) = self
            .selected_model()
            .and_then(|model| model.options.iter().find(|option| option.id == id))
        else {
            return;
        };
        if !option.choices.iter().any(|choice| choice.id == value) {
            return;
        }
        if value == option.default_choice {
            self.model_options.remove(&id);
        } else {
            self.model_options.insert(id, json!(value));
        }
        self.model_choices_changed(cx);
    }
    pub(super) fn model_choices_changed(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_none() {
            if let Some(config) = self.current_model_config() {
                self.preferences.remember_model_config(config);
                self.save_preferences();
            }
        } else {
            self.model_dirty = true;
            self.flush_model_config(cx);
        }
        cx.notify();
    }
    fn flush_model_config(&mut self, cx: &mut Context<Self>) {
        if !self.model_dirty || self.model_save.is_some() || !self.connected {
            return;
        }
        let Some(chat) = self.selected.clone() else {
            return;
        };
        let Some(id) = chat["id"].as_str().map(str::to_owned) else {
            return;
        };
        let Some(desired) = self.current_model_config() else {
            return;
        };
        let previous = chat["config"].clone();
        let config = desired.apply_to(&previous, self.sandbox);
        self.model_dirty = false;
        if config == previous {
            return;
        }
        let request = self.call(
            methods::MUTATE,
            json!({"op":"setChatConfig","chatId":id,"targetDeviceId":chat["deviceId"],"expectedConfig":{"deviceId":chat["deviceId"],"config":previous},"config":config}),
        );
        self.model_save = Some(PendingModelSave {
            id: request,
            chat: id,
            previous,
            config,
        });
        cx.notify();
    }
    pub(super) fn receive_model_config(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.model_watch == Some(id) {
            self.receive_model_watch(id, frame, cx);
            return true;
        }
        if !self.model_save.as_ref().is_some_and(|save| save.id == id) {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let save = self.model_save.take().unwrap();
        self.pending.remove(&id);
        if frame["ok"]["ok"] == true && frame.get("err").is_none() {
            if let Some(chat) = self
                .selected
                .as_mut()
                .filter(|chat| chat["id"] == save.chat)
            {
                if chat["config"] == save.previous {
                    chat["config"] = save.config;
                }
            }
            if self.model_dirty {
                self.flush_model_config(cx);
            } else if let Some(chat) = self.selected.clone() {
                self.sync_model_from_chat(&chat);
            }
        } else {
            self.model_dirty = false;
            self.model_unconfirmed.insert(save.chat);
            self.error=Some("The model change was not confirmed. Refreshing the saved configuration; no change will be replayed.".into());
            self.refresh_access_snapshot();
        }
        self.configure_reasoning_slider(cx);
        cx.notify();
        true
    }
    pub(super) fn interrupt_model_config(&mut self, cx: &mut Context<Self>) {
        self.model_confirmed = false;
        if let Some(watch) = self.model_watch.take() {
            self.cancel(watch);
        }
        let interrupted = self.model_save.is_some();
        if let Some(save) = self.model_save.take() {
            self.model_unconfirmed.insert(save.chat);
            self.cancel(save.id);
        }
        if self.model_dirty || interrupted {
            self.connection_notice=Some("Model choices may not have finished saving. Review the refreshed chat configuration before choosing again.".into());
        }
        self.model_dirty = false;
        cx.notify();
    }
}
