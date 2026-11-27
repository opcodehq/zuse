//! Cloud launch choices come from the prepared runtime's catalogue, not the viewer's CLIs.
use super::*;
use serde_json::Value;
use zuse_protocol::cloud_catalog::CloudAgentCatalog;

pub(super) fn preparation_catalog_error(error: &zuse_rpc::RpcError) -> bool {
    matches!(
        error,
        zuse_rpc::RpcError::Closed | zuse_rpc::RpcError::Transport(_)
    ) || matches!(error, zuse_rpc::RpcError::Failed(message)
            if message == "The workspace is busy. Wait for its current operation to finish."
                // A conflict the engine names by the server's code.
                || message.starts_with("Cloud couldn't do this right now ("))
}

/// A managed-base Codex catalogue for tests: a six-level ladder and a
/// two-tier service option that reads as fast mode.
#[cfg(test)]
pub(crate) fn configurable_cloud_context_for_test() -> Value {
    use serde_json::json;
    let model = json!({"id":"gpt-sol","label":"GPT Sol","reasoningLevels":["low","medium","high","xhigh","max","ultra"],"options":[{"id":"serviceTier","label":"Service Tier","choices":[{"id":"default","label":"Standard"},{"id":"fast","label":"Fast"}],"defaultChoice":"default"}]});
    let catalog = json!({"providers":[{"id":"codex","name":"Codex","defaultModel":"gpt-sol","models":[model]}]});
    json!({"accountId":"fixture-user","ownerId":"fixture-user","organizationId":null,"commandAdmissionVersion":2,"computerCapabilities":{"providers":[{"id":"boxd","configured":true,"managedBaseReady":true,"commandAdmissionVersion":2,"preservesDisk":true,"preservesMemory":true,"supportsSnapshots":true,"supportsDocker":false,"sizes":[{"id":"default","label":"Standard","vcpus":4,"memoryMib":16384,"diskGib":32}],"unavailableReason":null}]},"providerAgentCatalogs":{"boxd":catalog},"agentConnections":[{"agent":"codex","state":"connected","connectionId":"codex-selected"}]})
}

#[cfg(test)]
impl Pickers {
    /// Set a model's reasoning stop and fast mode through its detail card.
    pub(crate) fn set_card_settings_for_test(
        &mut self,
        harness: HarnessId,
        model: &str,
        effort: usize,
        fast: bool,
        cx: &mut Context<Self>,
    ) {
        self.set_detail_effort(harness, model, effort, cx);
        self.set_detail_fast(harness, model, fast, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[gpui::test]
    fn saved_task_model_survives_busy_catalog_and_scoped_catalog_recovery(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        let (send, _requests) = tokio::sync::mpsc::channel(1);
        let (_responses, receive) = tokio::sync::mpsc::channel(1);
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send, receive));
        let state = cx.new(|_| AppState::new());
        let pickers = cx.new(|cx| Pickers::new(state, cx));
        pickers.update(cx,|pickers,cx| {
            let choice=ChatConfig {harness:HarnessId::Codex,model:Some("saved-model".into()),reasoning:Some(ReasoningLevel::High),model_options:Default::default(),sandbox:SandboxLevel::WorkspaceWrite,plan:false};
            let target=crate::state::CloudTarget {controller,subject:"owner".into(),organization:Some("org".into()),workspace:"task".into(),initial_session:"session".into()};
            pickers.bind_cloud_task(target,"boxd".into(),Some(choice.clone()),cx);
            assert!(pickers.cloud_model_notice(cx).is_none(),"one conversation progress row while catalogue starts");
            for error in [zuse_rpc::RpcError::Failed("The workspace is busy. Wait for its current operation to finish.".into()),zuse_rpc::RpcError::Closed] {
                pickers.receive_cloud_model_error(error,cx);
                assert!(pickers.cloud_model_notice(cx).is_none(),"ordinary preparation is not a retry banner");
                let resolved=pickers.resolved(cx);
                assert_eq!(resolved.harness,Some(choice.harness));assert_eq!(resolved.model,choice.model);assert_eq!(resolved.reasoning,choice.reasoning);assert_eq!(resolved.sandbox,choice.sandbox);
                assert!(pickers.cloud.model_error.is_some(),"details remain available in explicit model picker");
            }
            pickers.receive_cloud_models(json!({"providers":[{"id":"codex","name":"Codex","defaultModel":"different-default","models":[{"id":"saved-model","label":"Saved"},{"id":"different-default","label":"Default"}]}]}),cx);
            assert!(!pickers.cloud.model_waiting);assert!(pickers.cloud.model_error.is_none());assert!(pickers.cloud_model_notice(cx).is_none());
            assert_eq!(pickers.resolved(cx).model,choice.model,"catalogue default never replaces saved task choice");
            let binding=pickers.cloud.task_binding.as_ref().unwrap();assert_eq!(binding.target.subject,"owner");assert_eq!(binding.target.organization.as_deref(),Some("org"));assert_eq!(binding.target.workspace,"task");
            pickers.cloud.catalog=None;pickers.cloud.models.clear();
            pickers.receive_cloud_model_error(zuse_rpc::RpcError::Capability("cloud_access_denied".into()),cx);
            let(notice,retry)=pickers.cloud_model_notice(cx).expect("real authority failure remains actionable");
            assert!(notice.contains("cloud_access_denied"));assert!(retry);assert!(!pickers.cloud.model_waiting);
            assert_eq!(pickers.resolved(cx).model,choice.model);
        });
    }

