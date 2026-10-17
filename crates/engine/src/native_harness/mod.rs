//! Application session adapter for the owned Zuse kernel.
mod events;
mod host;
mod runtime;
use crate::harness_models::{HarnessModel, HarnessModels};
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
use zuse_harness::{Error, ModelInput, ModelOutput, Options};
use zuse_protocol::{AgentEvent, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode};
use zuse_providers::{Harness, HarnessError, ModelCatalog, RunControls, RunIdentity};

#[async_trait]
trait Models: Send + Sync {
    async fn inventory(
        &self,
        force: bool,
        cancel: &CancellationToken,
    ) -> Result<Vec<HarnessModel>, Error>;
    async fn request(&self, input: ModelInput) -> Result<ModelOutput, Error>;
    fn remove_root(&self, root: &str);
}
#[async_trait]
impl Models for HarnessModels {
    async fn inventory(
        &self,
        force: bool,
        cancel: &CancellationToken,
    ) -> Result<Vec<HarnessModel>, Error> {
        self.models(force, cancel).await
    }
    async fn request(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        self.request(input).await
    }
    fn remove_root(&self, root: &str) {
        self.remove_root(root);
    }
}
#[derive(Clone)]
pub(crate) struct NativeHarness {
    profile: PathBuf,
    device_id: String,
    models: Arc<dyn Models>,
    command_host: PathBuf,
    uploads: crate::Uploads,
}
fn failure(error: impl std::fmt::Display) -> HarnessError {
    HarnessError::Protocol(error.to_string())
}
const REASONING: &[ReasoningLevel] = &[
    ReasoningLevel::Low,
    ReasoningLevel::Medium,
    ReasoningLevel::High,
];
impl NativeHarness {
    pub fn new(
        profile: PathBuf,
        device_id: String,
        models: HarnessModels,
        uploads: crate::Uploads,
    ) -> Result<Self, HarnessError> {
        Ok(Self {
            profile,
            device_id,
            models: Arc::new(models),
            command_host: std::env::current_exe()?,
            uploads,
        })
    }
    async fn options(
        &self,
        request: &RunRequest,
        cancel: &CancellationToken,
    ) -> Result<Options, HarnessError> {
        let inventory = self
            .models
            .inventory(false, cancel)
            .await
            .map_err(failure)?;
        let selected = match &request.model {
            Some(id) => inventory.iter().find(|model| &model.id == id),
            None => inventory.first(),
        }
        .ok_or_else(|| {
            failure("Connect a model account and choose one of its available models in Settings.")
        })?;
        let mut options = Options::new(&selected.id);
        options.reasoning = request
            .reasoning
            .and_then(|r| serde_json::to_value(r).ok())
            .and_then(|v| v.as_str().map(str::to_owned));
        if let Some(window) = selected.context_window_tokens {
            options.window_tokens = window;
        }
        options.compact_tokens = selected.auto_compact_token_limit;
        if let Some(percent) = selected.effective_context_window_percent {
            options.effective_window_percent = percent;
        }
        Ok(options)
    }
}
#[async_trait]
impl Harness for NativeHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Zuse
    }
    fn display_name(&self) -> &str {
        "Zuse"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        REASONING
    }
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    fn same_runtime(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd
            && live.model == next.model
            && live.reasoning == next.reasoning
            && live.model_options == next.model_options
            && live.sandbox == next.sandbox
            && live.auto_approve == next.auto_approve
    }
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        live.cwd == next.cwd && live.mcp == next.mcp
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(self.model_catalog(false).await?.models)
    }
    async fn model_catalog(&self, force: bool) -> Result<ModelCatalog, HarnessError> {
        let models = self
            .models
            .inventory(force, &CancellationToken::new())
            .await
            .map_err(failure)?;
        Ok(ModelCatalog {
            source: "live",
            models: models
                .into_iter()
                .map(|model| Model {
                    id: model.id,
                    label: model.label,
                    description: None,
                    reasoning_levels: REASONING.to_vec(),
                    options: vec![],
                })
                .collect(),
        })
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        runtime::start(self.clone(), request, controls, None).await
    }
    async fn run_delivery(
        &self,
        request: RunRequest,
        controls: RunControls,
        identity: RunIdentity,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        runtime::start(self.clone(), request, controls, Some(identity)).await
    }
}

#[cfg(test)]
mod tests;
