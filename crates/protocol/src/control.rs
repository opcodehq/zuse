//! Version negotiation for the public application control surface.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const API_VERSION: u32 = 1;
pub const API_INFO: &str = "ControlInfo";
pub const CALL: &str = "ControlCall";
pub const ZOT_COMMAND: &str = "ZotCommand";
pub const WATCH_ZOTS: &str = "WatchZots";
pub const ZOT_FILES: &str = "ZotFiles";
pub const ZOT_EVENTS: &str = "ZotEvents";
/// One issue in full, including activity the coordinator prompt leaves out.
pub const ZOT_ISSUE: &str = "ZotIssue";
pub const ACK_ZOT_EVENTS: &str = "AckZotEvents";

/// Explicit public surface. Credential-management and raw provider transport stay private.
pub const METHODS: &[&str] = &[
    API_INFO,
    ZOT_COMMAND,
    WATCH_ZOTS,
    ZOT_FILES,
    ZOT_EVENTS,
    ZOT_ISSUE,
    ACK_ZOT_EVENTS,
    "ControlUpdateStatus",
    "RetryControlUpdate",
    "EngineInfo",
    "WatchDevices",
    "WatchSpaces",
    "WatchChats",
    "WatchSessions",
    "Mutate",
    "ListHarnesses",
    "ListModels",
    "ListSkills",
    "ListCommands",
    "QueueCommand",
    "WatchDocMessages",
    "WatchQueue",
    "QueueMessage",
    "UpdateQueuedMessage",
    "RemoveQueuedMessage",
    "SendQueuedMessageNow",
    "SteerQueuedMessageNow",
    "ListRepos",
    "AddRepo",
    "CloneRepo",
    "CreateRepo",
    "ListBranches",
    "ListRefs",
    "ListGitHistory",
    "SearchGitHistory",
    "FetchAll",
    "SwitchRef",
    "CreateWorktree",
    "DeleteWorktree",
    "GetWorktreeSettings",
    "SetWorktreeSettings",
    "ListWorkspaceDirectory",
    "SearchWorkspaceFiles",
    "SearchWorkspaceContent",
    "ReadWorkspaceFile",
    "WriteWorkspaceFile",
    "MoveWorkspaceEntry",
    "DeleteWorkspaceEntry",
    "WatchWorkspaceFiles",
    "OpenTerminal",
    "SubscribeTerminal",
    "WriteTerminal",
    "ResizeTerminal",
    "CloseTerminal",
    "WatchCheckoutDiffs",
    "WatchWorkspaceGitStatus",
    "GetCheckoutDiff",
    "GetCheckoutFileDiffText",
    "WatchCheckoutChangeRequest",
    "ListChangeRequestPage",
    "GetChangeRequest",
    "GetChangeRequestDiff",
    "PostChangeRequestComment",
    "ListProjectActions",
    "UpsertProjectAction",
    "DeleteProjectAction",
    "RunProjectAction",
    "GetTitleSettings",
    "SetTitleSettings",
    "InspectRecovery",
    "ResolveRecovery",
    "UpdateStatus",
    "ApplyUpdate",
];

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ControlInfo {
    pub api_version: u32,
    pub app_version: String,
    pub capabilities: Vec<String>,
    pub methods: Vec<String>,
    #[ts(type = "Record<string, unknown>")]
    pub schemas: serde_json::Value,
}