    #[gpui::test]
    fn cloud_catalog_card_offers_the_models_reasoning_and_fast_mode(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        });
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
                state
            });
            Pickers::new(state, cx)
        });
        handle
            .update(cx, |picker, window, cx| {
                let project = json!({"projectId":"repository","accountId":"fixture-user","providerId":"boxd","defaultBranch":"main","supportsModelConfiguration":true});
                picker.seed_cloud_creation_for_test(
                    configurable_cloud_context_for_test(),
                    project,
                    cx,
                );
                assert!(picker.cloud_models_active(cx));
                picker.open_model_menu(window, cx);
                let detail = picker.model_detail(cx).expect("card for the cloud model");
                assert_eq!(detail.row.model.id, "gpt-sol");
                let effort = detail.effort.expect("reasoning slider");
                assert_eq!(effort.labels.len(), 6, "{:?}", effort.labels);
                assert_eq!(detail.fast, Some(false), "fast mode toggle");

                picker.set_card_settings_for_test(HarnessId::Codex, "gpt-sol", 3, true, cx);
                let resolved = picker.resolved(cx);
                assert_eq!(resolved.reasoning, Some(ReasoningLevel::XHigh));
                assert_eq!(resolved.model_options["serviceTier"], "fast");
                let detail = picker.model_detail(cx).unwrap();
                assert_eq!(detail.fast, Some(true));
                assert_eq!(detail.effort.unwrap().selected, 3);
            })
            .unwrap();
    }

    /// A new chat canvas that starts on Cloud by itself (a local project
    /// backed by a connected repository) holds the sidebar's project row,
    /// which has no `supportsModelConfiguration`. The launch context's row
    /// for the same project does, so Claude Code models still offer their
    /// reasoning ladder and 1M context switch, and the launch sends them.
    #[gpui::test]
    fn sidebar_project_canvas_offers_claude_reasoning_and_context(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
        });
        let handle = cx.add_window(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.apply_cloud_account(json!({"user":{"id":"fixture-user"},"orgId":null}));
                state
            });
            Pickers::new(state, cx)
        });
        handle
            .update(cx, |picker, window, cx| {
                let catalog = serde_json::to_value(CloudAgentCatalog {
                    providers: vec![zuse_protocol::cloud_catalog::CloudAgentModels {
                        id: HarnessId::ClaudeCode,
                        name: "Claude Code".into(),
                        default_model: "claude-sonnet-5".into(),
                        models: zuse_providers::claude::catalog::static_models(),
                    }],
                })
                .unwrap();
                let mut context = configurable_cloud_context_for_test();
                context["providerAgentCatalogs"]["boxd"] = catalog;
                context["agentConnections"] =
                    json!([{"agent":"claude","state":"connected","connectionId":"claude"}]);
                // Context row: what the picker's own destination list holds.
                let context_row = json!({"projectId":"repository","accountId":"fixture-user","providerId":"boxd","defaultBranch":"main","supportsModelConfiguration":true});
                picker.seed_cloud_creation_for_test(context, context_row, cx);
                // Sidebar row: what the shell puts on an auto-started canvas.
                picker.state.update(cx, |state, _| {
                    state.cloud_project = Some(json!({"projectId":"repository","accountId":"fixture-user","providerId":"boxd","defaultBranch":"main","canCreate":true}));
                });
                assert!(picker.cloud_models_active(cx));
                assert!(picker.model_configuration_available(cx));

                picker.open_model_menu(window, cx);
                let detail = picker.model_detail(cx).expect("card for the cloud model");
                assert_eq!(detail.row.model.id, "claude-sonnet-5");
                let effort = detail.effort.expect("reasoning slider");
                assert!(effort.labels.len() >= 4, "{:?}", effort.labels);
                let long = detail.long_context.expect("1M context switch");

                picker.set_detail_effort(HarnessId::ClaudeCode, "claude-sonnet-5", 2, cx);
                picker.set_detail_option(
                    HarnessId::ClaudeCode,
                    "claude-sonnet-5",
                    long.option.clone(),
                    long.on_value.clone(),
                    !long.off_default,
                    cx,
                );
                let resolved = picker.resolved(cx);
                assert_eq!(resolved.reasoning, Some(ReasoningLevel::High));
                assert_eq!(resolved.model_options[&long.option], long.on_value.as_str());

                // A row for another project never lends its flag.
                picker.state.update(cx, |state, _| {
                    state.cloud_project = Some(json!({"projectId":"other","accountId":"fixture-user","providerId":"boxd"}));
                });
                assert!(!picker.model_configuration_available(cx));
            })
            .unwrap();
    }

    #[test]
    fn only_transport_and_exact_busy_catalog_failures_are_preparation() {
        assert!(preparation_catalog_error(&zuse_rpc::RpcError::Closed));
        assert!(preparation_catalog_error(&zuse_rpc::RpcError::Transport(
            "reconnecting".into()
        )));
        assert!(preparation_catalog_error(&zuse_rpc::RpcError::Failed(
            "The workspace is busy. Wait for its current operation to finish.".into()
        )));
        assert!(!preparation_catalog_error(&zuse_rpc::RpcError::Capability(
            "cloud_access_denied".into()
        )));
        assert!(!preparation_catalog_error(&zuse_rpc::RpcError::Failed(
            "Busy-looking but unrelated error".into()
        )));
    }
}

