//! Resolve advertised effort controls without inventing provider option values.
use crate::{Model, ModelOption, ReasoningLevel};
use serde_json::{Map, Value};

pub fn model_effort_option(model: &Model) -> Option<&ModelOption> {
    model.options.iter().find(|option| {
        matches!(
            option.id.as_str(),
            "effort" | "reasoning" | "reasoning_effort"
        ) && option.choices.len() > 1
    })
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffortSetting {
    Reasoning(Option<ReasoningLevel>),
    Option {
        id: String,
        value: String,
        default: bool,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct EffortStop {
    pub label: String,
    pub setting: EffortSetting,
}
#[derive(Clone, Debug, PartialEq)]
pub struct ModelEffort {
    pub label: String,
    pub stops: Vec<EffortStop>,
    pub selected: usize,
}
impl ModelEffort {
    pub fn option_id(&self) -> Option<&str> {
        match &self.stops.first()?.setting {
            EffortSetting::Option { id, .. } => Some(id),
            _ => None,
        }
    }
    /// Selecting a provider default removes only that explicit override.
    /// Unknown positions leave the entire run configuration untouched.
    pub fn select(
        &self,
        index: usize,
        reasoning: &mut Option<ReasoningLevel>,
        options: &mut Map<String, Value>,
    ) -> bool {
        match self.stops.get(index).map(|stop| &stop.setting) {
            Some(EffortSetting::Reasoning(value)) => *reasoning = *value,
            Some(EffortSetting::Option { id, value, default }) => {
                *reasoning = None;
                if *default {
                    options.remove(id);
                } else {
                    options.insert(id.clone(), Value::String(value.clone()));
                }
            }
            None => return false,
        }
        true
    }
}

pub fn model_effort(
    model: &Model,
    reasoning: Option<ReasoningLevel>,
    options: &Map<String, Value>,
) -> Option<ModelEffort> {
    if !model.reasoning_levels.is_empty() {
        let mut stops = vec![EffortStop {
            label: "Default".into(),
            setting: EffortSetting::Reasoning(None),
        }];
        stops.extend(
            model
                .reasoning_levels
                .iter()
                .copied()
                .map(|level| EffortStop {
                    label: reasoning_label(level).into(),
                    setting: EffortSetting::Reasoning(Some(level)),
                }),
        );
        let selected = stops
            .iter()
            .position(|stop| stop.setting == EffortSetting::Reasoning(reasoning))
            .unwrap_or(0);
        return Some(ModelEffort {
            label: "Reasoning".into(),
            stops,
            selected,
        });
    }
    let option = model_effort_option(model)?;
    let current = options
        .get(&option.id)
        .and_then(Value::as_str)
        .unwrap_or(&option.default_choice);
    let selected = option
        .choices
        .iter()
        .position(|choice| choice.id == current)
        .or_else(|| {
            option
                .choices
                .iter()
                .position(|choice| choice.id == option.default_choice)
        })
        .unwrap_or(0);
    Some(ModelEffort {
        label: option.label.clone(),
        selected,
        stops: option
            .choices
            .iter()
            .map(|choice| EffortStop {
                label: choice.label.clone(),
                setting: EffortSetting::Option {
                    id: option.id.clone(),
                    value: choice.id.clone(),
                    default: choice.id == option.default_choice,
                },
            })
            .collect(),
    })
}
fn reasoning_label(level: ReasoningLevel) -> &'static str {
    match level {
        ReasoningLevel::Minimal => "Minimal",
        ReasoningLevel::Low => "Low",
        ReasoningLevel::Medium => "Medium",
        ReasoningLevel::High => "High",
        ReasoningLevel::XHigh => "Extra high",
        ReasoningLevel::Max => "Max",
        ReasoningLevel::Ultra => "Ultra",
        ReasoningLevel::Ultracode => "Ultracode",
        ReasoningLevel::Ultrathink => "Ultrathink",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModelOptionChoice;
    use serde_json::json;
    fn model(id: &str) -> Model {
        Model {
            id: "local".into(),
            label: "Local model".into(),
            description: None,
            reasoning_levels: vec![],
            options: vec![ModelOption {
                id: id.into(),
                label: "Effort".into(),
                choices: vec![
                    ModelOptionChoice {
                        id: "fast".into(),
                        label: "Low".into(),
                    },
                    ModelOptionChoice {
                        id: "balanced".into(),
                        label: "Balanced".into(),
                    },
                    ModelOptionChoice {
                        id: "deep".into(),
                        label: "High".into(),
                    },
                ],
                default_choice: "balanced".into(),
            }],
        }
    }
    #[test]
    fn provider_effort_roundtrips_and_default_removes_only_its_override() {
        for id in ["effort", "reasoning", "reasoning_effort"] {
            let model = model(id);
            let mut options = json!({"contextWindow":"1m","fastMode":"enabled"})
                .as_object()
                .unwrap()
                .clone();
            let mut reasoning = None;
            let control = model_effort(&model, reasoning, &options).unwrap();
            assert_eq!(control.selected, 1);
            assert_eq!(control.option_id(), Some(id));
            assert!(control.select(2, &mut reasoning, &mut options));
            assert_eq!(options[id], "deep");
            assert_eq!(
                model_effort(&model, reasoning, &options).unwrap().selected,
                2
            );
            assert!(control.select(1, &mut reasoning, &mut options));
            assert!(!options.contains_key(id));
            assert_eq!(
                options,
                json!({"contextWindow":"1m","fastMode":"enabled"})
                    .as_object()
                    .unwrap()
                    .clone()
            );
            let before = options.clone();
            assert!(!control.select(99, &mut reasoning, &mut options));
            assert_eq!(options, before);
        }
    }
    #[test]
    fn canonical_reasoning_retains_default_and_precedes_option_controls() {
        let mut model = model("effort");
        model.reasoning_levels = vec![ReasoningLevel::Low, ReasoningLevel::High];
        let mut options = json!({"effort":"deep","contextWindow":"1m"})
            .as_object()
            .unwrap()
            .clone();
        let before = options.clone();
        let mut reasoning = Some(ReasoningLevel::High);
        let control = model_effort(&model, reasoning, &options).unwrap();
        assert_eq!(control.selected, 2);
        assert_eq!(control.option_id(), None);
        assert!(control.select(1, &mut reasoning, &mut options));
        assert_eq!(reasoning, Some(ReasoningLevel::Low));
        assert!(control.select(0, &mut reasoning, &mut options));
        assert_eq!(reasoning, None);
        assert_eq!(options, before);
    }
    #[test]
    fn stale_options_use_advertised_default_and_arbitrary_choices_are_not_effort() {
        let model = model("reasoning_effort");
        let options = json!({"reasoning_effort":"removed"})
            .as_object()
            .unwrap()
            .clone();
        assert_eq!(model_effort(&model, None, &options).unwrap().selected, 1);
        assert!(model_effort(&self::model("lead"), None, &Map::new()).is_none());
        let mut single = self::model("effort");
        single.options[0].choices.truncate(1);
        assert!(model_effort(&single, None, &Map::new()).is_none());
    }
}
