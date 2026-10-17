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
/// Flip a harness's enablement on the target device (Settings → Providers);
/// replies with the device's fresh `ListHarnesses` catalog.
pub const GET_TITLE_SETTINGS: &str = "GetTitleSettings";
pub const SET_TITLE_SETTINGS: &str = "SetTitleSettings";
pub const SET_HARNESS_ENABLED: &str = "SetHarnessEnabled";
pub const LIST_MODELS: &str = "ListModels";
pub const LIST_SKILLS: &str = "ListSkills";
pub const LIST_COMMANDS: &str = "ListCommands";
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
// WatchWorkspaceFiles is the only streaming method in this group.
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
/// Store an agent's HTML page and show it inline in that chat's live
/// turn (`show_visual`). Local only: the calling agent runs on this device.
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
