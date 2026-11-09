//! Canonical API admission for server-admitted-v2 computers. Kept unadvertised
//! until producer, queue, runtime progress and asset contracts are certified.
use super::*;
use crate::access::Access;
use sqlx::Row;
use zuse_protocol::cloud_task_commands as wire;
mod admission;
mod agents;
pub(super) mod dispatch;
mod drain;
pub(super) mod initial;
mod interrupt;
mod progress;
mod queue;
mod read;
mod sessions;
mod validation;
pub const MIGRATION: &str = include_str!("../../migrations/0026_task_command_admission.sql");
/// Runtime paths are resolved on the guest; this ciphertext persists the
/// immutable selected task policy independently of expiring launch intents.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Policy {
    pub agent: String,
    pub connection_id: String,
    pub connection_owner_id: String,
    pub runtime_mode: zuse_protocol::cloud::CloudRuntimeMode,
    pub initial_model: String,
    pub reasoning: Option<zuse_protocol::ReasoningLevel>,
    pub model_options: serde_json::Map<String, Value>,
    pub catalog: zuse_protocol::cloud_catalog::CloudAgentModels,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub catalogs:
        std::collections::BTreeMap<String, zuse_protocol::cloud_catalog::CloudAgentModels>,
}
fn error(code: &'static str) -> ApiError {
    ApiError::conflict(code)
}
fn context(owner: &str, id: &str, session: &str, kind: &str, key: &str) -> String {
    format!("task-command-v2:{owner}:{id}:{session}:{kind}:{key}")
}
pub(crate) fn command_mode(row: &Value) -> bool {
    row["request_config"]["commandDeliveryMode"] == wire::MODE
        && row["request_config"]["rustCommandAdmissionVersion"] == wire::VERSION
        && row["request_config"]["rustCommandMailboxEnabled"] == true
        && row["request_config"]["runtimeRpcProtocol"] == zuse_protocol::cloud_gateway::RUST_RPC
}
fn authorized_actor(access: &Access) -> Option<Value> {
    access.membership.as_ref().map(|m|json!({"subject":access.actor.account_id,"membershipId":m.id,"organizationId":m.organization_id,"role":m.role.slug}))
}

mod selection;
