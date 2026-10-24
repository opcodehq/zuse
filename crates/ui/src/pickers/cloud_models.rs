//! Cloud launch choices come from the prepared runtime's catalogue, not the viewer's CLIs.
use super::*;
use serde_json::Value;
use zuse_protocol::cloud_catalog::CloudAgentCatalog;

impl Pickers {
    pub(crate) fn freeze_cloud_launch(&mut self, frozen: bool, cx: &mut Context<Self>) {
        self.cloud.frozen = frozen;
        if frozen {
            self.close(cx);
        }
        cx.notify();
    }

    pub(super) fn cloud_launch_frozen(&self) -> bool {
        self.cloud.frozen && !self.is_settings_model()
    }

    pub(super) fn cloud_models_active(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        !self.is_settings_model() && state.selected_chat.is_none() && state.cloud_project.is_some()
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
