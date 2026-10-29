//! Canonical native/browser engine RPC method names.
pub const VOICE_CAPABILITIES_V2: &str = "VoiceCapabilitiesV2";
pub const PREPARE_VOICE_V2: &str = "PrepareVoiceV2";
pub const OWN_VOICE_V2: &str = "OwnVoiceV2";
pub const NEGOTIATE_VOICE_V2: &str = "NegotiateVoiceV2";
pub const CONFIRM_VOICE_MEDIA_V2: &str = "ConfirmVoiceMediaV2";
pub const REPORT_VOICE_MEDIA_V2: &str = "ReportVoiceMediaV2";
pub const STOP_VOICE_V2: &str = "StopVoiceV2";
pub const CANCEL_VOICE_ATTEMPT_V2: &str = "CancelVoiceAttemptV2";
pub const WATCH_PREVIEWS: &str = "WatchPreviews";
pub const LIST_HARNESSES: &str = "ListHarnesses";
pub const CANCEL_INSTALL: &str = "CancelInstall";
pub const INSTALL_HARNESS: &str = "InstallHarness";
/// Read the owning host's saved checkpoint and checkout cleanup phase.
pub const GET_CHAT_ARCHIVE_STATUS: &str = "GetChatArchiveStatus";
pub const GET_CUSTOM_ACP_AGENTS: &str = "GetCustomAcpAgents";
pub const SET_CUSTOM_ACP_AGENT: &str = "SetCustomAcpAgent";
pub const DELETE_CUSTOM_ACP_AGENT: &str = "DeleteCustomAcpAgent";
pub const GET_TITLE_SETTINGS: &str = "GetTitleSettings";
pub const SET_TITLE_SETTINGS: &str = "SetTitleSettings";
/// Flip a harness's enablement on the target device (Settings → Providers);
/// replies with the device's fresh `ListHarnesses` catalog.
pub const SET_HARNESS_ENABLED: &str = "SetHarnessEnabled";
pub const LIST_MODELS: &str = "ListModels";
pub const LIST_SKILLS: &str = "ListSkills";
pub const LIST_COMMANDS: &str = "ListCommands";
/// Transient secret submission; never enqueue or persist its parameters.
pub const REQUEST_PRIVATE_OPERATION: &str = "RequestPrivateOperation";
pub const SUBMIT_PRIVATE_SECRET: &str = "SubmitPrivateSecret";
pub const QUEUE_COMMAND: &str = "QueueCommand";
pub const TAKE_PROJECT_ACTION_SETUP: &str = "TakeProjectActionSetup";
/// Peer-to-peer delivery fallback: the SENDER's engine forwards a queued
/// command entry (client-minted id and all) straight over the device-room
/// link when its chat2 rows can't reach the edge but the host's peer link
/// is alive. The host claims the id in its processed ledger before
/// executing, so the doc row arriving later dedupes to a no-op —
/// exactly-once by construction. Params `{chatId, entry}`.
pub const RELAY_COMMAND: &str = "RelayCommand";
/// User-driven delivery retry for a chat with unadopted queued sends:
/// fresh chat2 socket, host nudge, drain pass, and a new delivery escort
/// per pending command. Params `{chatId}`; IPC-only.
pub const RETRY_DELIVERY: &str = "RetryDelivery";
pub const CREATE_THREAD_SESSION: &str = "CreateThreadSession";
pub const FORK_SIDE_CHAT: &str = "ForkSideChat";
pub const WATCH_DOC_MESSAGES: &str = "WatchDocMessages";
/// Explicit user navigation, `{chatId}`. Prioritizes this device's sync
/// connection; automatic subscriptions and retries must not call it.
pub const FOCUS_CHAT: &str = "FocusChat";
/// Messages typed while the agent was busy, held on the chat doc so every
/// device sees the same queue. `{ chatId }` → `{ items: QueuedMessage[] }`.
pub const WATCH_QUEUE: &str = "WatchQueue";
/// Append to the queue. `{ chatId, text, attachments?, holdForTurnEnd? }` → `{ id }`.
pub const QUEUE_MESSAGE: &str = "QueueMessage";
/// Retype a queued message; empty text deletes it.
/// `{ chatId, id, text }` → `{ changed }`.
pub const UPDATE_QUEUED_MESSAGE: &str = "UpdateQueuedMessage";
/// Acquire a host-authoritative edit lease for one queued row.
pub const BEGIN_QUEUED_MESSAGE_EDIT: &str = "BeginQueuedMessageEdit";
/// Renew an acquired queue edit lease.
pub const RENEW_QUEUED_MESSAGE_EDIT: &str = "RenewQueuedMessageEdit";
/// Commit, cancel, discard, or explicitly release an acquired edit.
pub const FINISH_QUEUED_MESSAGE_EDIT: &str = "FinishQueuedMessageEdit";
/// Reorder. `{ chatId, id, toIndex }` → `{ changed }`.
pub const MOVE_QUEUED_MESSAGE: &str = "MoveQueuedMessage";
pub const REMOVE_QUEUED_MESSAGE: &str = "RemoveQueuedMessage";
/// Interrupt whatever is running and send this one. `{ chatId, id }` → `{ sent }`.
pub const SEND_QUEUED_MESSAGE_NOW: &str = "SendQueuedMessageNow";
/// Steer this row into the live turn without interrupting it.
/// `{ chatId, id }` → `{ sent }`.
pub const STEER_QUEUED_MESSAGE_NOW: &str = "SteerQueuedMessageNow";
/// Nudge every open room client to verify liveness NOW (window focus,
/// app foregrounded). No params; IPC-only. Each room ignores the hint
/// unless it has been broadcast-quiet ≥30s, so this is cheap to spam.
pub const PROBE_SYNC: &str = "ProbeSync";
/// Live sync introspection (`zuse sync` / debug surfaces): per-room
/// connection state, last pushed-frame/ack ages, rejoin/probe/resync
/// counters for the workspace room and every open chat doc. No params;
/// IPC-only.
pub const SYNC_STATUS: &str = "SyncStatus";
/// Pushed edge-connectivity posture (`zuse_protocol::Connectivity`):
/// current value first, then every change — the connection pill /
/// composer-honesty / queued-badge feed. No params; IPC-only.
pub const WATCH_CONNECTIVITY: &str = "WatchConnectivity";
/// In-flight queued-attachment transfers (`zuse_protocol::TransferProgress`
/// list): current set first, then a fresh snapshot per landed chunk —
/// the sending thumbnail's percent-ring feed. No params; IPC-only.
pub const WATCH_TRANSFERS: &str = "WatchTransfers";
pub const WATCH_CHATS: &str = "WatchChats";
pub const WATCH_CHAT_CONFIG: &str = "WatchChatConfig";
pub const WATCH_SIDEBAR_PREFERENCES: &str = "WatchSidebarPreferences";
pub const WATCH_DEVICES: &str = "WatchDevices";
pub const WATCH_SESSIONS: &str = "WatchSessions";
/// Spaces registry (device+folder pairs) from the workspace doc.
pub const WATCH_SPACES: &str = "WatchSpaces";
/// Entity mutations against the workspace doc (feature-inventory §2 DataRpc).
/// Params are tagged `{op: createChat|createSpace|renameSpace|deleteSpace|
/// renameChat|setChatArchived|deleteChat|renameDevice|markChatSeen, …}`.
pub const MUTATE: &str = "Mutate";
/// This engine's identity → `{deviceId}` (IPC-only; never relay-forwarded —
/// the answer is about whichever engine you are directly connected to).
pub const LOCAL_DEVICE: &str = "LocalDevice";
/// This engine runtime's fixed device and workspace identity.
pub const ENGINE_INFO: &str = "EngineInfo";
/// Readiness barrier for the engine runtime. The call completes once stores
/// and journals are assembled, or fails with the assembly error.
pub const ENGINE_READY: &str = "EngineReady";
/// Ask a headless IPC owner to drain its runtime and exit successfully.
/// Headed IPC owners do not implement this method: closing another app's
/// engine behind its windows would leave that process unusable.
pub const STOP_ENGINE: &str = "StopEngine";
pub const AUTH_STATUS: &str = "AuthStatus";
// AuthRpc mutations (feature-inventory §2 AuthRpc; IPC-only).
pub const SIGN_IN: &str = "SignIn";
pub const SIGN_IN_HEADLESS: &str = "SignInHeadless";
pub const COMPLETE_SIGN_IN: &str = "CompleteSignIn";
pub const SIGN_OUT: &str = "SignOut";
pub const LIST_ORGS: &str = "ListOrgs";
pub const CREATE_ORG: &str = "CreateOrg";
pub const SELECT_ORG: &str = "SelectOrg";
/// One-time local→synced profile import: what's importable (unary).
pub const LOCAL_IMPORT_STATUS: &str = "LocalImportStatus";
/// One-time local→synced profile import: run it (stream of progress items).
pub const IMPORT_LOCAL_WORKSPACE: &str = "ImportLocalWorkspace";
// Repos / worktrees / folders (ControlRpc, relay-forwardable).
pub const LIST_REPOS: &str = "ListRepos";
pub const ADD_REPO: &str = "AddRepo";
pub const CLONE_REPO: &str = "CloneRepo";
pub const CREATE_REPO: &str = "CreateRepo";
pub const LIST_BRANCHES: &str = "ListBranches";
pub const LIST_REFS: &str = "ListRefs";
pub const LIST_GIT_HISTORY: &str = "ListGitHistory";
/// A branch's commits beside its base's since they forked, and a merge
/// preview (`branch_graph::BranchGraphRequest`). Read-only.
pub const GET_BRANCH_GRAPH: &str = "GetBranchGraph";
/// Fuzzy commit-subject / SHA search over the complete public history.
pub const SEARCH_GIT_HISTORY: &str = "SearchGitHistory";
/// Resolve hosted profile images for a page of Git commit authors.
pub const RESOLVE_GIT_AVATARS: &str = "ResolveGitAvatars";
/// Update remote-tracking refs without changing HEAD, the index, or files.
pub const FETCH_ALL: &str = "FetchAll";
pub const SWITCH_REF: &str = "SwitchRef";
pub const LIST_FOLDERS: &str = "ListFolders";
pub const GET_WORKTREE_SETTINGS: &str = "GetWorktreeSettings";
pub const SET_WORKTREE_SETTINGS: &str = "SetWorktreeSettings";
/// The device's browse roots: home plus mounted drives/volumes.
pub const LIST_DRIVES: &str = "ListDrives";
/// Folders under the host's home by name (new-project picker).
pub const SEARCH_HOME_FOLDERS: &str = "SearchHomeFolders";
/// The new-project picker closed: drop the host's home index if unpinned.
pub const RELEASE_HOME_FOLDER_SEARCH: &str = "ReleaseHomeFolderSearch";
/// Fuzzy relative-path search rooted in a known chat or space checkout.
pub const SEARCH_FILES: &str = "SearchFiles";
// Device-local workspace filesystem operations. All are relay-forwardable;
// WatchWorkspaceFiles and WorkspaceSyncSnapshot stream their replies.
pub const WORKSPACE_SYNC_SNAPSHOT: &str = "WorkspaceSyncSnapshot";
pub const LIST_WORKSPACE_DIRECTORY: &str = "ListWorkspaceDirectory";
pub const SEARCH_WORKSPACE_FILES: &str = "SearchWorkspaceFiles";
/// Start or keep alive a workspace's search index (fire-and-forget warm).
pub const WARM_WORKSPACE_SEARCH: &str = "WarmWorkspaceSearch";
/// Plain-text search over a workspace's file contents.
pub const SEARCH_WORKSPACE_CONTENT: &str = "SearchWorkspaceContent";
pub const READ_WORKSPACE_IMAGE: &str = "ReadWorkspaceImage";
pub const READ_WORKSPACE_FILE: &str = "ReadWorkspaceFile";
pub const MOVE_WORKSPACE_ENTRY: &str = "MoveWorkspaceEntry";
pub const DELETE_WORKSPACE_ENTRY: &str = "DeleteWorkspaceEntry";
pub const WRITE_WORKSPACE_FILE: &str = "WriteWorkspaceFile";
pub const WATCH_WORKSPACE_FILES: &str = "WatchWorkspaceFiles";
pub const CREATE_WORKTREE: &str = "CreateWorktree";
pub const DELETE_WORKTREE: &str = "DeleteWorktree";
// Project Actions are private state on the device that owns the project.
pub const LIST_PROJECT_ACTIONS: &str = "ListProjectActions";
pub const UPSERT_PROJECT_ACTION: &str = "UpsertProjectAction";
pub const DELETE_PROJECT_ACTION: &str = "DeleteProjectAction";
pub const RUN_PROJECT_ACTION: &str = "RunProjectAction";
// Terminals (ControlRpc, relay-forwardable; SubscribeTerminal streams).
pub const LIST_TERMINALS: &str = "ListTerminals";
pub const OPEN_TERMINAL: &str = "OpenTerminal";
pub const SUBSCRIBE_TERMINAL: &str = "SubscribeTerminal";
pub const WRITE_TERMINAL: &str = "WriteTerminal";
pub const RESIZE_TERMINAL: &str = "ResizeTerminal";
pub const CLOSE_TERMINAL: &str = "CloseTerminal";
/// Checkout-diff stream for the target device's chats (DataRpc,
/// relay-forwardable — diffs are produced where the checkout lives).
pub const WATCH_CHECKOUT_DIFFS: &str = "WatchCheckoutDiffs";
pub const WATCH_WORKSPACE_GIT_STATUS: &str = "WatchWorkspaceGitStatus";
/// Current pull request for one checkout, resolved on the checkout's host device.
pub const WATCH_CHECKOUT_CHANGE_REQUEST: &str = "WatchCheckoutChangeRequest";
/// One page of a repository's open pull requests (`filter`, `after`
/// cursor), with the total count.
pub const LIST_CHANGE_REQUEST_PAGE: &str = "ListChangeRequestPage";
/// Resolve a selected checkout through local Git metadata only.
pub const GET_CHANGE_REQUEST_REPOSITORY: &str = "GetChangeRequestRepository";
pub const GET_CHANGE_REQUEST: &str = "GetChangeRequest";
pub const POST_CHANGE_REQUEST_COMMENT: &str = "PostChangeRequestComment";
/// Merge, toggle auto-merge, or change a pull request's status, on the
/// device whose `gh` acts for the user. `{url, update}` →
/// `{refusal?: string}`: GitHub's reason when it declined.
pub const UPDATE_CHANGE_REQUEST: &str = "UpdateChangeRequest";
pub const GET_CHANGE_REQUEST_DIFF: &str = "GetChangeRequestDiff";
pub const GET_CHECKOUT_DIFF: &str = "GetCheckoutDiff";
/// Permanently restore one chat-owned checkout to its current HEAD and
/// remove only its untracked, non-ignored paths.
pub const DISCARD_WORKING_TREE: &str = "DiscardWorkingTree";
pub const GET_CHECKOUT_FILE_DIFF_TEXT: &str = "GetCheckoutFileDiffText";
// Agent accounts (ControlRpc, relay-forwardable — CLI logins are per-device).
pub const LIST_AGENT_ACCOUNTS: &str = "ListAgentAccounts";
pub const ACTIVATE_AGENT_ACCOUNT: &str = "ActivateAgentAccount";
pub const FORGET_AGENT_ACCOUNT: &str = "ForgetAgentAccount";
pub const START_AGENT_LOGIN: &str = "StartAgentLogin";
pub const COMPLETE_AGENT_LOGIN: &str = "CompleteAgentLogin";
pub const POLL_AGENT_LOGIN: &str = "PollAgentLogin";
pub const CANCEL_AGENT_LOGIN: &str = "CancelAgentLogin";
// Uploads / attachments (ControlRpc, relay-forwardable — target the chat's host device).
pub const UPLOAD_CHUNK: &str = "UploadChunk";
pub const UPLOAD_COMMIT: &str = "UploadCommit";
pub const READ_ATTACHMENT_CHUNK: &str = "ReadAttachmentChunk";
/// A transient host-owned visual subscription; cancellation retires its lease.
pub const OPEN_VISUAL: &str = "OpenVisual";
pub const CONTROL_VISUAL: &str = "ControlVisual";
pub const DRAW_VISUAL: &str = "DrawVisual";
/// Store an agent's sandboxed visual in its live turn (`show_visual`).
/// Local only: the calling agent runs on this device.
pub const PUBLISH_VISUAL: &str = "PublishVisual";
/// Lazy full-tool-output fetch from the R2 sidecar by doc-resident ref
/// (chat2-sync A3). Edge-direct from any device — never relay-forwarded.
pub const FETCH_TOOL_BLOB: &str = "FetchToolBlob";
// Updates (ControlRpc, relay-forwardable — a device reports/applies its own
// binary's update). Stream: current UpdateStatus, then every change.
pub const UPDATE_STATUS: &str = "UpdateStatus";
/// Download + apply the newest release on the target device (symlink-managed
/// installs; the service restart is scheduled after the reply flushes).
pub const APPLY_UPDATE: &str = "ApplyUpdate";
/// Device-local agent CLI update lifecycle. The stream emits the complete
/// ordered status list initially and after every transition.
pub const WATCH_HARNESS_UPDATES: &str = "WatchHarnessUpdates";
pub const CHECK_HARNESS_UPDATES: &str = "CheckHarnessUpdates";
pub const APPLY_HARNESS_UPDATE: &str = "ApplyHarnessUpdate";
pub const CANCEL_HARNESS_UPDATE: &str = "CancelHarnessUpdate";
pub const DISMISS_HARNESS_UPDATE: &str = "DismissHarnessUpdate";
pub const SET_HARNESS_UPDATE_POLICY: &str = "SetHarnessUpdatePolicy";