impl Pickers {
    pub(super) fn receive_cloud_model_error(
        &mut self,
        error: zuse_rpc::RpcError,
        cx: &mut Context<Self>,
    ) {
        self.cloud.model_waiting = preparation_catalog_error(&error);
        let error = error.to_string();
        let error = if error.contains("Sign in to access cloud workspaces") {
            "Cloud could not connect to your signed-in account. Restart GPUI or reconnect in Settings → General.".to_owned()
        } else {
            error
        };
        self.cloud.set_context_error(error.clone());
        self.cloud.model_error = Some(error.clone());
        if self.cloud.catalog.is_none() {
            self.cloud.harnesses = Loadable::Error(error);
        }
        cx.notify();
    }

    pub(crate) fn finish_cloud_launch(&mut self, cx: &mut Context<Self>) {
        self.freeze_cloud_launch(false, cx);
        self.source_picker
            .update(cx, |picker, cx| picker.clear_selection(cx));
    }
    pub(crate) fn freeze_cloud_launch(&mut self, frozen: bool, cx: &mut Context<Self>) {
        self.cloud.frozen = frozen;
        if frozen {
            self.close(cx);
            self.source_picker
                .update(cx, |picker, cx| picker.dismiss(cx));
        }
        cx.notify();
    }

    pub(crate) fn cloud_launch_frozen(&self) -> bool {
        self.cloud.frozen && !self.is_settings_model()
    }

    pub(super) fn cloud_models_active(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        !self.is_settings_model()
            && (self.cloud.task_binding.is_some()
                || (state.selected_chat.is_none() && state.cloud_project.is_some()))
    }

