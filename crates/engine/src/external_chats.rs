//! Read provider history in place; import copies the transcript and retains its native cursor.
use chrono::DateTime;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};
use zuse_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zuse_protocol::{AgentEvent, HarnessId, ToolCall, external_chat::Row};
mod opencode;

pub(crate) struct Conversation {
    pub row: Row,
    pub cursor: String,
    pub source: PathBuf,
    pub messages: Vec<SessionMessageEntry>,
    pub sidecars: Vec<(String, zuse_doc::SidecarPayload)>,
}
fn string<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row[key].as_str().filter(|v| !v.trim().is_empty())
}
fn text(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    value
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| {
                    matches!(
                        block["type"].as_str(),
                        Some("text" | "input_text" | "output_text" | "summary_text")
                    )
                })
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}
fn meaningful(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && ![
            "<system_instruction>",
            "<task-notification>",
            "<app-context>",
            "You are working inside ",
            "<environment_context>",
            "# AGENTS.md instructions",
            "<permissions instructions>",
        ]
        .iter()
        .any(|prefix| text.starts_with(prefix))
}
fn clamp(text: &str, size: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= size {
        text
    } else {
        text.chars()
            .take(size - 1)
            .chain(std::iter::once('…'))
            .collect()
    }
}
fn rows(path: &Path) -> impl Iterator<Item = Value> {
    fs::File::open(path)
        .ok()
        .into_iter()
        .flat_map(|file| BufReader::new(file).lines().map_while(Result::ok))
        .filter_map(|line| serde_json::from_str(&line).ok())
}
fn files(root: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && entry.file_name() != "subagents" {
            files(&entry.path(), depth - 1, out);
        } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "jsonl") {
            out.push(entry.path());
        }
    }
}
pub(crate) fn roots() -> Vec<(HarnessId, PathBuf)> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let codex = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    vec![
        (HarnessId::ClaudeCode, claude.join("projects")),
        (HarnessId::Codex, codex.join("sessions")),
        (HarnessId::Codex, codex.join("archived_sessions")),
    ]
}
pub(crate) fn discover() -> Vec<Conversation> {
    let mut found: HashMap<String, Conversation> = HashMap::new();
    for (provider, root) in roots() {
        let mut paths = Vec::new();
        files(&root, 6, &mut paths);
        for source in paths {
            let Ok(conversation) = read(&source, provider, false) else {
                continue;
            };
            if found
                .get(&conversation.row.id)
                .is_none_or(|old| old.row.updated_at < conversation.row.updated_at)
            {
                found.insert(conversation.row.id.clone(), conversation);
            }
        }
    }
    for conversation in opencode::discover(&opencode::database()) {
        found
            .entry(conversation.row.id.clone())
            .or_insert(conversation);
    }
    let mut found: Vec<_> = found.into_values().collect();
    found.sort_by(|a, b| {
        b.row
            .updated_at
            .cmp(&a.row.updated_at)
            .then_with(|| a.row.id.cmp(&b.row.id))
    });
    found
}
/// Full transcript for a discovered conversation.
pub(crate) fn history(found: &Conversation) -> Result<Conversation, String> {
    match found.row.provider {
        HarnessId::Opencode => opencode::read(&found.source, &found.cursor, true),
        provider => read(&found.source, provider, true),
    }
}
pub(crate) fn read(
    source: &Path,
    provider: HarnessId,
    history: bool,
) -> Result<Conversation, String> {
    let metadata = fs::metadata(source).map_err(|_| {
        "This provider chat is no longer readable. Reopen Import chat to refresh.".to_owned()
    })?;
    let mut updated = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |time| time.as_millis() as i64);
    let mut cursor = String::new();
    let mut project = String::new();
    let mut title = None;
    let mut preview = None;
    let mut messages: Vec<SessionMessageEntry> = Vec::new();
    let mut sidecars = Vec::new();
    let mut claude = zuse_providers::claude::history_decoder();
    let mut seen = std::collections::HashSet::new();
    for row in rows(source) {
        let timestamp = string(&row, "timestamp")
            .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
            .map_or(updated, |v| v.timestamp_millis());
        updated = updated.max(timestamp);
        let payload = if provider == HarnessId::Codex {
            &row["payload"]
        } else {
            &row
        };
        if provider == HarnessId::ClaudeCode || row["type"] == "session_meta" {
            if let Some(id) = string(
                payload,
                if provider == HarnessId::Codex {
                    "id"
                } else {
                    "sessionId"
                },
            ) {
                cursor = id.to_owned();
            }
            if let Some(cwd) = string(payload, "cwd") {
                project = cwd.to_owned();
            }
        }
        if let Some(name) = string(&row, "aiTitle")
            .or_else(|| string(&row, "summary"))
            .or_else(|| string(&row, "customTitle"))
        {
            title = Some(name.to_owned());
        }
        let message = if provider == HarnessId::Codex {
            payload
        } else {
            &row["message"]
        };
        let role = message["role"]
            .as_str()
            .or_else(|| row["type"].as_str())
            .unwrap_or_default();
        let content = text(&message["content"]);
        if preview.is_none() && meaningful(&content) && matches!(role, "user" | "assistant") {
            preview = Some(content.clone());
        }
        if !history {
            continue;
        }
        if let Some(id) = string(&row, "uuid")
            && !seen.insert(id.to_owned())
        {
            continue;
        }
        if role == "user"
            && meaningful(&content)
            && (provider == HarnessId::ClaudeCode || row["type"] == "response_item")
        {
            user(&mut messages, content, timestamp);
        }
        let events = if provider == HarnessId::ClaudeCode
            && matches!(row["type"].as_str(), Some("assistant" | "user"))
        {
            claude(&row)
        } else if provider == HarnessId::Codex && row["type"] == "response_item" {
            codex_events(payload)
        } else {
            vec![]
        };
        append(&mut messages, &mut sidecars, events, timestamp);
    }
    if cursor.is_empty() || project.is_empty() {
        return Err("Provider chat has no session or project identity.".into());
    }
    if provider == HarnessId::Codex {
        if let Some(root) = source
            .ancestors()
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n == "sessions" || n == "archived_sessions")
            })
            .and_then(Path::parent)
        {
            for row in rows(&root.join("session_index.jsonl")) {
                if row["id"] == cursor
                    && let Some(name) = string(&row, "thread_name")
                {
                    title = Some(name.to_owned());
                }
            }
        }
    }
    let base = Base {
        provider,
        cursor,
        project,
        title,
        preview,
        updated,
    };
    Ok(base.finish(source, messages, sidecars))
}
pub(crate) struct Base {
    pub provider: HarnessId,
    pub cursor: String,
    pub project: String,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub updated: i64,
}
impl Base {
    pub fn finish(
        self,
        source: &Path,
        mut messages: Vec<SessionMessageEntry>,
        sidecars: Vec<(String, zuse_doc::SidecarPayload)>,
    ) -> Conversation {
        let Base {
            provider,
            cursor,
            project,
            title,
            preview,
            updated,
        } = self;
        let id = format!(
            "{:x}",
            Sha256::digest(format!("{provider:?}\0{cursor}\0{project}"))
        );
        let name = Path::new(&project)
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or_default()
            .to_owned();
        let preview = preview.unwrap_or_else(|| "Provider conversation".into());
        let title = title.unwrap_or_else(|| preview.clone());
        messages.retain(|message| !message.parts.is_empty());
        for message in &mut messages {
            for part in &mut message.parts {
                if let MessagePart::Tool { call, .. } = part {
                    *call = zuse_model::sanitize_tool_call(call);
                }
            }
        }
        Conversation {
            row: Row {
                id,
                provider,
                title: clamp(&title, 96),
                preview: clamp(&preview, 140),
                project_path: project.clone(),
                project_name: name,
                updated_at: updated,
                available: Path::new(&project).is_dir(),
            },
            cursor,
            source: source.to_owned(),
            messages,
            sidecars,
        }
    }
}
/// Assistant events fold into the open assistant entry, or start one.
fn append(
    messages: &mut Vec<SessionMessageEntry>,
    sidecars: &mut Vec<(String, zuse_doc::SidecarPayload)>,
    events: Vec<AgentEvent>,
    timestamp: i64,
) {
    if events.is_empty() {
        return;
    }
    if messages
        .last()
        .is_none_or(|m| m.role != MessageRole::Assistant)
    {
        messages.push(entry(
            MessageRole::Assistant,
            vec![],
            timestamp,
            messages.len(),
        ));
    }
    let current = messages.last_mut().unwrap();
    for event in events {
        zuse_model::fold_event_into_parts(&mut current.parts, &event);
        if let Some(payload) = zuse_doc::sidecar_payload(&event) {
            sidecars.push((current.id.clone(), payload));
        }
    }
}
fn user(messages: &mut Vec<SessionMessageEntry>, text: String, timestamp: i64) {
    messages.push(entry(
        MessageRole::User,
        vec![MessagePart::Text {
            id: "t0".into(),
            text,
        }],
        timestamp,
        messages.len(),
    ));
}
fn entry(
    role: MessageRole,
    parts: Vec<MessagePart>,
    timestamp: i64,
    index: usize,
) -> SessionMessageEntry {
    SessionMessageEntry {
        id: format!("import-{index}"),
        role,
        parts,
        created_at: timestamp,
        device_id: String::new(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    }
}
fn codex_events(item: &Value) -> Vec<AgentEvent> {
    match item["type"].as_str() {
        Some("message") if item["role"] == "assistant" => {
            let text = text(&item["content"]);
            if text.is_empty() {
                vec![]
            } else {
                vec![AgentEvent::TextDelta { text }]
            }
        }
        Some("function_call" | "custom_tool_call") => {
            let input = item["arguments"]
                .as_str()
                .and_then(|v| serde_json::from_str::<Value>(v).ok())
                .unwrap_or_else(|| item["input"].clone());
            let name = item["name"].as_str().unwrap_or("Tool");
            let call = if name == "exec_command" {
                ToolCall::Exec {
                    command: input["cmd"].as_str().unwrap_or_default().into(),
                }
            } else {
                ToolCall::Unknown {
                    name: name.into(),
                    input: Some(input),
                }
            };
            vec![AgentEvent::ToolCall {
                id: item["call_id"].as_str().unwrap_or_default().into(),
                call,
            }]
        }
        Some("function_call_output" | "custom_tool_call_output") => vec![AgentEvent::ToolResult {
            id: item["call_id"].as_str().unwrap_or_default().into(),
            is_error: false,
            output: item["output"].as_str().map(str::to_owned),
            diff: None,
        }],
        Some("reasoning") => {
            let text = text(&item["summary"]);
            if text.is_empty() {
                vec![]
            } else {
                vec![AgentEvent::ReasoningDelta { text }]
            }
        }
        _ => vec![],
    }
}
