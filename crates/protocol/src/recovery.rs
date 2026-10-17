//! Trusted user review of uncertain tool outcomes. Never part of model tools.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryCall {
    pub agent_id: String,
    pub agent_name: String,
    pub call_id: String,
    pub tool: String,
    pub input: serde_json::Value,
    /// Compare-and-set fence for the exact call and receipt being inspected.
    pub revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverySnapshot {
    pub session_id: String,
    pub calls: Vec<RecoveryCall>,
    pub reviewed_count: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum RecoveryAction {
    RetryAfterInspection,
    RecordOutcome { output: String },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveRecovery {
    pub chat_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub call_id: String,
    pub revision: String,
    pub decision: RecoveryAction,
}
