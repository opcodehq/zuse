//! Launch choices supplied by the cloud runtime, independent of local installations.
use crate::{HarnessId, Model};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAgentCatalog {
    pub providers: Vec<CloudAgentModels>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudAgentModels {
    pub id: HarnessId,
    pub name: String,
    pub default_model: String,
    pub models: Vec<Model>,
}

impl CloudAgentModels {
    pub fn resolve_model(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|model| model.id == id).or_else(|| {
            let family = match (self.id, id) {
                (HarnessId::ClaudeCode, "sonnet") => "claude-sonnet-",
                (HarnessId::ClaudeCode, "opus") => "claude-opus-",
                (HarnessId::ClaudeCode, "haiku") => "claude-haiku-",
                (HarnessId::ClaudeCode, "fable") => "claude-fable-",
                _ => return None,
            };
            self.models
                .iter()
                .find(|model| model.id.starts_with(family))
        })
    }
}
