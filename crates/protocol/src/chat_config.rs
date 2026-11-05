//! Changes name the fields they own, so coalescing preserves other choices.
use crate::{ChatConfig, SandboxLevel};

#[derive(Clone, Debug, Default)]
pub struct ChatConfigPatch {
    model: Option<ChatConfig>,
    sandbox: Option<SandboxLevel>,
    plan: Option<bool>,
}

impl ChatConfigPatch {
    pub fn model(config: ChatConfig) -> Self {
        Self {
            model: Some(config),
            ..Self::default()
        }
    }

    pub fn sandbox(sandbox: SandboxLevel) -> Self {
        Self {
            sandbox: Some(sandbox),
            ..Self::default()
        }
    }

    pub fn plan(plan: bool) -> Self {
        Self {
            plan: Some(plan),
            ..Self::default()
        }
    }

    pub fn merge(&mut self, newer: Self) {
        if newer.model.is_some() {
            self.model = newer.model;
        }
        if newer.sandbox.is_some() {
            self.sandbox = newer.sandbox;
        }
        if newer.plan.is_some() {
            self.plan = newer.plan;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.sandbox.is_none() && self.plan.is_none()
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
        if let Some(plan) = self.plan {
            config.plan = plan;
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
            plan: false,
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
        let mut saved = config("old", SandboxLevel::Auto);
        saved.plan = true;
        let result = patch.apply_to(&saved);
        assert_eq!(result.model.as_deref(), Some("next"));
        assert_eq!(result.sandbox, SandboxLevel::Auto);
        assert!(result.plan);
    }
    #[test]
    fn coalescing_keeps_the_last_choice_for_each_owned_field() {
        let mut patch = ChatConfigPatch::model(config("first", SandboxLevel::WorkspaceWrite));
        patch.merge(ChatConfigPatch::sandbox(SandboxLevel::Auto));
        patch.merge(ChatConfigPatch::plan(true));
        patch.merge(ChatConfigPatch::model(config(
            "last",
            SandboxLevel::DangerFullAccess,
        )));
        let result = patch.apply_to(&config("host", SandboxLevel::WorkspaceWrite));
        assert_eq!(result.model.as_deref(), Some("last"));
        assert_eq!(result.sandbox, SandboxLevel::Auto);
        assert!(result.plan);
        let permission = ChatConfigPatch::sandbox(SandboxLevel::WorkspaceWrite);
        assert_eq!(permission.apply_to(&result).model.as_deref(), Some("last"));
        assert!(permission.apply_to(&result).plan);
        assert!(!ChatConfigPatch::plan(false).apply_to(&result).plan);
        assert!(ChatConfigPatch::default().is_empty());
    }
}
