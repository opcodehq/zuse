//! Version negotiation for the public application control surface.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const API_VERSION: u32 = 1;
pub const API_INFO: &str = "ControlInfo";
pub const CALL: &str = "ControlCall";
pub const ZAPPER_COMMAND: &str = "ZapperCommand";
pub const WATCH_ZAPPERS: &str = "WatchZappers";
pub const ZAPPER_FILES: &str = "ZapperFiles";
pub const ZAPPER_EVENTS: &str = "ZapperEvents";
/// One issue in full, including activity the coordinator prompt leaves out.
pub const ZAPPER_ISSUE: &str = "ZapperIssue";
pub const ACK_ZAPPER_EVENTS: &str = "AckZapperEvents";
/// Engine-to-engine: place a worker on the device that hosts it.
pub const BIND_ZAPPER_WORKER: &str = "BindZapperWorker";

/// Explicit public surface. Credential-management and raw provider transport stay private.
pub const METHODS: &[&str] = &[
    API_INFO,
    ZAPPER_COMMAND,
    WATCH_ZAPPERS,
    ZAPPER_FILES,
    ZAPPER_EVENTS,
    ZAPPER_ISSUE,
    ACK_ZAPPER_EVENTS,
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
