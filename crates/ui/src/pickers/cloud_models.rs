//! Cloud launch choices come from the prepared runtime's catalogue, not the viewer's CLIs.
use super::*;
use serde_json::Value;
use zuse_protocol::cloud_catalog::CloudAgentCatalog;

pub(super) fn preparation_catalog_error(error: &zuse_rpc::RpcError) -> bool {
    matches!(error, zuse_rpc::RpcError::Closed | zuse_rpc::RpcError::Transport(_))
        || matches!(error, zuse_rpc::RpcError::Failed(message)
            if message == "The workspace is busy. Wait for its current operation to finish.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[gpui::test]
    fn saved_task_model_survives_busy_catalog_and_scoped_catalog_recovery(cx: &mut gpui::TestAppContext) {
        let runtime=tokio::runtime::Runtime::new().unwrap();let _entered=runtime.enter();
        let(send,_requests)=tokio::sync::mpsc::channel(1);
        let(_responses,receive)=tokio::sync::mpsc::channel(1);
        let controller=crate::state::EngineHandle::from_test_client(zuse_rpc::RpcClient::new(send,receive));
        let state=cx.new(|_|AppState::new());
        let pickers=cx.new(|cx|Pickers::new(state,cx));
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

    #[test]
    fn only_transport_and_exact_busy_catalog_failures_are_preparation() {
        assert!(preparation_catalog_error(&zuse_rpc::RpcError::Closed));
        assert!(preparation_catalog_error(&zuse_rpc::RpcError::Transport("reconnecting".into())));
        assert!(preparation_catalog_error(&zuse_rpc::RpcError::Failed("The workspace is busy. Wait for its current operation to finish.".into())));
        assert!(!preparation_catalog_error(&zuse_rpc::RpcError::Capability("cloud_access_denied".into())));
        assert!(!preparation_catalog_error(&zuse_rpc::RpcError::Failed("Busy-looking but unrelated error".into())));
    }
}

impl Pickers {
    pub(super) fn receive_cloud_model_error(&mut self, error: zuse_rpc::RpcError, cx: &mut Context<Self>) {
        self.cloud.model_waiting = preparation_catalog_error(&error);
        let error = error.to_string();
        let error = if error.contains("Sign in to access cloud workspaces") {
            "Cloud could not connect to your signed-in account. Restart GPUI or reconnect in Settings → General.".to_owned()
        } else { error };
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

    pub(super) fn cloud_launch_frozen(&self) -> bool {
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
        match serde_json::from_value::<CloudAgentCatalog>(value) {
            Ok(catalog) => {
                self.cloud.model_error = None;
                self.cloud.model_waiting = false;
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

    pub(super) fn model_configuration_available(&self, cx: &App) -> bool {
        !self.cloud_models_active(cx)
            || self.cloud.task_binding.is_some()
            || self
                .state
                .read(cx)
                .cloud_project
                .as_ref()
                .is_some_and(|project| project["supportsModelConfiguration"] == true)
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
                || matches!(self.cloud.harnesses, Loadable::Idle | Loadable::Loading)) {
            return None;
        }
        Some(match &self.cloud.harnesses {
            Loadable::Error(error) => (error.clone(), true),
            Loadable::Ready(_) => ("Choose a Cloud model before starting.".into(), true),
            _ => ("Loading Cloud models…".into(), false),
        })
    }

    pub(crate) fn refresh_cloud_models(&mut self, cx: &mut Context<Self>) {
        self.cloud.refresh();
        self.ensure_cloud(cx);
        cx.notify();
    }
}
