//! Model discovery follows the execution host; settings keep the connected host.
use super::*;
use zuse_protocol::presentation::PickerCatalog;
#[derive(Default)]
pub(super) struct ModelHost {
    pub target: Option<String>,
    pub rows: Vec<Value>,
    pub catalog: PickerCatalog,
    pub request: Option<u64>,
    pub confirmed: bool,
    pub error: Option<String>,
}
impl WorkspaceView {
    pub(super) fn chat_host_label(&self, chat: &Value) -> &'static str {
        if chat["deviceId"]
            .as_str()
            .is_some_and(|id| Some(id) != self.device_id.as_deref())
        {
            "Other computer"
        } else if self.cloud.guest {
            "Cloud"
        } else if self.account_remote {
            "Account computer"
        } else if self.paired {
            "Paired engine"
        } else {
            "This device"
        }
    }
    pub(super) fn composer_providers(&self) -> &[Value] {
        if self.model_host.target.is_some() {
            &self.model_host.rows
        } else {
            &self.providers
        }
    }
    pub(super) fn composer_provider(&self) -> Option<&Value> {
        if self.model_host.target.is_some() && !self.model_host.confirmed {
            return None;
        }
        let rows = self.composer_providers();
        if let Some(harness) = self
            .selected
            .as_ref()
            .and_then(|chat| chat["config"]["harness"].as_str())
        {
            rows.iter().find(|row| row["id"] == harness)
        } else {
            rows.get(self.provider_index)
        }
    }
    pub(super) fn picker_providers(&self) -> &[Value] {
        if self.picker_in_settings() {
            &self.providers
        } else {
            self.composer_providers()
        }
    }
    pub(super) fn composer_catalog(&self) -> &PickerCatalog {
        if self.model_host.target.is_some() {
            &self.model_host.catalog
        } else {
            &self.picker_catalog
        }
    }
    pub(super) fn active_picker_catalog(&self) -> &PickerCatalog {
        if self.picker_in_settings() {
            &self.picker_catalog
        } else {
            self.composer_catalog()
        }
    }
    fn sync_catalog_target(&mut self) {
        let target = self
            .selected
            .as_ref()
            .and_then(|chat| chat["deviceId"].as_str())
            .filter(|id| !id.is_empty() && Some(*id) != self.device_id.as_deref())
            .map(str::to_owned);
        if target == self.model_host.target {
            return;
        }
        if let Some(id) = self.model_host.request.take() {
            self.cancel(id);
        }
        for id in self.model_host.catalog.retire() {
            self.cancel(id);
        }
        self.model_host = ModelHost {
            target,
            ..Default::default()
        };
        self.picker_scope = None;
        self.picker_active = None;
        self.picker_anchors.clear();
    }
    pub(super) fn prefetch_model_catalogs(&mut self) {
        self.sync_catalog_target();
        self.load_catalogs_for(true, false);
        if self.model_host.target.is_some() {
            self.load_catalogs_for(false, false);
        }
    }
    pub(super) fn load_catalogs_for(&mut self, local: bool, force: bool) {
        if !self.connected {
            return;
        }
        if !local
            && self.model_host.target.is_some()
            && self.model_host.request.is_none()
            && (force || (!self.model_host.confirmed && self.model_host.error.is_none()))
        {
            self.model_host.error = None;
            self.model_host.request = Some(self.call(
                methods::LIST_HARNESSES,
                json!({"targetDeviceId":self.model_host.target}),
            ));
        }
        let rows = if local {
            &self.providers
        } else {
            &self.model_host.rows
        };
        let mut providers = rows
            .iter()
            .filter(|row| row["installed"] != false && row["enabled"] != false)
            .filter_map(|row| row["id"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        if local == self.model_host.target.is_none()
            && let Some(harness) = self
                .selected
                .as_ref()
                .and_then(|chat| chat["config"]["harness"].as_str())
            && !providers.iter().any(|id| id == harness)
        {
            providers.push(harness.into());
        }
        for provider in providers {
            let catalog = if local {
                &self.picker_catalog
            } else {
                &self.model_host.catalog
            };
            if catalog.providers.get(&provider).is_some_and(|slot| {
                slot.request.is_some()
                    || (!force && !slot.stale && (slot.loaded || slot.error.is_some()))
            }) {
                continue;
            }
            let target = if local {
                self.device_id.clone()
            } else {
                self.model_host.target.clone()
            };
            let id = self.call(
                methods::LIST_MODELS,
                json!({"harness":provider,"force":force,"targetDeviceId":target}),
            );
            if local {
                self.picker_catalog.begin(&provider, id);
            } else {
                self.model_host.catalog.begin(&provider, id);
            }
        }
    }
    pub(super) fn load_picker_catalogs(&mut self, force: bool) {
        self.sync_catalog_target();
        self.load_catalogs_for(
            self.picker_in_settings() || self.model_host.target.is_none(),
            force,
        );
        self.sync_picker_anchors();
    }
    pub(super) fn sync_picker_anchors(&mut self) {
        let keys = self
            .active_picker_catalog()
            .providers
            .iter()
            .flat_map(|(provider, catalog)| {
                catalog
                    .models
                    .iter()
                    .map(move |model| format!("{provider}:{}", model.id))
            })
            .collect::<Vec<_>>();
        for key in keys {
            self.picker_anchors
                .entry(key)
                .or_insert_with(|| ScrollAnchor::for_handle(self.picker_scroll.clone()));
        }
    }
    pub(super) fn receive_host_catalog(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.model_host.request != Some(id) {
            return false;
        }
        let result = if let Some(error) = frame["err"].as_str() {
            Err(error.to_owned())
        } else if let Some(value) = frame.get("ok") {
            value
                .as_array()
                .filter(|rows| rows.iter().all(|row| row["id"].is_string()))
                .cloned()
                .ok_or_else(|| "The execution host returned an invalid agent catalog.".into())
        } else if frame["done"] == true {
            Err("The execution host closed its agent catalog without a result.".into())
        } else {
            return true;
        };
        self.model_host.request = None;
        self.pending.remove(&id);
        match result {
            Ok(rows) => {
                let preferred = self
                    .selected
                    .as_ref()
                    .and_then(|chat| chat["config"]["harness"].as_str())
                    .or_else(|| {
                        self.model_host
                            .rows
                            .get(self.provider_index)
                            .and_then(|row| row["id"].as_str())
                    })
                    .map(str::to_owned);
                self.model_host.rows = rows.into_iter().filter(|row| row["id"] != "mock").collect();
                self.provider_index = preferred
                    .and_then(|id| self.model_host.rows.iter().position(|row| row["id"] == id))
                    .or_else(|| {
                        self.model_host
                            .rows
                            .iter()
                            .position(|row| row["installed"] != false && row["enabled"] != false)
                    })
                    .unwrap_or(0);
                self.model_host.confirmed = true;
                self.model_host.error = self
                    .selected
                    .as_ref()
                    .and_then(|chat| chat["config"]["harness"].as_str())
                    .filter(|harness| !self.model_host.rows.iter().any(|row| row["id"] == *harness))
                    .map(|_| {
                        "The agent configured for this chat is not available on its execution host."
                            .into()
                    });
                if let Some(chat) = self.selected.clone() {
                    self.sync_model_from_chat(&chat);
                }
                self.load_catalogs_for(false, false);
            }
            Err(error) => {
                self.model_host.confirmed = false;
                self.model_host.error = Some(error);
            }
        }
        self.sync_picker_anchors();
        self.configure_reasoning_slider(cx);
        cx.notify();
        true
    }
    pub(super) fn interrupt_model_catalogs(&mut self) {
        self.picker_catalog.interrupted();
        self.model_host.catalog.interrupted();
        self.model_host.request = None;
        self.model_host.confirmed = false;
        self.model_host.error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    struct Fixture {
        window: WindowHandle<WorkspaceView>,
        requests: UnboundedReceiver<Value>,
        _events: UnboundedSender<Value>,
    }
    fn fixture(cx: &mut TestAppContext) -> Fixture {
        cx.update(initialize);
        let (requests, rx) = futures::channel::mpsc::unbounded();
        let (events, frames) = futures::channel::mpsc::unbounded();
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
        window
            .update(cx, |view, _, _| {
                view.connected = true;
                view.connection_epoch = 1;
                view.device_id = Some("local".into());
                view.workspace_synced = true;
                view.preferences.default_provider = Some("claude-code".into());
            })
            .unwrap();
        Fixture {
            window,
            requests: rx,
            _events: events,
        }
    }
    fn rows(id: &str) -> Value {
        json!([{"id":id,"name":id,"installed":true,"enabled":true}])
    }
    fn models(label: &str) -> Value {
        json!([{"id":"default","label":label,"description":null,"reasoningLevels":[],"options":[]}])
    }
    fn local(view: &mut WorkspaceView, cx: &mut Context<WorkspaceView>) {
        let id = view.call(methods::LIST_HARNESSES, json!({}));
        view.receive(json!({"id":id,"ok":rows("claude-code"),"done":true}), cx);
    }
    fn chat(host: &str) -> Value {
        json!({"id":"chat","deviceId":host,"config":{"harness":"codex","model":"default","modelOptions":{},"sandbox":"workspace-write"}})
    }
    #[gpui::test]
    fn local_catalog_prefetch_resolves_the_composer_without_opening_a_picker(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx);
        f.window
            .update(cx, |view, _, cx| {
                local(view, cx);
                assert!(!view.model_picker);
                let id = view.picker_catalog.providers["claude-code"]
                    .request
                    .unwrap();
                view.receive(json!({"id":id,"ok":models("Local model"),"done":true}), cx);
                assert_eq!(view.picker_label(), "Local model");
                view.prefetch_model_catalogs();
                assert!(
                    view.picker_catalog.providers["claude-code"]
                        .request
                        .is_none()
                );
            })
            .unwrap();
        let mut requests = vec![];
        while let Ok(request) = f.requests.try_recv() {
            if request["method"] == methods::LIST_MODELS {
                requests.push(request);
            }
        }
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["params"]["targetDeviceId"], "local");
    }
    #[gpui::test]
    fn host_catalog_handoff_retires_old_reads_and_keeps_settings_local(cx: &mut TestAppContext) {
        let mut f = fixture(cx);
        f.window
            .update(cx, |view, _, cx| {
                local(view, cx);
                let local = view.picker_catalog.providers["claude-code"]
                    .request
                    .unwrap();
                let a = chat("peer-a");
                view.selected = Some(a.clone());
                view.sync_model_from_chat(&a);
                assert!(view.composer_provider().is_none());
                let agents = view.model_host.request.unwrap();
                let models_a = view.model_host.catalog.providers["codex"].request.unwrap();
                view.receive(
                    json!({"id":models_a,"ok":models("Peer A model"),"done":true}),
                    cx,
                );
                view.receive(json!({"id":agents,"ok":rows("codex"),"done":true}), cx);
                assert_eq!(view.picker_label(), "Peer A model");
                assert_eq!(view.composer_provider().unwrap()["id"], "codex");
                assert_eq!(view.providers[0]["id"], "claude-code");
                view.load_catalogs_for(false, true);
                let old_agents = view.model_host.request.unwrap();
                let old_models = view.model_host.catalog.providers["codex"].request.unwrap();
                let b = chat("peer-b");
                view.selected = Some(b.clone());
                view.sync_model_from_chat(&b);
                assert!(view.composer_provider().is_none());
                assert!(view.model_host.catalog.providers["codex"].models.is_empty());
                view.receive(
                    json!({"id":old_models,"ok":models("Stale A model"),"done":true}),
                    cx,
                );
                view.receive(json!({"id":old_agents,"ok":rows("gemini"),"done":true}), cx);
                assert!(view.composer_provider().is_none());
                let agents = view.model_host.request.unwrap();
                let models_b = view.model_host.catalog.providers["codex"].request.unwrap();
                view.receive(json!({"id":agents,"ok":rows("codex"),"done":true}), cx);
                view.receive(
                    json!({"id":models_b,"ok":models("Peer B model"),"done":true}),
                    cx,
                );
                view.receive(
                    json!({"id":local,"ok":models("Local model"),"done":true}),
                    cx,
                );
                assert_eq!(view.picker_label(), "Peer B model");
                view.picker_target = model_picker::PickerTarget::Defaults;
                assert_eq!(view.picker_providers()[0]["id"], "claude-code");
                assert_eq!(
                    view.active_picker_catalog().providers["claude-code"].models[0].label,
                    "Local model"
                );
                assert_eq!(view.selected_model().unwrap().label, "Peer B model");
                view.picker_target = model_picker::PickerTarget::Composer;
                assert!(view.new_chat(cx));
                assert!(view.model_host.target.is_none());
                assert_eq!(view.picker_label(), "Local model");
            })
            .unwrap();
        let mut remote = vec![];
        while let Ok(request) = f.requests.try_recv() {
            if request["method"] == methods::LIST_MODELS && request["params"]["harness"] == "codex"
            {
                remote.push(request["params"]["targetDeviceId"].clone());
            }
        }
        assert_eq!(
            remote,
            vec![json!("peer-a"), json!("peer-a"), json!("peer-b")]
        );
    }
    #[gpui::test]
    fn catalog_reconnect_revalidates_cached_and_disabled_committed_models_once(
        cx: &mut TestAppContext,
    ) {
        let f = fixture(cx);
        f.window
            .update(cx, |view, _, cx| {
                local(view,cx);
                let c = chat("local");
                view.selected = Some(c.clone());
                view.sync_model_from_chat(&c);
                let agents=view.call(methods::LIST_HARNESSES,json!({}));
                view.receive(json!({"id":agents,"ok":[{"id":"claude-code","installed":true,"enabled":true},{"id":"codex","installed":true,"enabled":false}],"done":true}),cx);
                assert_eq!(view.composer_providers()[view.provider_index]["id"],"codex");
                for provider in ["claude-code", "codex"] {
                    let id = view.picker_catalog.providers[provider].request.unwrap();
                    view.receive(json!({"id":id,"ok":models(provider),"done":true}), cx);
                }
                view.interrupt_model_catalogs();
                view.prefetch_model_catalogs();
                let id = view.picker_catalog.providers["codex"].request.unwrap();
                assert!(view.picker_catalog.providers["codex"].stale);
                assert_eq!(view.picker_label(), "codex");
                view.prefetch_model_catalogs();
                assert_eq!(view.picker_catalog.providers["codex"].request, Some(id));
                view.receive(json!({"id":id,"err":"Probe failed","done":true}), cx);
                assert_eq!(view.picker_label(), "codex");
                view.prefetch_model_catalogs();
                assert!(view.picker_catalog.providers["codex"].request.is_none());
                view.load_picker_catalogs(true);
                assert!(view.picker_catalog.providers["codex"].request.is_some());
            })
            .unwrap();
    }
    #[gpui::test]
    fn absent_remote_agent_never_falls_back_to_a_local_provider_or_sends_a_draft(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx);
        f.window
            .update(cx, |view, _, cx| {
                local(view, cx);
                let c = chat("peer");
                view.chats = vec![c.clone()];
                view.selected = Some(c.clone());
                view.sync_model_from_chat(&c);
                let agents = view.model_host.request.unwrap();
                view.receive(json!({"id":agents,"ok":rows("gemini"),"done":true}), cx);
                assert!(view.composer_provider().is_none());
                view.begin_transcript(cx);
                let read = view.transcript_watch.unwrap();
                view.receive(json!({"id":read,"item":{"reset":[]}}), cx);
                view.model_confirmed = true;
                view.input.update(cx, |input, cx| {
                    input.set_value("Keep this draft".into(), cx)
                });
            })
            .unwrap();
        cx.run_until_parked();
        f.window
            .update(cx, |view, window, cx| {
                view.submit(&Submit, window, cx);
                assert_eq!(view.input.read(cx).value(cx), "Keep this draft");
                assert!(view.error.as_deref().unwrap().contains("execution host"));
            })
            .unwrap();
        while let Ok(request) = f.requests.try_recv() {
            assert!(!matches!(
                request["method"].as_str(),
                Some(methods::QUEUE_COMMAND | methods::QUEUE_MESSAGE | methods::MUTATE)
            ));
        }
    }
}
