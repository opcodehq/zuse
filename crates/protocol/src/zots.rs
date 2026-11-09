//! Durable coordinator contracts shared by all application clients.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const CAPABILITY: &str = "zots.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotRepository {
    pub device_id: String,
    pub space_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotModel {
    pub harness: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotConfig {
    pub name: String,
    pub purpose: String,
    pub home_device_id: String,
    pub repositories: Vec<ZotRepository>,
    pub coordinator_model: ZotModel,
    pub worker_device_id: String,
    pub max_concurrent_workers: u32,
    pub max_followups: u32,
    /// Saved pixel character; absent uses one derived from the Zot's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub avatar: Option<ZotAvatar>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotAvatar {
    pub mascot: String,
    pub color: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ZotStatus {
    Active,
    Paused,
    Stopped,
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotWorker {
    pub chat_id: String,
    pub device_id: String,
    pub space_id: String,
    pub objective: String,
    pub followups: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotDecision {
    pub id: String,
    pub worker_chat_id: Option<String>,
    pub question: String,
    pub answer: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Zot {
    pub id: String,
    pub revision: u32,
    #[serde(default)]
    pub activity_revision: u32,
    pub config: ZotConfig,
    pub status: ZotStatus,
    pub coordinator_chat_id: String,
    pub previous_coordinator_chats: Vec<String>,
    pub workers: Vec<ZotWorker>,
    pub decisions: Vec<ZotDecision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotFile {
    pub path: String,
    pub version: u32,
    pub content: String,
}

/// IDs are caller-minted and reused verbatim after a transport failure.
#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotCommand {
    pub command_id: String,
    pub zot_id: String,
    pub expected_revision: Option<u32>,
    pub action: ZotAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ZotAction {
    Create {
        config: ZotConfig,
    },
    Configure {
        config: ZotConfig,
    },
    SetStatus {
        status: ZotStatus,
    },
    AdoptWorker {
        worker: ZotWorker,
    },
    Delegate {
        worker: ZotWorker,
        model: ZotModel,
        base: String,
    },
    MessageWorker {
        chat_id: String,
        text: String,
        automatic: bool,
    },
    StopWorker {
        chat_id: String,
    },
    ArchiveWorker {
        chat_id: String,
    },
    ReleaseWorker {
        chat_id: String,
    },
    Decide {
        decision: ZotDecision,
    },
    WriteFile {
        path: String,
        content: String,
        expected_version: u32,
    },
    RestoreFile {
        path: String,
        version: u32,
        expected_version: u32,
    },
    FreshConversation,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotEvent {
    pub id: String,
    pub zot_id: String,
    pub worker_chat_id: String,
    pub kind: String,
    pub summary: String,
    pub acknowledged: bool,
}

/// Canonical command schema exposed by both application discovery and MCP.
pub fn command_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(ZotCommand)).expect("command schema serializes")
}
