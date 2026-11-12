//! Server-owned task command admission. Version 2 is separately negotiated;
//! an API accepting this shape does not certify an installed guest.
use crate::{
    cloud_assets::Asset,
    session_commands::{CommandBasedOn, SessionCommandPayload},
};
use serde::{Deserialize, Serialize};
pub const VERSION: u32 = 2;
pub const CAPABILITY: &str = "rust-task-command-admission-v2";
pub const MODE: &str = "server-admitted-v2";
/// The computer accepts a run whose files were uploaded straight to it and
/// are referenced as paths in the run's attachments.
pub const DIRECT_UPLOADS_CAPABILITY: &str = "rust-direct-uploads-v1";
/// The computer accepts a run whose access mode differs from the one its task
/// policy was bound with (each message may choose its own access mode).
pub const MESSAGE_ACCESS_MODE_CAPABILITY: &str = "rust-message-access-mode-v1";
/// The computer's guest runtime predates something this message needs; only
/// recreating the computer (onto a current image) updates it.
pub const RUNTIME_OUTDATED: &str = "cloud_computer_runtime_outdated";
pub const MAX_TEXT_BYTES: usize = 128 * 1024;
pub const MAX_QUEUE_ROWS: i64 = 128;
pub const EDIT_LEASE_MS: i64 = 30_000;
/// Prefix of a capability error that means the server definitively refused a
/// command (bad model, access mode, attachment …). The rest is the message
/// to show; the client may build a fresh command instead of retrying it.
pub const TASK_REJECTED_PREFIX: &str = "cloud_task_rejected:";
/// Prefix of a capability error about a message whose delivery outcome is
/// unknown. `review-required`: the server refuses new runs on the
/// conversation until that message is reviewed (retried or discarded).
/// `not-found`: a review found nothing unknown left to review.
pub const TASK_UNKNOWN_OUTCOME_PREFIX: &str = "cloud_task_unknown_outcome:";

/// No implicit resume occurs merely because work was persisted.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WakePolicy {
    #[default]
    PreserveSleep,
    ExplicitResume,
}
/// Immutable agent choices saved with a queued item, not mutable conversation defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSelection {
    pub harness: crate::HarnessId,
    pub model: String,
    pub reasoning: Option<crate::ReasoningLevel>,
    #[serde(default)]
    pub model_options: serde_json::Map<String, serde_json::Value>,
    pub sandbox: crate::SandboxLevel,
}
pub const QUEUE_RUN_SELECTION_VERSION: u32 = 1;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Input {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_selection: Option<RunSelection>,
    pub text: String,
    #[serde(default)]
    pub attachments: Vec<Asset>,
    /// Files already uploaded straight to the computer: their absolute paths
    /// there. The server never sees their bytes; the computer checks each is
    /// one of its own uploads when the queued message runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub computer_files: Vec<String>,
    /// References remain task-relative until the enrolled guest resolves them.
    #[serde(default)]
    pub file_refs: Vec<String>,
    #[serde(default)]
    pub skill_refs: Vec<String>,
    #[serde(default)]
    pub annotations: Vec<String>,
    #[serde(default)]
    pub as_goal: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    /// Client-minted once per logical action; retained across lost responses.
    pub command_id: String,
    pub session_id: String,
    pub action: Action,
    #[serde(default)]
    pub wake: WakePolicy,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    /// Persist an empty conversation before its first command or computer wake.
    #[serde(rename_all = "camelCase")]
    CreateSession { title: Option<String> },
    #[serde(rename_all = "camelCase")]
    Submit {
        message_id: String,
        payload: SessionCommandPayload,
        #[serde(default)]
        attachments: Vec<Asset>,
        #[serde(default)]
        based_on: Option<CommandBasedOn>,
    },
    #[serde(rename_all = "camelCase")]
    Queue {
        queue_id: String,
        message_id: String,
        input: Input,
        #[serde(default)]
        hold_for_turn_end: bool,
    },
    #[serde(rename_all = "camelCase")]
    Edit {
        queue_id: String,
        revision: i64,
        input: Input,
        #[serde(default)]
        lease_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Delete {
        queue_id: String,
        revision: i64,
        #[serde(default)]
        lease_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Move {
        queue_id: String,
        revision: i64,
        to_index: usize,
    },
    #[serde(rename_all = "camelCase")]
    SendNow { queue_id: String, revision: i64 },
    #[serde(rename_all = "camelCase")]
    SteerNow {
        queue_id: String,
        revision: i64,
        #[serde(default)]
        based_on: Option<CommandBasedOn>,
    },
    #[serde(rename_all = "camelCase")]
    PauseQueue { paused: bool },
    #[serde(rename_all = "camelCase")]
    ReviewUnknown { interrupted_command_id: String },
    #[serde(rename_all = "camelCase")]
    BeginEdit {
        queue_id: String,
        revision: i64,
        editor_device_id: String,
        editor_instance_id: String,
    },
    #[serde(rename_all = "camelCase")]
    RenewEdit { queue_id: String, lease_id: String },
    #[serde(rename_all = "camelCase")]
    CancelEdit { queue_id: String, lease_id: String },
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionState {
    Queued,
    Starting,
    Running,
    WaitingInput,
    Completed,
    Cancelled,
    Failed,
    OutcomeUnknown,
}
impl ExecutionState {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Cancelled | Self::Failed | Self::OutcomeUnknown
        )
    }
}
/// A generation-bound proof from the guest, distinct from a mailbox delivery ACK.
/// Sequence is a durable local counter. Equal revision with different content
/// is a conflict; a lost response retries the exact report without redispatch.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Progress {
    pub command_id: String,
    pub fingerprint: String,
    pub storage_incarnation_id: String,
    pub revision: i64,
    pub state: ExecutionState,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub error_code: Option<String>,
}
/// Unresolved paths are private operational binding, not part of the rendered
/// transcript. The original envelope/fingerprint is checked before resolution.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeBinding {
    pub version: u32,
    pub policy_revision: i64,
    /// Server-selected current admitted Run, never a caller-selected future turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt_command_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt_message_id: Option<String>,
    /// Empty or '.' selects the recorded conversation checkout. Other values
    /// must be relative and remain below that checkout after canonicalization.
    pub cwd_relative: String,
    #[serde(default)]
    pub attachments: Vec<Asset>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<RuntimeSession>,
}

/// API-registered conversation identity. This never carries a harness-native
/// session id or a fabricated guest projection cursor.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeSession {
    pub session_id: String,
    pub thread_id: String,
    pub title: Option<String>,
    pub created_at: i64,
}
