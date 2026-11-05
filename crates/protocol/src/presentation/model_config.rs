//! Remembered choices stay scoped to their provider and exact model identity.
use crate::{Model, ReasoningLevel};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RememberedModelConfig {
    pub provider: String,
    pub model: Option<String>,
    pub reasoning: Option<ReasoningLevel>,
    pub model_options: Map<String, Value>,
}
impl RememberedModelConfig {
    pub fn apply_to(&self, previous: &Value, sandbox: crate::SandboxLevel) -> Value {
        let mut config = if previous.is_object() {
            previous.clone()
        } else {
            serde_json::json!({"sandbox":sandbox})
        };
        config["harness"] = serde_json::json!(self.provider);
        config["model"] = serde_json::json!(self.model);
        config["reasoning"] = serde_json::json!(self.reasoning);
        config["modelOptions"] = serde_json::json!(self.model_options);
        config
    }

    pub fn normalize_for(&mut self, model: &Model) {
        if !model.reasoning_levels.is_empty()
            && self
                .reasoning
                .is_some_and(|level| !model.reasoning_levels.contains(&level))
        {
            self.reasoning = None;
        }
        self.model_options.retain(|id, value| {
            model.options.iter().any(|option| {
                option.id == *id
                    && value.as_str().is_some_and(|value| {
                        value != option.default_choice
                            && option.choices.iter().any(|choice| choice.id == value)
                    })
            })
        });
    }
}