/// Engine-owned Zuse account and managed Executor gateway.
pub const LIST_ACCOUNT_ROUTES: &str = "ListAccountRoutes";
pub const REMEMBER_ACCOUNT_ROUTE: &str = "RememberAccountRoute";
pub const FORGET_ACCOUNT_ROUTE: &str = "ForgetAccountRoute";
pub const BOOTSTRAP_ACCOUNT_CONNECTION: &str = "BootstrapAccountConnection";
pub const LIST_ACCOUNT_ENVIRONMENTS: &str = "ListAccountEnvironments";
pub const MANAGED_HOST_STATUS: &str = "ManagedHostStatus";
pub const WATCH_MANAGED_HOST: &str = "WatchManagedHost";
pub const LINK_MANAGED_HOST: &str = "LinkManagedHost";
pub const UNLINK_MANAGED_HOST: &str = "UnlinkManagedHost";
pub const LIST_ACCOUNT_DEVICES: &str = "ListAccountDevices";
pub const REVOKE_ACCOUNT_DEVICE: &str = "RevokeAccountDevice";
/// Host-authorized cloud catalog and saved history; neither wakes compute.
pub const CLOUD_CONTEXT: &str = "CloudContext";
pub const CLOUD_CREATE: &str = "CloudCreate";
pub const CLOUD_ENVIRONMENT: &str = "CloudEnvironment";
pub const CLOUD_RPC: &str = "CloudRpc";
pub const CLOUD_WORKSPACES: &str = "CloudWorkspaces";
pub const CLOUD_HISTORY: &str = "CloudHistory";
/// Explicit lifecycle mutation. Never automatically retry on reconnect.
pub const CLOUD_CLEANUP_MIRROR: &str = "CloudCleanupMirror";
pub const CLOUD_DELETE: &str = "CloudDelete";
pub const CLOUD_UNARCHIVE: &str = "CloudUnarchive";
pub const CLOUD_LOCAL_TERMINAL: &str = "CloudLocalTerminal";
pub const CLOUD_SYNC_STATUS: &str = "CloudSyncStatus";
pub const CLOUD_SYNC_CONFIGURE: &str = "CloudSyncConfigure";
pub const CLOUD_SYNC_RETRY: &str = "CloudSyncRetry";
pub const CLOUD_PAUSE: &str = "CloudPause";
pub const CLOUD_WORKSPACE: &str = "CloudWorkspace";
pub const CLOUD_ACCOUNT_STATUS: &str = "CloudAccountStatus";
pub const CLOUD_ACCOUNT_SIGN_IN: &str = "CloudAccountSignIn";
pub const CLOUD_ACCOUNT_SIGN_OUT: &str = "CloudAccountSignOut";
pub const PLUGINS: &str = "Plugins";
pub const PLUGIN_TOOLS: &str = "PluginTools";

