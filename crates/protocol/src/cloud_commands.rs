//! Durable encrypted mailbox contracts. Payload bytes stay opaque to the control plane.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const PROTOCOL_VERSION: u32 = 3;
/// Native commands are accepted only on explicitly negotiated Rust workspaces.
pub fn session_command_kind(kind: crate::session_commands::SessionCommandKind) -> &'static str {
    use crate::session_commands::SessionCommandKind::*;
    match kind {
        Run => "zuse.session.run",
        Steer => "zuse.session.steer",
        Interrupt => "zuse.session.interrupt",
        RespondInput => "zuse.session.respond-input",
        StopBackgroundTask => "zuse.session.stop-background-task",
    }
}
pub fn is_session_command_kind(kind: &str) -> bool {
    matches!(
        kind,
        "zuse.session.run"
            | "zuse.session.steer"
            | "zuse.session.interrupt"
            | "zuse.session.respond-input"
            | "zuse.session.stop-background-task"
    )
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CloudSessionCommand {
    pub command_id: String,
    pub session_id: String,
    pub payload: crate::session_commands::SessionCommandPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on: Option<crate::session_commands::CommandBasedOn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}
pub const MAX_ENVELOPE_BYTES: usize = 256 * 1024;
pub const MAX_NONTERMINAL: i64 = 256;
pub const MAX_MAILBOX_BYTES: i64 = 8 * 1024 * 1024;
pub const LEASE_TTL_MS: i64 = 30_000;
pub const WATCH_MAX_WAIT_MS: u32 = 25_000;
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandState {
    Reserved,
    Accepted,
    WaitingForRuntime,
    Leased,
    LeaseExpiredAwaitingFence,
    Blocked,
    Applied,
    Rejected,
    Expired,
    Cancelled,
    OutcomeUnknown,
}
impl CommandState {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Applied | Self::Rejected | Self::Expired | Self::Cancelled | Self::OutcomeUnknown
        )
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AcknowledgmentState {
    Applied,
    Rejected,
    Expired,
    OutcomeUnknown,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    pub blob_id: String,
    pub ciphertext_bytes: i64,
    pub ciphertext_sha256: String,
    pub key_version: i64,
    pub expires_at: i64,
}
/// Never Debug: encrypted bytes and author metadata must not enter runtime logs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandEnvelope {
    pub protocol_version: u32,
    pub workspace_id: String,
    pub session_id: String,
    pub command_id: String,
    pub kind: String,
    pub fingerprint: String,
    pub schema_version: i64,
    pub key_version: i64,
    pub destruction_fence: i64,
    pub created_at: i64,
    pub iv: String,
    pub ciphertext: String,
    pub dependencies: Vec<Dependency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<Value>,
}
impl CommandEnvelope {
    /// The original ordered JSON array is authenticated by AES-GCM on the client/guest.
    pub fn additional_data(&self) -> Vec<u8> {
        serde_json::to_vec(&json!([
            "zuse-cloud-command-v3",
            self.workspace_id,
            self.session_id,
            self.command_id,
            self.kind,
            self.fingerprint,
            self.schema_version,
            self.key_version,
            self.destruction_fence,
            self.created_at,
            self.dependencies
        ]))
        .expect("JSON values")
    }
    pub fn eligible(&self) -> bool {
        self.protocol_version == PROTOCOL_VERSION
            && (self.kind == "messages.send" || is_session_command_kind(&self.kind))
            && self.schema_version == 1
            && self.key_version == 1
            && self.dependencies.is_empty()
            && [&self.workspace_id, &self.session_id, &self.command_id]
                .into_iter()
                .all(|s| !s.is_empty() && s.len() <= 512)
            && self.fingerprint.starts_with("hmac-sha256:")
            && self.fingerprint.len() == 55
            && (0..=MAX_SAFE_INTEGER).contains(&self.created_at)
            && (0..=MAX_SAFE_INTEGER).contains(&self.destruction_fence)
            && !self.iv.is_empty()
            && self.iv.len() <= 32
            && !self.ciphertext.is_empty()
            && serde_json::to_vec(self).is_ok_and(|bytes| bytes.len() <= MAX_ENVELOPE_BYTES)
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLease {
    pub command: CommandEnvelope,
    pub workspace_sequence: i64,
    pub lease_token: String,
    pub lease_deadline: i64,
    pub runtime_generation: i64,
    pub provider_sandbox_id: String,
    pub storage_incarnation_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeAcknowledgment {
    pub command_id: String,
    pub lease_token: String,
    pub fingerprint: String,
    pub state: AcknowledgmentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_iv: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_ciphertext: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseRequest {
    pub storage_incarnation_id: String,
    #[serde(default)]
    pub wait_ms: Option<u32>,
    #[serde(default)]
    pub after_revision: Option<i64>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeasePage {
    pub leases: Vec<RuntimeLease>,
    pub nonterminal_count: i64,
    pub mailbox_revision: i64,
    pub fence_required: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cloud_command_original_v3_aad_and_wire_round_trip() {
        let value = json!({"protocolVersion":3,"workspaceId":"ws-original","sessionId":"session-original","commandId":"command-original","kind":"messages.send","fingerprint":"hmac-sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","schemaVersion":1,"keyVersion":1,"destructionFence":2,"createdAt":1790000000000i64,"iv":"AAAAAAAAAAAAAAAA","ciphertext":"encrypted-original","dependencies":[]});
        let command: CommandEnvelope = serde_json::from_value(value.clone()).unwrap();
        assert!(command.eligible());
        // Byte fixture from the original JSON.stringify array contract. Ordering is cryptographic.
        assert_eq!(
            String::from_utf8(command.additional_data()).unwrap(),
            r#"["zuse-cloud-command-v3","ws-original","session-original","command-original","messages.send","hmac-sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",1,1,2,1790000000000,[]]"#
        );
        assert_eq!(serde_json::to_value(command.clone()).unwrap(), value);
        let mut invalid = command.clone();
        invalid.created_at = 9_007_199_254_740_992;
        assert!(!invalid.eligible());
        let mut invalid = command;
        invalid.schema_version = 2;
        assert!(!invalid.eligible());
    }
}