    pub(super) fn model_catalog(&self, cx: &App) -> &HashMap<HarnessId, Loadable<Vec<Model>>> {
        if self.cloud_models_active(cx) {
            &self.cloud.models
        } else {
            &self.models
        }
    }

    pub(super) fn harness_catalog(&self, cx: &App) -> &Loadable<Vec<HarnessDescriptor>> {
        if self.cloud_models_active(cx) {
            &self.cloud.harnesses
        } else {
            &self.harnesses
        }
    }

    pub(super) fn receive_cloud_models(&mut self, value: Value, cx: &mut Context<Self>) {
        self.model_menu.hover.cancel();
        match serde_json::from_value::<CloudAgentCatalog>(value) {
            Ok(catalog) => {
                self.cloud.model_error = None;
                self.cloud.model_waiting = false;
                super::remember_model_labels(
                    catalog
                        .providers
                        .iter()
                        .flat_map(|provider| provider.models.iter()),
                );
                self.cloud.models = catalog
                    .providers
                    .iter()
                    .map(|provider| (provider.id, Loadable::Ready(provider.models.clone())))
                    .collect();
                self.cloud.harnesses = Loadable::Ready(
                    catalog
                        .providers
                        .iter()
                        .map(|provider| HarnessDescriptor {
                            id: provider.id,
                            name: provider.name.clone(),
                            supports_steering: false,
                            steering_mode: zuse_protocol::SteeringMode::TurnBoundary,
                            reasoning_levels: vec![],
                            installed: true,
                            can_install: false,
                            enabled: Some(true),
                            signed_in: None,
                        })
                        .collect(),
                );
                self.cloud.catalog = Some(catalog);
            }
            Err(_) => {
                self.cloud.model_waiting = false;
                let message = "Cloud models are unavailable. Refresh to retry.".to_owned();
                self.cloud.model_error = Some(message.clone());
                if self.cloud.catalog.is_none() {
                    self.cloud.harnesses = Loadable::Error(message);
                }
            }
        }
        self.catalog_rev += 1;
        cx.notify();
    }

    pub(crate) fn cloud_model_ready(&self, cx: &App) -> bool {
        self.cloud_models_active(cx) && self.selected_model(cx).is_some()
    }

    /// Whether a new cloud chat can pick reasoning and model options. Only
    /// the launch context's project rows carry `supportsModelConfiguration`;
    /// a canvas started from the sidebar's project list holds a row without
    /// it, so the flag is read from the matching context row too.
    pub(super) fn model_configuration_available(&self, cx: &App) -> bool {
        if !self.cloud_models_active(cx) || self.cloud.task_binding.is_some() {
            return true;
        }
        let state = self.state.read(cx);
        let Some(project) = state.cloud_project.as_ref() else {
            return false;
        };
        project["supportsModelConfiguration"] == true
            || self.cloud.rows.iter().any(|row| {
                row["projectId"] == project["projectId"]
                    && row["accountId"] == project["accountId"]
                    && row["supportsModelConfiguration"] == true
            })
    }

    pub(crate) fn cloud_model_notice(&self, cx: &App) -> Option<(String, bool)> {
        if !self.cloud_models_active(cx) || self.cloud_model_ready(cx) {
            return None;
        }
        // Existing tasks carry an explicit saved choice independently of a
        // model-discovery read. Keep that choice during ordinary preparation
        // or reconnect; the conversation already owns its one progress row.
        // Errors remain available inside the model picker for an explicit edit.
        if self.cloud_task_choice().is_some()
            && (self.cloud.model_waiting
                || self.cloud.model_read_pending()
                || matches!(self.cloud.harnesses, Loadable::Idle | Loadable::Loading))
        {
            return None;
        }
        // Loading is quiet: the model picker shows its own placeholder, and a
        // tray row that appears on every chat switch only to vanish a moment
        // later reads as something going wrong.
        match &self.cloud.harnesses {
            Loadable::Error(error) => Some((error.clone(), true)),
            Loadable::Ready(_) => Some(("Choose a Cloud model before starting.".into(), true)),
            _ => None,
        }
    }

    pub(crate) fn refresh_cloud_models(&mut self, cx: &mut Context<Self>) {
        self.cloud.refresh();
        self.ensure_cloud(cx);
        cx.notify();
    }
}
