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
    /// The user's own picture, shown instead of the mascot: a small square
    /// PNG or JPEG as a `data:` URL (see [`AVATAR_IMAGE_MAX_LEN`]). It lives
    /// beside `avatar` so the mascot stays the fallback for clients, Slack
    /// personas and prompt envelopes that never carry image bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub avatar_image: Option<String>,
}

/// Longest accepted `avatar_image`, in bytes of the `data:` URL. Clients crop
/// to a 128px square first, which lands well under this.
pub const AVATAR_IMAGE_MAX_LEN: usize = 96 * 1024;

/// Whether `value` is an avatar image this version knows how to store.
pub fn avatar_image_ok(value: &str) -> bool {
    value.len() <= AVATAR_IMAGE_MAX_LEN
        && ["data:image/png;base64,", "data:image/jpeg;base64,"]
            .iter()
            .any(|prefix| {
                value.strip_prefix(prefix).is_some_and(|data| {
                    !data.is_empty()
                        && data
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='))
                })
            })
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
    /// Set while the thread waits for a free slot (the Zapper already runs
    /// `maxConcurrentWorkers` threads): its place in the start order, 1 being
    /// the next to start. The engine derives it when it reports the Zapper and
    /// ignores it on commands; absent means the thread was started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub queue_position: Option<u32>,
}
impl ZotWorker {
    pub fn queued(&self) -> bool {
        self.queue_position.is_some()
    }
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
#[derive(Debug, Clone, Serialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZotCommand {
    pub command_id: String,
    pub zot_id: String,
    /// Optional. When given, the command fails only if what it depends on
    /// changed after that revision (see [`ZotAction::conflict_scope`]).
    pub expected_revision: Option<u32>,
    /// The action object, or the same object as a JSON-encoded string for
    /// clients that struggle to escape large nested JSON.
    #[schemars(schema_with = "action_or_json_string_schema")]
    pub action: ZotAction,
}

impl<'de> Deserialize<'de> for ZotCommand {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Raw {
            command_id: String,
            zot_id: String,
            #[serde(default)]
            expected_revision: Option<u32>,
            action: serde_json::Value,
        }
        let raw = Raw::deserialize(deserializer)?;
        let action = match raw.action {
            serde_json::Value::String(text) => serde_json::from_str(&text),
            value => serde_json::from_value(value),
        }
        .map_err(D::Error::custom)?;
        Ok(Self {
            command_id: raw.command_id,
            zot_id: raw.zot_id,
            expected_revision: raw.expected_revision,
            action,
        })
    }
}

fn action_or_json_string_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let action = generator.subschema_for::<ZotAction>();
    schemars::json_schema!({
        "anyOf": [
            action,
            {
                "type": "string",
                "description": "The same action object, JSON-encoded as a string."
            }
        ]
    })
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
    /// Hand an issue to the user with one question: they own it until they
    /// answer, and their answer hands it back.
    AskUser {
        issue_id: String,
        question: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        recommendation: Option<String>,
    },
    /// Close an issue: merged needs a linked pull request, and either way
    /// the note says what happened.
    CloseIssue {
        issue_id: String,
        outcome: ZotIssueOutcome,
        note: String,
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

/// Whether a failed command may still have been applied, because the request
/// or its answer was lost in transit. Only then should a retry reuse the
/// command ID, so the engine's receipt makes it idempotent. Any other failure
/// is a definite rejection (e.g. a revision conflict): nothing was written,
/// and the next attempt is a new command at the current revision.
pub fn command_outcome_unknown(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "transport",
        "connection closed",
        "host unavailable",
        "timed out",
        "timeout",
        "disconnected",
    ]
    .iter()
    .any(|signal| error.contains(signal))
}

/// Canonical command schema exposed by both application discovery and MCP.
pub fn command_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(ZotCommand)).expect("command schema serializes")
}

/// What a command's `expectedRevision` protects. Most commands touch one
/// issue, decision or file, so state that changed elsewhere since the caller
/// read it (a thread's report, another issue's comment) must not reject them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictScope {
    /// Order against every other change matters: any newer revision conflicts.
    Whole,
    /// Conflicts only when one of these targets changed after the expected
    /// revision. Empty means the action commutes with everything (append-only
    /// changes, or ones guarded by their own check such as a file version).
    Targets(Vec<String>),
}

