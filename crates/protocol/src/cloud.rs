//! Cloud contracts shared by the hosted API and native/browser clients.
use serde::{Deserialize, Serialize};

pub const ARCHIVE_RETENTION_MS: i64 = 10 * 24 * 60 * 60 * 1000;
pub const DELETION_TOMBSTONE_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Public runtime keys only; reusable credentials are never part of enrollment input.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudRuntimeBootstrapRequest {
    pub credential_public_jwk: String,
    pub signing_public_jwk: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Vec<String>>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudRuntimeBootstrapAck {
    pub runtime_generation: i64,
    pub gateway_epoch: i64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CloudRuntimeMode {
    Auto,
    ApprovalRequired,
    AutoAcceptEdits,
    AutoAcceptEditsAndBash,
    FullAccess,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudForkSource {
    pub chat_id: String,
    pub session_id: String,
    pub message_id: String,
}

/// The sealed initial intent retains the original command/turn identity.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudLaunchIntent {
    pub command_id: String,
    pub turn_id: String,
    pub title: String,
    pub agent: String,
    pub model: String,
    pub permissions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_mode: Option<CloudRuntimeMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_rename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_source: Option<CloudForkSource>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArchivePhase {
    Uploading,
    Verifying,
    Retiring,
    Retired,
    Restoring,
    Restored,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveManifest {
    pub version: u32,
    pub operation_id: String,
    pub workspace_id: String,
    pub chat_id: String,
    pub session_id: String,
    pub chunks: Vec<String>,
    pub archive_sha256: String,
}

impl ArchiveManifest {
    pub fn valid(&self) -> bool {
        self.version == 1
            && !self.operation_id.is_empty()
            && !self.workspace_id.is_empty()
            && !self.chat_id.is_empty()
            && !self.session_id.is_empty()
            && (1..=8192).contains(&self.chunks.len())
            && digest(&self.archive_sha256)
            && self.chunks.iter().all(|s| digest(s))
    }
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Exact legacy request_config.recoveryArchive shape; object contents stay encrypted.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryArchive {
    pub version: u32,
    pub id: String,
    pub source_sandbox_id: String,
    pub archived_at_ms: i64,
    pub expires_at_ms: i64,
    pub phase: ArchivePhase,
    pub verified_chunks: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<ArchiveManifest>,
}

impl RecoveryArchive {
    pub fn valid_for(&self, workspace: &str, chat: &str, session: &str) -> bool {
        self.version == 1
            && !self.id.is_empty()
            && !self.source_sandbox_id.is_empty()
            && self.archived_at_ms >= 0
            && self.expires_at_ms > self.archived_at_ms
            && match &self.manifest {
                Some(manifest) => {
                    manifest.valid()
                        && manifest.operation_id == self.id
                        && manifest.workspace_id == workspace
                        && manifest.chat_id == chat
                        && manifest.session_id == session
                        && self.verified_chunks <= manifest.chunks.len()
                }
                None => self.verified_chunks == 0 && self.phase == ArchivePhase::Uploading,
            }
    }

    pub fn backup_verified(&self) -> bool {
        self.manifest.as_ref().is_some_and(|manifest| {
            manifest.valid() && self.verified_chunks == manifest.chunks.len()
        }) && matches!(
            self.phase,
            ArchivePhase::Retiring
                | ArchivePhase::Retired
                | ArchivePhase::Restoring
                | ArchivePhase::Restored
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudArchiveStatus {
    pub workspace_id: String,
    pub archived: bool,
    pub machine_retained: bool,
    pub backup_verified: bool,
    pub compute_retired: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retention_deadline: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<ArchivePhase>,
    pub verified_chunks: usize,
    pub total_chunks: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

/// Legacy rows without an archive request preserve their existing deadline.
pub fn archive_deadline(requested: Option<i64>, previous: Option<i64>) -> Option<i64> {
    requested
        .map(|at| at.saturating_add(ARCHIVE_RETENTION_MS))
        .map(|at| previous.map_or(at, |prior| at.min(prior)))
        .or(previous)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_archive_round_trip_and_retirement_barrier() {
        let mut value = serde_json::json!({"version":1,"id":"operation","sourceSandboxId":"machine",
            "archivedAtMs":100,"expiresAtMs":3600100,"phase":"verifying","verifiedChunks":0,
            "manifest":{"version":1,"operationId":"operation","workspaceId":"workspace","chatId":"chat",
                "sessionId":"session","chunks":["a".repeat(64)],"archiveSha256":"b".repeat(64)}});
        let mut archive: RecoveryArchive = serde_json::from_value(value.clone()).unwrap();
        assert!(archive.valid_for("workspace", "chat", "session"));
        assert!(!archive.backup_verified());
        assert_eq!(serde_json::to_value(&archive).unwrap(), value);
        archive.phase = ArchivePhase::Retiring;
        assert!(!archive.backup_verified());
        archive.verified_chunks = 1;
        assert!(archive.backup_verified());
        assert!(!archive.valid_for("other", "chat", "session"));
        value["manifest"]["chunks"] = serde_json::json!([]);
        let invalid: RecoveryArchive = serde_json::from_value(value).unwrap();
        assert!(!invalid.valid_for("workspace", "chat", "session"));
        assert!(!invalid.backup_verified());
    }
    #[test]
    fn retention_does_not_extend_or_drop_old_deadlines() {
        assert_eq!(archive_deadline(None, Some(100)), Some(100));
        assert_eq!(
            archive_deadline(Some(100), None),
            Some(100 + ARCHIVE_RETENTION_MS)
        );
        assert_eq!(archive_deadline(Some(100), Some(200)), Some(200));
        assert_eq!(
            archive_deadline(Some(100), Some(100 + 30 * 86400000)),
            Some(100 + ARCHIVE_RETENTION_MS)
        );
        assert_eq!(archive_deadline(None, None), None);
    }
}