/// Public model connection metadata and sign-in controls; no credential read RPC.
pub const MODEL_CONNECTIONS: &str = "ModelConnections";
pub const MODEL_CONNECT: &str = "ModelConnect";
pub const MODEL_RENAME: &str = "ModelConnectionRename";
pub const MODEL_PREFERRED: &str = "ModelConnectionPreferred";
pub const MODEL_DISCONNECT: &str = "ModelConnectionDisconnect";
pub const MODEL_ACKNOWLEDGE_PLAN: &str = "ModelConnectionAcknowledgePlan";
pub const MODEL_CANCEL_LOGIN: &str = "ModelConnectionCancelLogin";

pub const MODEL_COMPLETE_LOGIN: &str = "ModelConnectionCompleteLogin";

/// Authenticated user controls; these methods must never be model tools.
pub const INSPECT_RECOVERY: &str = "InspectRecovery";
pub const RESOLVE_RECOVERY: &str = "ResolveRecovery";

/// Only explicit read operations may be retried after an uncertain disconnect.
/// Unknown methods and all actions keep their original delivery uncertainty.
pub fn retryable_read(method: &str, params: &serde_json::Value) -> bool {
    match method {
        GET_GIT_WORKSPACE
        | GET_BRANCH_GRAPH
        | RELAY_ACCESS_STATUS
        | BROWSER_ACCESS_STATUS
        | LIST_PAIRINGS
        | LIST_ACCOUNT_ROUTES
        | LIST_ACCOUNT_ENVIRONMENTS
        | MANAGED_HOST_STATUS
        | WATCH_MANAGED_HOST
        | LIST_ACCOUNT_DEVICES
        | CLOUD_ACCOUNT_STATUS
        | CLOUD_CONTEXT
        | SLACK_CONNECTIONS
        | CLOUD_PROJECTS
        | CLOUD_GITHUB
        | CLOUD_PREPAID_BALANCE
        | CLOUD_BILLING_ENTITLEMENTS
        | CLOUD_BILLING_SUMMARY
        | CLOUD_BILLING_USAGE
        | CLOUD_WORKSPACES
        | CLOUD_HISTORY
        | CLOUD_WORKSPACE
        | CLOUD_SYNC_STATUS
        | AUTH_STATUS
        | LOCAL_DEVICE
        | LIST_HARNESSES
        | WATCH_CHATS
        | WATCH_CHAT_CONFIG
        | WATCH_SESSIONS
        | WATCH_SPACES
        | LIST_REPOS
        | MODEL_CONNECTIONS
        | LIST_MODELS
        | LIST_AGENT_ACCOUNTS
        | WATCH_DOC_MESSAGES
        | WATCH_QUEUE
        | LIST_TERMINALS
        | WORKSPACE_SYNC_SNAPSHOT
        | LIST_WORKSPACE_DIRECTORY
        | READ_WORKSPACE_FILE
        | READ_ATTACHMENT_CHUNK
        | GET_CHECKOUT_DIFF
        | GET_CHECKOUT_FILE_DIFF_TEXT
        | FETCH_TOOL_BLOB
        | SUBSCRIBE_TERMINAL
        | INSPECT_RECOVERY
        | GET_CUSTOM_ACP_AGENTS
        | crate::custom_acp::catalog::GET
        | crate::custom_acp::catalog::ICON
        | GET_CHAT_ARCHIVE_STATUS
        | LIST_REFS
        | LIST_FOLDERS
        | crate::project_creation::LIST
        | crate::project_creation::WATCH
        | crate::project_creation::DEFAULT_PARENT
        | crate::project_creation::GITHUB_STATUS
        | crate::project_creation::GITHUB_REPOS
        | SEARCH_FILES => true,
        PLUGINS => params["action"] == "list",
        _ => false,
    }
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    #[test]
    fn privileged_actions_and_unknown_methods_keep_uncertain_delivery() {
        for method in [
            CREATE_PAIRING,
            REVOKE_PAIRING,
            START_RELAY_ACCESS,
            RESUME_RELAY_ACCESS,
            SET_RELAY_AUTO_START,
            STOP_RELAY_ACCESS,
            START_BROWSER_ACCESS,
            STOP_BROWSER_ACCESS,
            SELECT_PLUGIN_WORKSPACE,
            GIT_WORKSPACE_ACTION,
            QUEUE_COMMAND,
            WRITE_WORKSPACE_FILE,
            SWITCH_REF,
            PLUGINS,
            MODEL_CONNECT,
            CLOUD_UNARCHIVE,
            CLOUD_PAUSE,
            CLOUD_LOCAL_TERMINAL,
            CLOUD_SYNC_CONFIGURE,
            CLOUD_SYNC_RETRY,
            CLOUD_DELETE,
            CLOUD_CLEANUP_MIRROR,
            CLOUD_CREATE,
            CLOUD_PROJECT_CONNECT,
            CLOUD_PROJECT_REMOVE,
            CLOUD_PREPAID_CHECKOUT,
            CLOUD_BILLING_CHECKOUT,
            CLOUD_BILLING_CAP,
            CLOUD_BILLING_PORTAL,
            CLOUD_ENVIRONMENT,
            CLOUD_RPC,
            "FutureMethod",
        ] {
            assert!(!retryable_read(method, &serde_json::json!({})));
        }
        assert!(retryable_read(CLOUD_CONTEXT, &serde_json::Value::Null));
        assert!(retryable_read(CLOUD_WORKSPACES, &serde_json::Value::Null));
        assert!(retryable_read(CLOUD_HISTORY, &serde_json::Value::Null));
        assert!(retryable_read(AUTH_STATUS, &serde_json::Value::Null));
        assert!(retryable_read(LIST_PAIRINGS, &serde_json::Value::Null));
        assert!(retryable_read(
            BROWSER_ACCESS_STATUS,
            &serde_json::Value::Null
        ));
        assert!(retryable_read(LIST_FOLDERS, &serde_json::Value::Null));
        assert!(retryable_read(
            PLUGINS,
            &serde_json::json!({"action":"list"})
        ));
        assert!(!retryable_read(
            PLUGINS,
            &serde_json::json!({"action":"call"})
        ));
    }
}