/// Revision-check target for an issue's details.
pub fn issue_target(id: &str) -> String {
    format!("issue:{id}")
}
/// Revision-check target for a decision.
pub fn decision_target(id: &str) -> String {
    format!("decision:{id}")
}

impl ZotAction {
    /// The wire name of this action's `type`.
    pub fn kind(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v["type"].as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// Deliberately exhaustive: a new action must say what it conflicts with.
    pub fn conflict_scope(&self) -> ConflictScope {
        use ConflictScope::*;
        match self {
            Self::Create { .. }
            | Self::Configure { .. }
            | Self::SetStatus { .. }
            | Self::FreshConversation
            | Self::Delete => Whole,
            // Replacing or moving an issue depends on what the caller read of it.
            Self::EditIssue { issue_id, .. }
            | Self::AskUser { issue_id, .. }
            | Self::CloseIssue { issue_id, .. }
            | Self::Delegate {
                issue_id: Some(issue_id),
                ..
            } => Targets(vec![issue_target(issue_id)]),
            Self::Decide { decision } => Targets(vec![decision_target(&decision.id)]),
            // Append-only, set-like or idempotent: the engine validates them
            // against the current state, and files carry their own version.
            Self::CreateIssue { .. }
            | Self::CommentIssue { .. }
            | Self::LinkIssueThread { .. }
            | Self::LinkIssuePullRequest { .. }
            | Self::UnlinkIssueThread { .. }
            | Self::UnlinkIssuePullRequest { .. }
            | Self::Delegate { issue_id: None, .. }
            | Self::AdoptWorker { .. }
            | Self::MessageWorker { .. }
            | Self::StopWorker { .. }
            | Self::ArchiveWorker { .. }
            | Self::ReleaseWorker { .. }
            | Self::WriteFile { .. }
            | Self::RestoreFile { .. } => Targets(vec![]),
        }
    }
}

/// One action as the command schema defines it: its `type` and top-level
/// fields, each marked required or optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionShape {
    pub kind: String,
    pub fields: Vec<(String, bool)>,
}

impl ActionShape {
    /// `createIssue {issueId, details}`; optional fields end in `?`.
    pub fn signature(&self) -> String {
        if self.fields.is_empty() {
            return self.kind.clone();
        }
        let fields: Vec<String> = self
            .fields
            .iter()
            .map(|(name, required)| {
                if *required {
                    name.clone()
                } else {
                    format!("{name}?")
                }
            })
            .collect();
        format!("{} {{{}}}", self.kind, fields.join(", "))
    }
}

/// Every action, read from the generated schema so tool descriptions and
/// error hints can't drift from what the engine accepts.
pub fn action_shapes() -> Vec<ActionShape> {
    let schema =
        serde_json::to_value(schemars::schema_for!(ZotAction)).expect("action schema serializes");
    let variants = schema["oneOf"]
        .as_array()
        .or_else(|| schema["anyOf"].as_array())
        .cloned()
        .unwrap_or_default();
    variants
        .iter()
        .filter_map(|variant| {
            let kind = variant["properties"]["type"]["const"]
                .as_str()
                .or_else(|| variant["properties"]["type"]["enum"][0].as_str())?
                .to_owned();
            let required: Vec<&str> = variant["required"]
                .as_array()
                .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            // Required fields keep their declaration order; optional ones follow.
            let mut fields: Vec<(String, bool)> = required
                .iter()
                .filter(|k| **k != "type")
                .map(|k| (k.to_string(), true))
                .collect();
            if let Some(props) = variant["properties"].as_object() {
                fields.extend(
                    props
                        .keys()
                        .filter(|k| *k != "type" && !required.contains(&k.as_str()))
                        .map(|k| (k.clone(), false)),
                );
            }
            Some(ActionShape { kind, fields })
        })
        .collect()
}

