//! Explicit user Git actions. Reads may retry; mutations retain receipt uncertainty.
use crate::GitFileStatus;
use serde::{Deserialize, Serialize};
/// A successful snapshot survives refresh failures and reconnects. Freshness
/// belongs to this resource, not to the lifetime of a particular screen.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceControlFrame {
    #[serde(default)]
    pub reset: bool,
    pub snapshot: Option<serde_json::Value>,
    pub updated_at: Option<i64>,
    pub refreshing: bool,
    pub error: Option<String>,
    #[serde(default)]
    pub last_fetch_at: Option<i64>,
    #[serde(default)]
    pub fetch_error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitWorkspace {
    pub chat_id: String,
    pub checkout_id: String,
    pub cwd: String,
    pub branch: String,
    pub head_sha: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<GitFileStatus>,
    pub complete: bool,
    pub revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_pull_request_url: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum GitOperation {
    Stage {
        paths: Vec<String>,
    },
    Unstage {
        paths: Vec<String>,
    },
    Commit {
        message: String,
    },
    Push,
    Pull,
    Fetch,
    CreateBranch {
        name: String,
    },
    SwitchBranch {
        name: String,
    },
    CreatePullRequest {
        title: String,
        body: String,
        base: String,
        base_repository: Option<String>,
        draft: bool,
    },
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitTarget {
    pub chat_id: String,
    pub cwd: String,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitAction {
    pub chat_id: String,
    pub cwd: String,
    pub checkout_id: String,
    pub revision: String,
    pub operation: GitOperation,
}