/// Profile-owned pairing grants; only creation returns a private invitation.
pub const LIST_PAIRINGS: &str = "ListPairings";
pub const CREATE_PAIRING: &str = "CreatePairing";
pub const REVOKE_PAIRING: &str = "RevokePairing";

/// Engine-owned browser listener controls. Status never includes an access token.
pub const BROWSER_ACCESS_STATUS: &str = "BrowserAccessStatus";
pub const START_BROWSER_ACCESS: &str = "StartBrowserAccess";
pub const STOP_BROWSER_ACCESS: &str = "StopBrowserAccess";

/// Profile-owned outbound relay; routing credentials never appear in status.
pub const RELAY_ACCESS_STATUS: &str = "RelayAccessStatus";
/// Resume the saved profile-local relay configuration; never automatically retry.
pub const RESUME_RELAY_ACCESS: &str = "ResumeRelayAccess";
/// Persist the next-launch preference; `{enabled}`. Does not start/stop a connector.
pub const SET_RELAY_AUTO_START: &str = "SetRelayAutoStart";
pub const START_RELAY_ACCESS: &str = "StartRelayAccess";
pub const STOP_RELAY_ACCESS: &str = "StopRelayAccess";

/// Explicit user selection for new agent plugin sessions; never retried as a read.
pub const SELECT_PLUGIN_WORKSPACE: &str = "SelectPluginWorkspace";

