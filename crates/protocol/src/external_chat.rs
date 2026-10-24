//! Device-local provider conversations that can be continued without a new first turn.
use crate::{Chat, HarnessId, Space};
use serde::{Deserialize, Serialize};

pub const LIST: &str = "ListExternalChats";
pub const IMPORT: &str = "ImportExternalChat";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub id: String,
    pub provider: HarnessId,
    pub title: String,
    pub preview: String,
    pub project_path: String,
    pub project_name: String,
    pub updated_at: i64,
    pub available: bool,
}
impl Row {
    pub fn importable(&self) -> bool {
        self.available
            && !self.project_path.trim().is_empty()
            && !matches!(
                self.project_name.trim().to_lowercase().as_str(),
                "t" | "temp" | "temporary" | "temporary folder"
            )
    }
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        let provider = match self.provider {
            HarnessId::ClaudeCode => "Claude Code",
            HarnessId::Codex => "Codex",
            _ => "",
        };
        let fields: [&str; 5] = [
            &self.title,
            &self.preview,
            &self.project_name,
            &self.project_path,
            provider,
        ];
        fields
            .iter()
            .any(|value| value.to_lowercase().contains(&query))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    pub chat: Chat,
    pub project: Space,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportRequest {
    pub id: String,
    pub request_id: String,
}
