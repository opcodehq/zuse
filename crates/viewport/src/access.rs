//! A saved access change is acknowledged before it can affect another turn.
use super::*;
use zuse_protocol::SandboxLevel;

pub(super) struct PendingAccessSave {
    pub id: u64,
    pub chat: String,
    pub config: Value,
    pub previous: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    #[gpui::test]
    fn access_choice_becomes_the_default_for_the_next_project(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut saved) = futures::channel::mpsc::unbounded();
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
            view.choose_access(SandboxLevel::DangerFullAccess, cx);
            assert_eq!(view.preferences.default_permission_mode.sandbox(), SandboxLevel::DangerFullAccess);
            view.project = Some(json!({"path":"/another-project"}));
            assert!(view.new_chat(cx));
            assert_eq!(view.sandbox, SandboxLevel::DangerFullAccess);

            // Selecting an existing chat still uses its own saved access.
            view.selected = Some(json!({"id":"existing","deviceId":"local","config":{"harness":"codex","sandbox":"auto"}}));
            view.workspace_synced = true;
            view.model_confirmed = true;
            view.choose_access(SandboxLevel::WorkspaceWrite, cx);
            assert!(view.access_save.is_some());
            assert_eq!(view.preferences.default_permission_mode.sandbox(), SandboxLevel::WorkspaceWrite);
        }).unwrap();
        let mut defaults = Vec::new();
        while let Ok(frame) = saved.try_recv() {
            if frame["ui"] == "savePreferences" {
                defaults.push(frame["value"]["defaultPermissionMode"].clone());
            }
        }
        assert_eq!(
            defaults,
            vec![json!("full-access"), json!("approval-required")]
        );
    }
}
impl WorkspaceView {
    pub(super) fn toggle_access(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.access_save.is_some() {
            return;
        }
        self.context_menu = false;
        self.access_picker = !self.access_picker;
        self.add_menu = false;
        self.model_picker = false;
        self.agent_picker = false;
        if self.access_picker {
            self.access_active = zuse_protocol::presentation::ACCESS_LEVELS
                .iter()
                .position(|level| *level == self.sandbox)
                .unwrap_or(0);
            window.focus(&self.access_focus, cx);
        } else {
            window.focus(&self.input.focus_handle(cx), cx);
        }
        cx.notify();
    }
    pub(super) fn on_access_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use zuse_protocol::presentation::ACCESS_LEVELS;
        if !self.access_picker {
            if matches!(event.keystroke.key.as_str(), "enter" | "space" | "down") && !event.is_held
            {
                self.toggle_access(window, cx);
                cx.stop_propagation();
            }
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.access_picker = false,
            "up" => {
                self.access_active =
                    (self.access_active + ACCESS_LEVELS.len() - 1) % ACCESS_LEVELS.len()
            }
            "down" => self.access_active = (self.access_active + 1) % ACCESS_LEVELS.len(),
            "home" => self.access_active = 0,
            "end" => self.access_active = ACCESS_LEVELS.len() - 1,
            "enter" | "space" if !event.is_held => {
                self.choose_access(ACCESS_LEVELS[self.access_active], cx)
            }
            _ => return,
        }
        if !self.access_picker {
            window.focus(&self.input.focus_handle(cx), cx);
        }
        cx.stop_propagation();
        cx.notify();
    }
    /// Stage an access change and save the selection as the global new-chat default.
    pub(super) fn choose_access(&mut self, level: SandboxLevel, cx: &mut Context<Self>) {
        self.access_picker = false;
        if self.model_change_pending_for_selection() {
            self.error = Some(
                "Wait for the model choices to finish saving before changing permissions.".into(),
            );
            cx.notify();
            return;
        }
        if self.access_save.is_some() {
            return;
        }
        if self.creating.is_some()
            || self.cloud_model_change_pending()
            || (self.selected.is_some() && !self.workspace_synced)
        {
            self.error =
                Some("Wait for the conversation to refresh before changing permissions.".into());
            cx.notify();
            return;
        }
        let Some(chat) = self.selected.as_ref() else {
            self.sandbox = level;
            self.remember_access(level);
            cx.notify();
            return;
        };
        let Some(chat_id) = chat["id"].as_str().map(str::to_owned) else {
            return;
        };
        let target = chat["deviceId"].clone();
        let previous = chat["config"].clone();
        let mut config = previous.clone();
        if !config.is_object() {
            let Some(provider) = self.composer_provider() else {
                return;
            };
            config = json!({"harness":provider["id"],"model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options});
        }
        config["sandbox"] = json!(level);
        if !self.require_connection(cx) {
            return;
        }
        if self.access_unconfirmed.as_deref() == Some(&chat_id) {
            self.error = Some("Waiting for the saved permission mode to reconnect.".into());
            cx.notify();
            return;
        }
        let id = self.call(
            methods::MUTATE,
            json!({"op":"setChatConfig","chatId":chat_id,"targetDeviceId":target,"expectedConfig":{"deviceId":target,"config":previous},"config":config}),
        );
        self.access_save = Some(PendingAccessSave {
            id,
            chat: chat_id,
            config,
            previous,
        });
        self.remember_access(level);
        cx.notify();
    }
    /// Persist the last access selection through the host preferences channel.
    fn remember_access(&mut self, level: SandboxLevel) {
        let mode = zuse_protocol::presentation::PermissionMode::from_sandbox(level);
        self.preferences.default_permission_mode = mode;
        self.save_preferences();
    }
    pub(super) fn receive_access(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.access_save.as_ref().is_some_and(|save| save.id == id) {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let save = self.access_save.take().unwrap();
        self.pending.remove(&id);
        if let Some(error) = frame["err"].as_str() {
            self.error = Some(format!("Permission change was not confirmed: {error}"));
            self.access_unconfirmed = Some(save.chat);
            self.refresh_access_snapshot();
        } else if frame["ok"]["ok"] == true {
            if let Some(chat) = self
                .selected
                .as_mut()
                .filter(|chat| chat["id"] == save.chat)
            {
                if chat["config"] == save.previous {
                    chat["config"] = save.config;
                }
                self.sandbox =
                    serde_json::from_value(chat["config"]["sandbox"].clone()).unwrap_or_default();
            }
        } else {
            self.error = Some("The host closed the permission request without confirmation. Rechecking saved permissions.".into());
            self.access_unconfirmed = Some(save.chat);
            self.refresh_access_snapshot();
        }
        cx.notify();
        true
    }
    pub(super) fn refresh_access_snapshot(&mut self) {
        let previous: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, method)| (method == methods::WATCH_CHATS).then_some(*id))
            .collect();
        for id in previous {
            self.cancel(id);
        }
        self.call(methods::WATCH_CHATS, json!({}));
        self.refresh_model_config_watch();
    }
    pub(super) fn access_change_pending_for_selection(&self) -> bool {
        self.selected
            .as_ref()
            .and_then(|chat| chat["id"].as_str())
            .is_some_and(|chat| {
                self.access_save
                    .as_ref()
                    .is_some_and(|save| save.chat == chat)
                    || self.access_unconfirmed.as_deref() == Some(chat)
            })
    }
}