pub(super) fn normalize_history(history: &mut Vec<RememberedModelConfig>) {
    let mut seen = std::collections::HashSet::new();
    let mut bytes = 0usize;
    history.retain(|entry| {
        if entry.provider.len() > 128
            || entry.model.as_ref().is_some_and(|model| model.len() > 1024)
            || entry.model_options.len() > 32
        {
            return false;
        }
        let size = serde_json::to_string(entry).map_or(usize::MAX, |text| text.len());
        if size > 4096 || bytes.saturating_add(size) > 24 * 1024 || seen.len() >= 16 {
            return false;
        }
        if !seen.insert((entry.provider.clone(), entry.model.clone())) {
            return false;
        }
        bytes += size;
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModelOption, ModelOptionChoice, presentation::WorkspacePreferences};
    use serde_json::json;
    fn config(provider: &str, model: Option<&str>, tier: &str) -> RememberedModelConfig {
        RememberedModelConfig {
            provider: provider.into(),
            model: model.map(str::to_owned),
            reasoning: Some(ReasoningLevel::High),
            model_options: serde_json::from_value(json!({"tier":tier})).unwrap(),
        }
    }
    #[test]
    fn remembered_configuration_is_exactly_scoped_and_keeps_agent_default_distinct() {
        let mut preferences = WorkspacePreferences::default();
        for entry in [
            config("codex", Some("same"), "fast"),
            config("claude-code", Some("same"), "off"),
            config("codex", None, "auto"),
            config("codex", Some("default"), "explicit"),
        ] {
            preferences.remember_model_config(entry);
        }
        assert_eq!(
            preferences
                .model_config("codex", Some("same"))
                .unwrap()
                .model_options["tier"],
            "fast"
        );
        assert_eq!(
            preferences
                .model_config("claude-code", Some("same"))
                .unwrap()
                .model_options["tier"],
            "off"
        );
        assert_eq!(
            preferences
                .model_config("codex", None)
                .unwrap()
                .model_options["tier"],
            "auto"
        );
        assert_eq!(
            preferences
                .model_config("codex", Some("default"))
                .unwrap()
                .model_options["tier"],
            "explicit"
        );
        let round: WorkspacePreferences =
            serde_json::from_value(serde_json::to_value(preferences).unwrap()).unwrap();
        assert!(round.model_config("codex", Some("same")).is_some());
        assert!(
            serde_json::from_value::<WorkspacePreferences>(
                json!({"defaultProvider":"codex","defaultModels":{"codex":"legacy"}})
            )
            .unwrap()
            .model_config_history
            .is_empty()
        );
    }
    #[test]
    fn refresh_removes_obsolete_choices_and_explicit_defaults_without_changing_valid_picks() {
        let mut remembered = config("codex", Some("model"), "fast");
        remembered.reasoning = Some(ReasoningLevel::Ultra);
        remembered
            .model_options
            .insert("obsolete".into(), json!("value"));
        let model = Model {
            id: "model".into(),
            label: "Model".into(),
            description: None,
            reasoning_levels: vec![ReasoningLevel::Low, ReasoningLevel::High],
            options: vec![ModelOption {
                id: "tier".into(),
                label: "Tier".into(),
                choices: vec![
                    ModelOptionChoice {
                        id: "standard".into(),
                        label: "Standard".into(),
                    },
                    ModelOptionChoice {
                        id: "fast".into(),
                        label: "Fast".into(),
                    },
                ],
                default_choice: "standard".into(),
            }],
        };
        remembered.normalize_for(&model);
        assert_eq!(remembered.reasoning, None);
        assert_eq!(remembered.model_options.len(), 1);
        remembered
            .model_options
            .insert("tier".into(), json!("standard"));
        remembered.normalize_for(&model);
        assert!(remembered.model_options.is_empty());
    }
    #[test]
    fn explicit_default_precedes_history_and_agent_default_survives_recent_choices() {
        let mut preferences = WorkspacePreferences::default();
        preferences.record_model(crate::presentation::ModelSelection {
            provider: "codex".into(),
            model: "previous".into(),
        });
        preferences.remember_model_config(config("codex", None, "auto"));
        assert_eq!(preferences.starting_model(), Some(("codex".into(), None)));
        preferences.set_default(crate::presentation::ModelSelection {
            provider: "claude-code".into(),
            model: "chosen".into(),
        });
        assert_eq!(
            preferences.starting_model(),
            Some(("claude-code".into(), Some("chosen".into())))
        );
        preferences.default_models.remove("claude-code");
        assert_eq!(
            preferences.starting_model(),
            Some(("claude-code".into(), None))
        );
    }
    #[test]
    fn model_patch_preserves_permission_and_unowned_fields() {
        let config = config("codex", Some("model"), "fast");
        let patched = config.apply_to(
            &json!({"harness":"codex","sandbox":"auto","plan":true,"futureSetting":{"keep":true}}),
            crate::SandboxLevel::DangerFullAccess,
        );
        assert_eq!(patched["sandbox"], "auto");
        assert_eq!(patched["plan"], true);
        assert_eq!(patched["futureSetting"]["keep"], true);
        assert_eq!(patched["modelOptions"]["tier"], "fast");
        assert_eq!(
            config.apply_to(&Value::Null, crate::SandboxLevel::WorkspaceWrite)["sandbox"],
            "workspace-write"
        );
    }
    #[test]
    fn history_is_bounded_and_latest_configuration_wins() {
        let mut preferences = WorkspacePreferences::default();
        preferences.remember_model_config(config("codex", Some("model"), "first"));
        preferences.remember_model_config(config("codex", Some("model"), "latest"));
        assert_eq!(preferences.model_config_history.len(), 1);
        assert_eq!(
            preferences
                .model_config("codex", Some("model"))
                .unwrap()
                .model_options["tier"],
            "latest"
        );
        for i in 0..40 {
            preferences.remember_model_config(config("codex", Some(&format!("model-{i}")), "fast"));
        }
        assert_eq!(preferences.model_config_history.len(), 16);
        assert!(
            preferences
                .model_config("codex", Some("model-39"))
                .is_some()
        );
        let mut huge = config("codex", Some("huge"), "fast");
        huge.model_options
            .insert("payload".into(), json!("x".repeat(8000)));
        preferences.remember_model_config(huge);
        assert!(preferences.model_config("codex", Some("huge")).is_none());
    }
}
