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
    #[serde(default)]
    pub issues: Vec<ZotIssue>,
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
        /// Atomically link the new thread and mark this issue in progress.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        issue_id: Option<String>,
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
    CreateIssue {
        issue_id: String,
        details: ZotIssueDetails,
    },
    EditIssue {
        issue_id: String,
        details: ZotIssueDetails,
    },
    LinkIssueThread {
        issue_id: String,
        chat_id: String,
    },
    LinkIssuePullRequest {
        issue_id: String,
        url: String,
        title: String,
    },
    UnlinkIssueThread {
        issue_id: String,
        chat_id: String,
    },
    UnlinkIssuePullRequest {
        issue_id: String,
        url: String,
    },
    CommentIssue {
        issue_id: String,
        text: String,
        /// Activity entry this comment answers; shown nested under it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reply_to: Option<String>,
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

/// Work is complete only when explicitly marked done; a finished model turn is
/// evidence for review, never an automatic completion signal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ZotIssueStatus {
    #[default]
    Backlog,
    Todo,
    InProgress,
    Blocked,
    InReview,
    Done,
    Canceled,
}
impl ZotIssueStatus {
    pub const ALL: [Self; 7] = [
        Self::Backlog,
        Self::Todo,
        Self::InProgress,
        Self::Blocked,
        Self::InReview,
        Self::Done,
        Self::Canceled,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Backlog => "Backlog",
            Self::Todo => "Todo",
            Self::InProgress => "In progress",
            Self::Blocked => "Blocked",
            Self::InReview => "In review",
            Self::Done => "Done",
            Self::Canceled => "Canceled",
        }
    }
    pub fn terminal(self) -> bool {
        matches!(self, Self::Done | Self::Canceled)
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ZotIssuePriority {
    Urgent,
    High,
    #[default]
    Normal,
    Low,
}
impl ZotIssuePriority {
    pub const ALL: [Self; 4] = [Self::Urgent, Self::High, Self::Normal, Self::Low];
    pub fn label(self) -> &'static str {
        match self {
            Self::Urgent => "Urgent",
            Self::High => "High",
            Self::Normal => "Normal",
            Self::Low => "Low",
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotIssueDetails {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub status: ZotIssueStatus,
    #[serde(default)]
    pub priority: ZotIssuePriority,
    /// Assigned to the owning Zot. Unassigned issues remain visible in its backlog.
    #[serde(default)]
    pub assigned: bool,
    /// ISO calendar date, or absent.
    pub due_date: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    /// Other issue ids in the same Zot.
    #[serde(default)]
    pub relations: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotIssuePullRequest {
    pub url: String,
    pub title: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotIssueActivity {
    #[serde(default)]
    pub actor: Option<String>,
    pub id: String,
    #[ts(type = "number")]
    pub created_at: i64,
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reply_to: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotIssue {
    pub id: String,
    pub number: u32,
    pub details: ZotIssueDetails,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
    pub threads: Vec<String>,
    pub pull_requests: Vec<ZotIssuePullRequest>,
    pub activity: Vec<ZotIssueActivity>,
}
impl ZotIssue {
    pub fn code(&self) -> String {
        format!("ZOT-{}", self.number)
    }
}
