//! Changes name the fields they own, so coalescing preserves other choices.
use crate::{ChatConfig, SandboxLevel};

#[derive(Clone, Debug, Default)]
pub struct ChatConfigPatch {
    model: Option<ChatConfig>,
    sandbox: Option<SandboxLevel>,
}

impl ChatConfigPatch {
    pub fn model(config: ChatConfig) -> Self {
        Self {
            model: Some(config),
            sandbox: None,
        }
    }

    pub fn sandbox(sandbox: SandboxLevel) -> Self {
        Self {
            model: None,
            sandbox: Some(sandbox),
        }
    }

    pub fn merge(&mut self, newer: Self) {
        if newer.model.is_some() {
            self.model = newer.model;
        }
        if newer.sandbox.is_some() {
            self.sandbox = newer.sandbox;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.sandbox.is_none()
    }

    pub fn apply_to(&self, saved: &ChatConfig) -> ChatConfig {
        let mut config = saved.clone();
        if let Some(model) = &self.model {
            config.harness = model.harness;
            config.model = model.model.clone();
            config.reasoning = model.reasoning;
            config.model_options = model.model_options.clone();
        }
        if let Some(sandbox) = self.sandbox {
            config.sandbox = sandbox;
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HarnessId;
    fn config(model: &str, sandbox: SandboxLevel) -> ChatConfig {
        ChatConfig {
            harness: HarnessId::Codex,
            model: Some(model.into()),
            reasoning: None,
            model_options: Default::default(),
            sandbox,
        }
    }
    #[test]
    fn model_edits_keep_the_latest_host_permission() {
        let patch = ChatConfigPatch::model(config("next", SandboxLevel::DangerFullAccess));
        let saved = config("old", SandboxLevel::ReadOnly);
        let result = patch.apply_to(&saved);
        assert_eq!(result.model.as_deref(), Some("next"));
        assert_eq!(result.sandbox, SandboxLevel::ReadOnly);
    }
    #[test]
    fn coalescing_keeps_the_last_choice_for_each_owned_field() {
        let mut patch = ChatConfigPatch::model(config("first", SandboxLevel::WorkspaceWrite));
        patch.merge(ChatConfigPatch::sandbox(SandboxLevel::ReadOnly));
        patch.merge(ChatConfigPatch::model(config(
            "last",
            SandboxLevel::DangerFullAccess,
        )));
        let result = patch.apply_to(&config("host", SandboxLevel::WorkspaceWrite));
        assert_eq!(result.model.as_deref(), Some("last"));
        assert_eq!(result.sandbox, SandboxLevel::ReadOnly);
        let permission = ChatConfigPatch::sandbox(SandboxLevel::WorkspaceWrite);
        assert_eq!(permission.apply_to(&result).model.as_deref(), Some("last"));
        assert!(ChatConfigPatch::default().is_empty());
    }
}
