//! Retained-machine operations; recovery retirement requires a separate verified barrier.
use serde::{Deserialize, Serialize};

pub const CAPABILITY: &str = "rust-workspace-lifecycle-v1";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Pause,
    Archive,
    Unarchive,
    Resume,
}
impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Archive => "archive",
            Self::Unarchive => "unarchive",
            Self::Resume => "resume",
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionRequest {
    pub workspace_id: String,
    pub command_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Directive {
    pub operation_id: String,
    pub action: Action,
    pub phase: String,
    pub generation: i64,
    pub storage_incarnation_id: String,
    pub source_machine_id: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RuntimeRequest {
    Query,
    Quiesced {
        #[serde(rename = "operationId")]
        operation_id: String,
    },
    Resumed {
        #[serde(rename = "operationId")]
        operation_id: String,
    },
}