/// `createIssue {issueId, details}, commentIssue {issueId, text, replyTo?}, …`
pub fn action_catalog() -> String {
    action_shapes()
        .iter()
        .map(ActionShape::signature)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Parse a command, turning serde's bare "missing field" into a message that
/// names the action's full shape.
pub fn parse_command(value: serde_json::Value) -> Result<ZotCommand, String> {
    serde_json::from_value::<ZotCommand>(value.clone()).map_err(|error| {
        let action = match &value["action"] {
            serde_json::Value::String(text) => serde_json::from_str(text).unwrap_or_default(),
            other => other.clone(),
        };
        let shapes = action_shapes();
        match action["type"].as_str() {
            Some(kind) => match shapes.iter().find(|s| s.kind == kind) {
                Some(shape) => format!(
                    "{error}. {} takes {{type: \"{kind}\"{}}}; put its fields directly in action",
                    shape.kind,
                    shape
                        .signature()
                        .strip_prefix(kind)
                        .map(|f| f.trim().trim_start_matches('{').trim_end_matches('}'))
                        .filter(|f| !f.is_empty())
                        .map(|f| format!(", {f}"))
                        .unwrap_or_default()
                ),
                None => format!(
                    "{error}. Unknown action type \"{kind}\"; actions: {}",
                    action_catalog()
                ),
            },
            None => format!(
                "{error}. A command is {{commandId, zotId, expectedRevision?, action: {{type, ...fields}}}}"
            ),
        }
    })
}

fn issue_summary(issue: &ZotIssue) -> serde_json::Value {
    serde_json::json!({
        "id": issue.id,
        "code": issue.code(),
        "title": issue.details.title,
        "status": issue.details.status,
        "owner": issue.details.current_owner(),
        "parent": issue.details.parent,
        "updatedAt": issue.updated_at,
        "threads": issue.threads,
        "pullRequests": issue.pull_requests,
        "activityCount": issue.activity.len(),
        "lastActivity": issue.activity.last().map(|a| serde_json::json!({
            "id": a.id,
            "kind": a.kind,
        })),
    })
}

/// A short result for agents: the new revision, what changed and one line
/// saying so. The full state stays available to callers that ask for it.
pub fn command_summary(command: &ZotCommand, result: Option<&Zot>) -> serde_json::Value {
    let kind = command.action.kind();
    let Some(zot) = result else {
        return serde_json::json!({
            "ok": true,
            "action": kind,
            "summary": "Zapper deleted",
        });
    };
    let issue = |id: &str| zot.issues.iter().find(|i| i.id == id);
    let worker = |id: &str| zot.workers.iter().find(|w| w.chat_id == id);
    let mut out = serde_json::json!({
        "ok": true,
        "action": kind,
        "revision": zot.revision,
    });
    let summary = match &command.action {
        ZotAction::CreateIssue { issue_id, .. }
        | ZotAction::EditIssue { issue_id, .. }
        | ZotAction::LinkIssueThread { issue_id, .. }
        | ZotAction::LinkIssuePullRequest { issue_id, .. }
        | ZotAction::UnlinkIssueThread { issue_id, .. }
        | ZotAction::UnlinkIssuePullRequest { issue_id, .. }
        | ZotAction::CommentIssue { issue_id, .. }
        | ZotAction::AskUser { issue_id, .. }
        | ZotAction::CloseIssue { issue_id, .. } => match issue(issue_id) {
            Some(i) => {
                out["issue"] = issue_summary(i);
                format!(
                    "{kind} on {} \"{}\": now {}",
                    i.code(),
                    i.details.title,
                    i.details.status.label()
                )
            }
            None => format!("{kind} applied"),
        },
        ZotAction::Delegate {
            issue_id,
            worker: w,
            ..
        } => {
            if let Some(i) = issue_id.as_deref().and_then(issue) {
                out["issue"] = issue_summary(i);
            }
            out["worker"] = serde_json::json!(worker(&w.chat_id));
            // Whether it runs now or queues behind maxConcurrentWorkers is
            // decided when the engine delivers it; the worker reports it.
            match issue_id.as_deref().and_then(issue) {
                Some(i) => format!(
                    "Delegated thread {} on {}; it starts when a slot is free",
                    w.chat_id,
                    i.code()
                ),
                None => format!(
                    "Delegated thread {}; it starts when a slot is free",
                    w.chat_id
                ),
            }
        }
        ZotAction::AdoptWorker { worker: w } => {
            out["worker"] = serde_json::json!(worker(&w.chat_id));
            format!("Adopted thread {}", w.chat_id)
        }
        ZotAction::MessageWorker { chat_id, .. } => {
            out["worker"] = serde_json::json!(worker(chat_id));
            format!("Message queued for {chat_id}")
        }
        ZotAction::StopWorker { chat_id } => {
            out["worker"] = serde_json::json!(worker(chat_id));
            format!("Stopping {chat_id}")
        }
        ZotAction::ArchiveWorker { chat_id } => {
            out["worker"] = serde_json::json!(worker(chat_id));
            format!("Archiving {chat_id}")
        }
        ZotAction::ReleaseWorker { chat_id } => format!("Released {chat_id}"),
        ZotAction::Decide { decision } => {
            out["decision"] = serde_json::json!(zot.decisions.iter().find(|d| d.id == decision.id));
            format!("Recorded decision {}", decision.id)
        }
        ZotAction::WriteFile {
            path,
            expected_version,
            ..
        }
        | ZotAction::RestoreFile {
            path,
            expected_version,
            ..
        } => {
            let version = expected_version + 1;
            out["file"] = serde_json::json!({"path": path, "version": version});
            format!("Wrote {path} at version {version}")
        }
        ZotAction::Create { .. }
        | ZotAction::Configure { .. }
        | ZotAction::SetStatus { .. }
        | ZotAction::FreshConversation => {
            out["zot"] = serde_json::json!({
                "name": zot.config.name,
                "status": zot.status,
                "coordinatorChatId": zot.coordinator_chat_id,
            });
            format!("{kind} applied")
        }
        ZotAction::Delete => "Zapper deleted".into(),
    };
    out["summary"] = serde_json::json!(summary);
    out
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
    /// Waiting on the user: they own the issue until they answer.
    NeedsYou,
    Blocked,
    InReview,
    Done,
    Canceled,
}
impl ZotIssueStatus {
    pub const ALL: [Self; 8] = [
        Self::Backlog,
        Self::Todo,
        Self::InProgress,
        Self::NeedsYou,
        Self::Blocked,
        Self::InReview,
        Self::Done,
        Self::Canceled,
    ];
    /// Board order: what needs the user first, then live work, then queued,
    /// then finished.
    pub const BOARD: [Self; 8] = [
        Self::NeedsYou,
        Self::InProgress,
        Self::InReview,
        Self::Blocked,
        Self::Todo,
        Self::Backlog,
        Self::Done,
        Self::Canceled,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Backlog => "Backlog",
            Self::Todo => "Todo",
            Self::InProgress => "In progress",
            Self::NeedsYou => "Needs you",
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
    /// The Zapper (or one of its threads) is responsible. Kept in step with
    /// [`ZotIssueDetails::owner`] for clients that predate owners.
    #[serde(default)]
    pub assigned: bool,
    /// ISO calendar date, or absent.
    pub due_date: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    /// Other issue ids in the same Zot.
    #[serde(default)]
    pub relations: Vec<String>,
    /// Who holds the issue now: exactly one owner at a time. Absent on issues
    /// saved before owners, where `assigned` means the Zapper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub owner: Option<ZotIssueOwner>,
    /// What must be true for the issue to be done.
    #[serde(default)]
    pub done_criteria: String,
    /// What this issue deliberately leaves alone.
    #[serde(default)]
    pub out_of_scope: String,
    /// The request-level issue this one is a shippable part of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent: Option<String>,
}
impl ZotIssueDetails {
    /// The current owner, reading issues saved before owners existed.
    pub fn current_owner(&self) -> Option<ZotIssueOwner> {
        self.owner
            .clone()
            .or_else(|| self.assigned.then_some(ZotIssueOwner::Zapper))
    }
    /// Keep `assigned` and the status in step with the owner: the user owning
    /// an open issue means it needs them, and an issue that no longer needs
    /// them goes back to work.
    pub fn normalize(&mut self, has_thread: bool) {
        let owner = self.current_owner();
        self.assigned = matches!(
            owner,
            Some(ZotIssueOwner::Zapper | ZotIssueOwner::Thread { .. })
        );
        if self.status.terminal() {
            return;
        }
        match owner {
            Some(ZotIssueOwner::User) => self.status = ZotIssueStatus::NeedsYou,
            _ if self.status == ZotIssueStatus::NeedsYou => {
                self.status = if has_thread {
                    ZotIssueStatus::InProgress
                } else {
                    ZotIssueStatus::Todo
                }
            }
            _ => {}
        }
    }
}
/// The single owner of an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ZotIssueOwner {
    /// The Zapper itself, while it plans or decides.
    Zapper,
    /// The person the Zapper works for.
    User,
    /// One of the Zapper's threads, while it builds.
    Thread { chat_id: String },
}
/// How an issue closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ZotIssueOutcome {
    /// Its pull request merged.
    Merged,
    /// Deliberately not done.
    Dropped,
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
        format!("ZAP-{}", self.number)
    }
}
impl Zot {
    /// The queued thread that owns `issue`, if any. Such an issue keeps the
    /// `inProgress` status it was delegated with (the work is committed to a
    /// thread and starts on its own), but every surface shows it as Queued
    /// until the thread starts, so nothing claims a thread is running.
    pub fn issue_queued_worker(&self, issue: &ZotIssue) -> Option<&ZotWorker> {
        let ZotIssueOwner::Thread { chat_id } = issue.details.owner.as_ref()? else {
            return None;
        };
        self.workers
            .iter()
            .find(|w| &w.chat_id == chat_id && w.queued())
    }
    /// The status label an issue shows: Queued while its thread waits.
    pub fn issue_status_label(&self, issue: &ZotIssue) -> &'static str {
        if self.issue_queued_worker(issue).is_some() {
            "Queued"
        } else {
            issue.details.status.label()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn action_may_arrive_as_a_json_string() {
        let action =
            json!({"type": "commentIssue", "issueId": "fix", "text": "a \"quoted\"\nline"});
        let as_object: ZotCommand = parse_command(json!({
            "commandId": "c1", "zotId": "z", "action": action
        }))
        .unwrap();
        let as_string: ZotCommand = parse_command(json!({
            "commandId": "c1", "zotId": "z", "expectedRevision": 3,
            "action": action.to_string()
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(&as_object.action).unwrap(),
            serde_json::to_value(&as_string.action).unwrap()
        );
        assert_eq!(as_string.expected_revision, Some(3));
        // Serialization stays an object, so receipts compare the same either way.
        assert_eq!(
            serde_json::to_value(&as_string).unwrap()["action"]["type"],
            "commentIssue"
        );
        let schema = command_schema();
        let any_of = schema["properties"]["action"]["anyOf"].as_array().unwrap();
        assert!(any_of.iter().any(|s| s["type"] == "string"));
    }

    #[test]
    fn every_action_variant_appears_in_the_catalog_with_its_required_fields() {
        let shapes = action_shapes();
        // One sample per variant; adding a variant without a sample fails the
        // exhaustive match below, and the catalog check covers its fields.
        let samples: Vec<ZotAction> = vec![
            ZotAction::FreshConversation,
            ZotAction::Delete,
            ZotAction::StopWorker {
                chat_id: "w".into(),
            },
        ];
        for sample in &samples {
            match sample {
                ZotAction::Create { .. }
                | ZotAction::Configure { .. }
                | ZotAction::SetStatus { .. }
                | ZotAction::AdoptWorker { .. }
                | ZotAction::Delegate { .. }
                | ZotAction::MessageWorker { .. }
                | ZotAction::StopWorker { .. }
                | ZotAction::ArchiveWorker { .. }
                | ZotAction::ReleaseWorker { .. }
                | ZotAction::Decide { .. }
                | ZotAction::WriteFile { .. }
                | ZotAction::RestoreFile { .. }
                | ZotAction::CreateIssue { .. }
                | ZotAction::EditIssue { .. }
                | ZotAction::LinkIssueThread { .. }
                | ZotAction::LinkIssuePullRequest { .. }
                | ZotAction::UnlinkIssueThread { .. }
                | ZotAction::UnlinkIssuePullRequest { .. }
                | ZotAction::CommentIssue { .. }
                | ZotAction::AskUser { .. }
                | ZotAction::CloseIssue { .. }
                | ZotAction::FreshConversation
                | ZotAction::Delete => {}
            }
            assert!(shapes.iter().any(|s| s.kind == sample.kind()));
        }
        let schema = serde_json::to_value(schemars::schema_for!(ZotAction)).unwrap();
        let variants = schema["oneOf"].as_array().unwrap();
        assert_eq!(shapes.len(), variants.len());
        let catalog = action_catalog();
        for shape in &shapes {
            assert!(catalog.contains(&shape.signature()), "{}", shape.kind);
        }
        let create = shapes.iter().find(|s| s.kind == "createIssue").unwrap();
        assert_eq!(
            create.signature(),
            "createIssue {issueId, details}",
            "required fields are listed without ?"
        );
        let comment = shapes.iter().find(|s| s.kind == "commentIssue").unwrap();
        assert!(comment.signature().contains("replyTo?"));
        let link = shapes
            .iter()
            .find(|s| s.kind == "linkIssuePullRequest")
            .unwrap();
        assert!(link.fields.contains(&("title".into(), true)));
    }

    #[test]
    fn parse_errors_name_the_action_shape() {
        let error = parse_command(json!({
            "commandId": "c", "zotId": "z",
            "action": {"type": "createIssue", "issue": {"title": "x"}}
        }))
        .unwrap_err();
        assert!(error.contains("missing field"), "{error}");
        assert!(error.contains("createIssue takes"), "{error}");
        assert!(error.contains("issueId, details"), "{error}");
        let error = parse_command(json!({
            "commandId": "c", "zotId": "z",
            "action": {"type": "linkIssuePullRequest", "issueId": "i", "url": "u"}
        }))
        .unwrap_err();
        assert!(error.contains("title"), "{error}");
        let error = parse_command(json!({
            "commandId": "c", "zotId": "z", "action": {"type": "nope"}
        }))
        .unwrap_err();
        assert!(error.contains("Unknown action type"), "{error}");
    }

    #[test]
    fn conflict_scopes_keep_ordered_actions_strict() {
        assert_eq!(
            ZotAction::FreshConversation.conflict_scope(),
            ConflictScope::Whole
        );
        assert_eq!(
            ZotAction::CommentIssue {
                issue_id: "a".into(),
                text: "t".into(),
                reply_to: None
            }
            .conflict_scope(),
            ConflictScope::Targets(vec![])
        );
        assert_eq!(
            ZotAction::EditIssue {
                issue_id: "a".into(),
                details: Default::default()
            }
            .conflict_scope(),
            ConflictScope::Targets(vec![issue_target("a")])
        );
    }

    #[test]
    fn command_summary_is_compact() {
        let zot = Zot {
            id: "z".into(),
            revision: 7,
            activity_revision: 0,
            config: ZotConfig {
                name: "n".into(),
                purpose: "p".repeat(10_000),
                home_device_id: "d".into(),
                repositories: vec![],
                coordinator_model: ZotModel {
                    harness: "mock".into(),
                    model: None,
                },
                worker_device_id: "d".into(),
                max_concurrent_workers: 1,
                max_followups: 1,
                avatar: None,
                avatar_image: None,
            },
            status: ZotStatus::Active,
            coordinator_chat_id: "zot-z-0".into(),
            previous_coordinator_chats: vec![],
            workers: vec![],
            decisions: vec![],
            issues: vec![ZotIssue {
                id: "fix".into(),
                number: 3,
                details: ZotIssueDetails {
                    title: "Fix".into(),
                    description: "d".repeat(10_000),
                    ..Default::default()
                },
                created_at: 0,
                updated_at: 1,
                threads: vec![],
                pull_requests: vec![],
                activity: vec![],
            }],
        };
        let command = ZotCommand {
            command_id: "c".into(),
            zot_id: "z".into(),
            expected_revision: None,
            action: ZotAction::CommentIssue {
                issue_id: "fix".into(),
                text: "hi".into(),
                reply_to: None,
            },
        };
        let summary = command_summary(&command, Some(&zot));
        assert_eq!(summary["revision"], 7);
        assert_eq!(summary["issue"]["code"], "ZAP-3");
        assert!(summary["summary"].as_str().unwrap().contains("ZAP-3"));
        assert!(summary.to_string().len() < 1000);
    }
}