/// User-owned Git workflow; only the workspace read may reconnect automatically.
pub const GET_GIT_WORKSPACE: &str = "GetGitWorkspace";
pub const GIT_WORKSPACE_ACTION: &str = "GitWorkspaceAction";

/// Account-scoped cloud administration; handled by the host authority.
pub const CLOUD_BILLING_SUMMARY: &str = "CloudBillingSummary";
pub const CLOUD_BILLING_USAGE: &str = "CloudBillingUsage";
pub const CLOUD_PREPAID_BALANCE: &str = "CloudPrepaidBalance";
pub const CLOUD_PREPAID_CHECKOUT: &str = "CloudPrepaidCheckout";
pub const CLOUD_BILLING_ENTITLEMENTS: &str = "CloudBillingEntitlements";
pub const CLOUD_BILLING_CHECKOUT: &str = "CloudBillingCheckout";
pub const CLOUD_BILLING_PORTAL: &str = "CloudBillingPortal";
pub const CLOUD_BILLING_CAP: &str = "CloudBillingSetCap";

pub const CLOUD_PROJECTS: &str = "CloudProjects";
pub const CLOUD_PROJECT_CONNECT: &str = "CloudProjectConnect";
pub const CLOUD_PROJECT_REMOVE: &str = "CloudProjectRemove";
pub const CLOUD_GITHUB: &str = "CloudGithubCatalog";
pub const CLOUD_IMAGE: &str = "CloudImage";
pub const CLOUD_IMAGE_BUILD: &str = "CloudImageBuild";
pub const CLOUD_IMAGE_DELETE: &str = "CloudImageDelete";

pub const SLACK_CONNECTIONS: &str = "SlackConnections";
pub const SLACK_CONNECT: &str = "SlackConnect";
pub const SLACK_DISCONNECT: &str = "SlackDisconnect";
