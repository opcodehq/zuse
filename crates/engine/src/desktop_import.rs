//! Read-only Electron snapshots imported through the ordinary GPUI document stores.
//! Import never dispatches a provider request or runs a project command.
pub mod onboarding;
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
};
use zuse_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zuse_protocol::{Chat, ChatConfig, HarnessId, SandboxLevel, Space, ToolCall};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub projects: usize,
    pub threads: usize,
    pub sessions: usize,
    pub source_messages: usize,
    pub imported_sessions: usize,
    pub skipped_sessions: usize,
    pub rendered_entries: usize,
    pub resumable_sessions: usize,
    pub attachments_copied: usize,
    pub missing_attachments: Vec<String>,
    pub retained_message_kinds: BTreeMap<String, usize>,
    pub settings_mapped: Vec<String>,
    pub settings_retained: Vec<String>,
    pub warnings: Vec<String>,
}

fn id(kind: &str, original: &str) -> String {
    format!("desktop_{kind}_{:x}", Sha256::digest(original.as_bytes()))
}
fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v[key].as_str().filter(|s| !s.is_empty())
}
fn timestamp(v: &Value, key: &str) -> DateTime<Utc> {
    text(v, key)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or(DateTime::UNIX_EPOCH)
}
fn rows(db: &Connection, table: &str) -> Result<Vec<Value>> {
    // Only fixed table names from this module reach this helper.
    let mut statement = db.prepare(&format!("SELECT * FROM {table}"))?;
    let names = statement
        .column_names()
        .iter()
        .map(|n| (*n).to_owned())
        .collect::<Vec<_>>();
    let mut query = statement.query([])?;
    let mut result = Vec::new();
    while let Some(row) = query.next()? {
        let mut value = serde_json::Map::new();
        for (i, name) in names.iter().enumerate() {
            use rusqlite::types::ValueRef;
            value.insert(
                name.clone(),
                match row.get_ref(i)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => json!(n),
                    ValueRef::Real(n) => json!(n),
                    ValueRef::Text(s) => Value::String(std::str::from_utf8(s)?.to_owned()),
                    ValueRef::Blob(_) => bail!("Unexpected binary field in {table}"),
                },
            );
        }
        result.push(Value::Object(value));
    }
    Ok(result)
}
fn harness(provider: &str) -> Result<HarnessId> {
    Ok(match provider {
        "claude" => HarnessId::ClaudeCode,
        "codex" => HarnessId::Codex,
        "cursor" => HarnessId::Cursor,
        "grok" => HarnessId::Grok,
        "opencode" => HarnessId::Opencode,
        "gemini" => HarnessId::Gemini,
        "kiro" => HarnessId::Kiro,
        "devin" => HarnessId::Devin,
        "hermes" => HarnessId::Hermes,
        "pi" => HarnessId::Pi,
        "muse" => HarnessId::Muse,
        "antigravity" => HarnessId::Antigravity,
        "zuse" => HarnessId::Zuse,
        _ => bail!("Unsupported Electron provider: {provider}"),
    })
}

/// The SQLite snapshot is created locally with SQLite's backup API, including WAL data.
/// Its path stays in the receipt so unsupported source fields remain recoverable.
#[derive(Clone)]
pub struct Destination {
    pub workspace: crate::WorkspaceHost,
    pub doc_host: crate::DocHost,
    pub device_id: String,
    pub device_root: PathBuf,
    pub profile_root: PathBuf,
}

pub struct DesktopImport {
    db: Connection,
    snapshot: PathBuf,
    source: PathBuf,
    projects: Vec<Value>,
    chats: HashMap<String, Value>,
    sessions: Vec<Value>,
    worktrees: HashMap<String, Value>,
    provider_threads: HashMap<String, Value>,
    attachments: HashMap<String, Value>,
}
impl DesktopImport {
    pub fn open(source: &Path, snapshot: &Path) -> Result<Self> {
        let source = source
            .canonicalize()
            .context("Electron profile is unavailable")?;
        let snapshot = snapshot
            .canonicalize()
            .context("Electron snapshot is unavailable")?;
        let db = Connection::open_with_flags(&snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        db.execute_batch("PRAGMA query_only=ON; BEGIN;")?;
        let schema: i64 = db.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('chats','sessions','messages','projects','worktrees','session_provider_threads','attachments')", [], |r| r.get(0))?;
        ensure!(schema == 7, "Unsupported Electron database schema");
        let indexed = |table: &str| -> Result<HashMap<String, Value>> {
            Ok(rows(&db, table)?
                .into_iter()
                .map(|r| (r["id"].as_str().unwrap_or_default().to_owned(), r))
                .collect())
        };
        let projects = rows(&db, "projects")?;
        let chats = indexed("chats")?;
        let sessions = rows(&db, "sessions")?;
        let worktrees = indexed("worktrees")?;
        let provider_threads = indexed("session_provider_threads")?;
        let attachments = indexed("attachments")?;
        for session in &sessions {
            harness(text(session, "provider_id").context("Missing provider")?)?;
            ensure!(text(session, "id").is_some(), "Session has no ID");
            ensure!(
                chats.contains_key(text(session, "chat_id").context("Missing chat ID")?),
                "Session belongs to an unavailable thread"
            );
        }
        Ok(Self {
            db,
            snapshot,
            source,
            projects,
            chats,
            sessions,
            worktrees,
            provider_threads,
            attachments,
        })
    }
    pub fn inspect(&self) -> Result<Report> {
        Ok(Report {
            projects: self.projects.len(),
            threads: self.chats.len(),
            sessions: self.sessions.len(),
            source_messages: self
                .db
                .query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))?
                as usize,
            ..Default::default()
        })
    }
    /// `core` must own the destination's instance lock. Existing chats/settings are retained.
    pub fn apply(
        &self,
        core: &crate::EngineCore,
        device_root: &Path,
        profile_root: &Path,
    ) -> Result<Report> {
        self.apply_to(
            &Destination {
                workspace: core.workspace.clone(),
                doc_host: core.doc_host.clone(),
                device_id: core.device_id.clone(),
                device_root: device_root.to_path_buf(),
                profile_root: profile_root.to_path_buf(),
            },
            true,
            |_, _| {},
        )
    }
    pub fn apply_to(
        &self,
        destination: &Destination,
        apply_settings: bool,
        progress: impl Fn(usize, usize),
    ) -> Result<Report> {
        let device_root = &destination.device_root;
        let profile_root = &destination.profile_root;
        let mut report = self.inspect()?;
        let receipt_dir = profile_root.join("desktop-import");
        std::fs::create_dir_all(&receipt_dir)?;
        let owner_path = receipt_dir.join("source.json");
        if let Ok(bytes) = std::fs::read(&owner_path) {
            let old: Value = serde_json::from_slice(&bytes)?;
            ensure!(
                old["source"] == self.source.to_string_lossy().as_ref(),
                "This destination already imported another Electron profile"
            );
        }
        private_json(
            &owner_path,
            &json!({"source":self.source,"snapshot":self.snapshot,"format":1}),
        )?;
        let projects = self
            .projects
            .iter()
            .map(|p| (p["id"].as_str().unwrap_or_default(), p))
            .collect::<HashMap<_, _>>();
        let existing_spaces = destination.workspace.read_spaces()?;
        let mut mapped_projects = HashMap::new();
        for project in &self.projects {
            let key = text(project, "id").context("Project has no ID")?;
            let path = text(project, "path").context("Project has no path")?;
            let space_id = existing_spaces
                .iter()
                .find(|s| {
                    s.device_id == destination.device_id
                        && (s.path == path
                            || Path::new(&s.path)
                                .canonicalize()
                                .ok()
                                .zip(Path::new(path).canonicalize().ok())
                                .is_some_and(|(a, b)| a == b))
                })
                .map(|s| s.id.clone())
                .unwrap_or_else(|| id("project", key));
            mapped_projects.insert(key.to_owned(), space_id.clone());
            if destination.workspace.space(&space_id)?.is_some() {
                continue;
            }
            let space: Space = serde_json::from_value(json!({
                "id":space_id,"deviceId":destination.device_id,"path":path,"name":project["name"],
                "gitDetected":Path::new(path).join(".git").exists(),"createdAt":timestamp(project,"created_at")
            }))?;
            destination.workspace.import_space_row(&space)?;
        }
        let mut attachment_paths = HashMap::new();
        let uploads = profile_root.join("uploads").join("desktop-import");
        std::fs::create_dir_all(&uploads)?;
        for (key, attachment) in &self.attachments {
            ensure!(
                Path::new(key)
                    .file_name()
                    .is_some_and(|name| name == std::ffi::OsStr::new(key))
                    && key != "."
                    && key != "..",
                "Invalid attachment identity"
            );
            let original = text(attachment, "abs_path")
                .map(PathBuf::from)
                .filter(|p| p.is_file())
                .or_else(|| {
                    [
                        self.source.join("attachments").join(key),
                        self.source.join("attachments").join(format!("{key}.bin")),
                    ]
                    .into_iter()
                    .find(|p| p.is_file())
                });
            let Some(original) = original else {
                report.missing_attachments.push(key.clone());
                continue;
            };
            let extension = match text(attachment, "mime_type") {
                Some("image/png") => "png",
                Some("image/jpeg") => "jpg",
                Some("image/webp") => "webp",
                Some("image/gif") => "gif",
                Some("image/svg+xml") => "svg",
                Some("application/pdf") => "pdf",
                _ => "bin",
            };
            let destination = uploads.join(format!("{}.{}", id("attachment", key), extension));
            if !destination.exists() {
                let pending = destination.with_extension("pending");
                std::fs::copy(&original, &pending)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&pending, std::fs::Permissions::from_mode(0o600))?;
                }
                std::fs::rename(pending, &destination)?;
                report.attachments_copied += 1;
            }
            attachment_paths.insert(key.clone(), destination.to_string_lossy().into_owned());
        }
        for (position, session) in self.sessions.iter().enumerate() {
            progress(position, self.sessions.len());
            let original_id = text(session, "id").context("Missing session ID")?;
            let chat_id = id("session", original_id);
            let root_session = self.root_session(session)?;
            let completed = receipt_dir.join(format!("{chat_id}.json"));
            // A completed import is never replayed over a conversation edited in GPUI.
            if completed.exists() {
                let mut existing = destination
                    .workspace
                    .chat(&chat_id)?
                    .context("An imported session was deleted in GPUI; refusing to resurrect it")?;
                let expected_thread =
                    (original_id != root_session).then(|| id("session", root_session));
                if existing.thread_id != expected_thread {
                    existing.thread_id = expected_thread;
                    destination.workspace.import_chat_row(&existing)?;
                }
                report.skipped_sessions += 1;
                continue;
            }
            let thread = &self.chats[text(session, "chat_id").unwrap()];
            let project_id = text(session, "project_id").or_else(|| text(thread, "project_id"));
            let project = project_id.and_then(|id| projects.get(id).copied());
            let worktree = text(session, "worktree_id").and_then(|key| self.worktrees.get(key));
            let cwd = worktree
                .and_then(|w| text(w, "path"))
                .or_else(|| project.and_then(|p| text(p, "path")));
            let provider = harness(text(session, "provider_id").unwrap())?;
            let mut messages = self.messages(original_id, &attachment_paths, &mut report)?;
            let handle = destination.doc_host.open_local(&chat_id)?;
            for message in &mut messages {
                message.device_id = destination.device_id.clone();
                for part in &mut message.parts {
                    if let MessagePart::Tool {
                        id,
                        output,
                        output_ref,
                        output_bytes,
                        ..
                    } = part
                    {
                        if let Some(body) = output.take() {
                            let payload = zuse_doc::SidecarPayload {
                                part_id: id.clone(),
                                output: Some(body.clone()),
                                diff: None,
                            };
                            let (reference, _) = destination.doc_host.persist_tool_sidecar(
                                &chat_id,
                                &message.id,
                                &payload,
                            )?;
                            *output_bytes = Some(body.len() as u64);
                            *output_ref = reference;
                            *output = Some(body.chars().take(300).collect());
                        }
                    }
                }
            }
            handle.import_transcript(|| -> Result<()> {
                let existing = handle
                    .doc()
                    .read_entries()?
                    .into_iter()
                    .map(|m| m.id)
                    .collect::<HashSet<_>>();
                for message in &messages {
                    if !existing.contains(&message.id) {
                        handle.doc().push_message(message)?;
                    }
                }
                Ok(())
            })?;
            destination.doc_host.flush_chat_checked(&handle)?;
            let native = text(session, "active_provider_thread_id")
                .and_then(|id| self.provider_threads.get(id));
            let resume = native
                .filter(|n| {
                    n["status"] == "idle" && text(n, "provider") == text(session, "provider_id")
                })
                .and_then(|n| text(n, "native_thread_id"))
                .filter(|_| cwd.is_some_and(|p| Path::new(p).is_dir()));
            let entry_time = messages.last().map(|m| {
                DateTime::from_timestamp_millis(m.created_at).unwrap_or(DateTime::UNIX_EPOCH)
            });
            let archived = !thread["archived_at"].is_null();
            let chat: Chat = serde_json::from_value(json!({
                "id":chat_id,"deviceId":destination.device_id,"title":if original_id==root_session {text(thread,"title").or_else(||text(session,"title"))} else {text(session,"title").or_else(||text(thread,"title"))},
                "archived":archived,"cwd":cwd,"branch":worktree.and_then(|w|text(w,"branch")),
                "checkoutId":Value::Null,"spaceId":project_id.and_then(|p|mapped_projects.get(p)),
                "config":ChatConfig{harness:provider,model:text(session,"model").map(str::to_owned),reasoning:None,model_options:Default::default(),sandbox:sandbox(text(session,"runtime_mode")),plan:false},
                "lastMessagePreview":messages.iter().rev().flat_map(|m|&m.parts).find_map(|p|if let MessagePart::Text{text,..}=p{Some(text.chars().take(160).collect::<String>())}else{None}),
                "lastMessageAt":entry_time,"createdAt":timestamp(session,"created_at"),"lastSeenAt":text(thread,"last_read_at"),
                "harnessSessionId":resume,"harnessSessionCwd":resume.and(cwd),"roomGen":2,
                "parentChatId":Value::Null,"threadId":if original_id==root_session {None}else{Some(id("session",root_session))}
            }))?;
            destination.workspace.import_chat_row(&chat)?;
            private_json(
                &completed,
                &json!({"sourceSession":original_id,"chatId":chat_id,"entries":messages.len(),"providerCursor":native,"sourceSnapshot":self.snapshot}),
            )?;
            report.rendered_entries += messages.len();
            report.imported_sessions += 1;
            report.resumable_sessions += usize::from(resume.is_some());
            eprintln!("Imported session {}/{}", position + 1, self.sessions.len());
        }
        if apply_settings {
            self.settings(device_root, &receipt_dir, &mapped_projects, &mut report)?;
        }
        private_json(
            &receipt_dir.join("report.json"),
            &serde_json::to_value(&report)?,
        )?;
        Ok(report)
    }

    fn root_session<'a>(&'a self, session: &Value) -> Result<&'a str> {
        let thread_id = text(session, "chat_id").context("Session has no thread")?;
        let thread = self.chats.get(thread_id).context("Session has no thread")?;
        if let Some(active) = text(thread, "active_session_id") {
            if self
                .sessions
                .iter()
                .any(|s| text(s, "id") == Some(active) && text(s, "chat_id") == Some(thread_id))
            {
                return Ok(active);
            }
        }
        self.sessions
            .iter()
            .filter(|s| text(s, "chat_id") == Some(thread_id))
            .min_by_key(|s| {
                (
                    timestamp(s, "created_at"),
                    text(s, "id").unwrap_or_default(),
                )
            })
            .and_then(|s| text(s, "id"))
            .context("Thread has no saved sessions")
    }

    fn settings(
        &self,
        root: &Path,
        receipts: &Path,
        projects: &HashMap<String, String>,
        report: &mut Report,
    ) -> Result<()> {
        let marker = receipts.join("settings.json");
        if marker.exists() {
            return Ok(());
        }
        let Some(directory) = self.snapshot.parent() else {
            return Ok(());
        };
        let file = directory.join("settings.json");
        if !file.exists() {
            return Ok(());
        }
        let source: Value = serde_json::from_slice(&std::fs::read(file)?)?;
        ensure!(
            source.is_object(),
            "Electron settings must be a JSON object"
        );
        let read = |name: &str| -> Result<Value> {
            match std::fs::read(root.join(name)) {
                Ok(bytes) => {
                    let value: Value = serde_json::from_slice(&bytes)?;
                    ensure!(value.is_object(), "Invalid existing GPUI settings: {name}");
                    Ok(value)
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
                Err(e) => Err(e.into()),
            }
        };
        let mut ui = read("ui-settings.json")?;
        let mut composer = read("composer-defaults.json")?;
        let mut providers = read("harness-prefs.json")?;
        private_json(&receipts.join("original-settings.json"), &source)?;
        private_json(
            &receipts.join("gpui-settings-before.json"),
            &json!({"ui":ui,"composer":composer,"providers":providers}),
        )?;
        if let Some(appearance) =
            text(&source, "appearanceMode").filter(|v| matches!(*v, "system" | "dark" | "light"))
        {
            ui["appearance"] = json!(appearance);
            report.settings_mapped.push("appearanceMode".into());
        }
        if let Some(enabled) = source["completionSoundEnabled"].as_bool() {
            ui["soundCompletionEnabled"] = json!(enabled);
            report.settings_mapped.push("completionSoundEnabled".into());
        }
        if let Some(preset) = text(&source, "completionSoundPreset")
            .filter(|v| matches!(*v, "chime" | "soft" | "pop" | "bell" | "rise" | "bloom"))
        {
            ui["completionSoundPreset"] = json!(preset);
            report.settings_mapped.push("completionSoundPreset".into());
        }
        if let Some(mode) = text(&source, "defaultRuntimeMode") {
            let mode = match mode {
                "auto" => "auto",
                "full-access" => "full-access",
                "approval-required" => "approval-required",
                _ => {
                    report.warnings.push("The old automatic approval mode has no exact GPUI equivalent; imported new-chat default requires approval.".into());
                    "approval-required"
                }
            };
            ui["defaultPermissionMode"] = json!(mode);
            report.settings_mapped.push("defaultRuntimeMode".into());
        }
        let mut models = composer["modelByHarness"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        if let Some(defaults) = source["defaultModelByProvider"].as_object() {
            for (provider, model) in defaults {
                let h = harness(provider)?;
                let key = serde_json::to_value(h)?.as_str().unwrap().to_owned();
                let Some(model) = model.as_str().or_else(|| text(model, "id")) else {
                    continue;
                };
                models.insert(key, json!({"id":model,"label":model}));
            }
            composer["modelByHarness"] = json!(models);
            report.settings_mapped.push("defaultModelByProvider".into());
        }
        if let Some(provider) = text(&source, "defaultProviderId") {
            let h = harness(provider)?;
            let key = serde_json::to_value(h)?;
            composer["harness"] = key.clone();
            if let Some(model) = key
                .as_str()
                .and_then(|key| models.get(key))
                .and_then(|m| text(m, "id"))
            {
                ui["defaultModel"] = json!({"provider":h,"model":model,"label":model});
            }
            report.settings_mapped.push("defaultProviderId".into());
        }
        if let Some(enabled) = source["providerEnabled"].as_object() {
            let mut disabled = providers["disabled"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            for (provider, value) in enabled {
                let h = serde_json::to_value(harness(provider)?)?;
                disabled.retain(|v| v != &h);
                if value == false {
                    disabled.push(h);
                }
            }
            providers["disabled"] = json!(disabled);
            providers.as_object_mut().unwrap().remove("enabled");
            report.settings_mapped.push("providerEnabled".into());
        }
        if let Some(worktree) = source["defaultAutoCreateWorktree"].as_bool() {
            let mut choices = composer["workspaceByProject"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            for project in projects.values() {
                choices.insert(
                    project.clone(),
                    json!(if worktree { "worktree" } else { "local" }),
                );
            }
            composer["workspaceByProject"] = json!(choices);
            report
                .settings_mapped
                .push("defaultAutoCreateWorktree".into());
        }
        for key in source.as_object().unwrap().keys() {
            if !report.settings_mapped.contains(key)
                && !matches!(key.as_str(), "schemaVersion" | "onboardingCompleted")
            {
                report.settings_retained.push(key.clone());
            }
        }
        if !report.settings_retained.is_empty() {
            report.warnings.push("Settings without a GPUI equivalent are retained in original-settings.json and the Electron backup; they are not silently treated as applied.".into());
        }
        private_json(&root.join("ui-settings.json"), &ui)?;
        private_json(&root.join("composer-defaults.json"), &composer)?;
        private_json(&root.join("harness-prefs.json"), &providers)?;
        private_json(
            &marker,
            &json!({"mapped":report.settings_mapped,"retained":report.settings_retained,"source":self.source}),
        )?;
        Ok(())
    }

    fn messages(
        &self,
        session: &str,
        attachments: &HashMap<String, String>,
        report: &mut Report,
    ) -> Result<Vec<SessionMessageEntry>> {
        let mut statement = self.db.prepare("SELECT id,role,kind,content_json,created_at FROM messages WHERE session_id=? ORDER BY sequence,created_at,id")?;
        let mut query = statement.query([session])?;
        let mut entries: Vec<SessionMessageEntry> = Vec::new();
        let mut assistant = None;
        let mut tools: HashMap<String, (usize, usize)> = HashMap::new();
        while let Some(row) = query.next()? {
            let original: String = row.get(0)?;
            let role: String = row.get(1)?;
            let kind: String = row.get(2)?;
            let raw: String = row.get(3)?;
            let content: Value = serde_json::from_str(&raw)
                .context("Invalid message JSON; source record retained in snapshot")?;
            let at: String = row.get(4)?;
            let part_id = id("part", &original);
            let mut parts = Vec::new();
            match kind.as_str() {
                "user" | "user_rich" | "assistant" => {
                    if let Some(body) = text(&content, "text") {
                        parts.push(MessagePart::Text {
                            id: part_id.clone(),
                            text: body.to_owned(),
                        });
                    }
                    if kind == "user_rich" {
                        let mut ids = self.db.prepare(
                            "SELECT attachment_id FROM message_attachments WHERE message_id=?",
                        )?;
                        let linked = ids
                            .query_map([&original], |r| r.get::<_, String>(0))?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        for key in linked {
                            let metadata = self.attachments.get(&key);
                            let name = metadata
                                .and_then(|m| text(m, "original_name"))
                                .unwrap_or("Attachment");
                            let mime = metadata
                                .and_then(|m| text(m, "mime_type"))
                                .unwrap_or("application/octet-stream");
                            if let Some(path) =
                                attachments.get(&key).filter(|_| mime.starts_with("image/"))
                            {
                                parts.push(MessagePart::Image {
                                    id: id("attachment", &key),
                                    path: path.clone(),
                                    name: name.into(),
                                    mime_type: mime.into(),
                                });
                            } else {
                                parts.push(MessagePart::Text {
                                    id: id("attachment", &key),
                                    text: format!(
                                        "[Imported attachment: {name}; {}]",
                                        attachments
                                            .get(&key)
                                            .map(String::as_str)
                                            .unwrap_or("original file unavailable")
                                    ),
                                });
                            }
                        }
                    }
                }
                "thinking" => {
                    if let Some(body) = text(&content, "text") {
                        parts.push(MessagePart::Reasoning {
                            id: part_id.clone(),
                            text: body.to_owned(),
                        });
                    }
                }
                "tool_use" => {
                    parts.push(serde_json::from_value(json!({"kind":"tool","id":part_id,"call":ToolCall::Unknown{name:text(&content,"tool").unwrap_or("Imported tool").into(),input:content.get("input").cloned()},"resolved":true,"isError":false}))?);
                }
                "tool_result" => {
                    if let Some((entry, part)) =
                        text(&content, "itemId").and_then(|i| tools.get(i)).copied()
                    {
                        if let MessagePart::Tool {
                            output,
                            resolved,
                            is_error,
                            ..
                        } = &mut entries[entry].parts[part]
                        {
                            *output = Some(
                                content["output"]
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| content["output"].to_string()),
                            );
                            *resolved = true;
                            *is_error = content["isError"].as_bool().unwrap_or(false);
                        }
                        continue;
                    }
                    parts.push(MessagePart::Text {
                        id: part_id.clone(),
                        text: format!(
                            "[Imported tool result]\n{}",
                            content["output"]
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| content["output"].to_string())
                        ),
                    });
                }
                "error"
                | "interrupted"
                | "context_compaction"
                | "model_switch"
                | "user_question"
                | "user_question_answer"
                | "subagent_summary"
                | "ui_spec" => {
                    parts.push(MessagePart::Text {
                        id: part_id.clone(),
                        text: format!(
                            "[Imported {kind}]\n{}",
                            text(&content, "text")
                                .or_else(|| text(&content, "summary"))
                                .map(str::to_owned)
                                .unwrap_or_else(|| content.to_string())
                        ),
                    });
                }
                _ => {
                    *report.retained_message_kinds.entry(kind).or_default() += 1;
                    continue;
                }
            }
            if parts.is_empty() {
                continue;
            }
            let index = if role == "user" {
                assistant = None;
                entries.push(entry(id("message", &original), MessageRole::User, &at));
                entries.len() - 1
            } else if role == "assistant" || role == "tool" {
                *assistant.get_or_insert_with(|| {
                    entries.push(entry(id("message", &original), MessageRole::Assistant, &at));
                    entries.len() - 1
                })
            } else {
                assistant = None;
                entries.push(entry(id("message", &original), MessageRole::System, &at));
                entries.len() - 1
            };
            if kind == "tool_use" {
                if let Some(item) = text(&content, "itemId") {
                    tools.insert(item.into(), (index, entries[index].parts.len()));
                }
            }
            entries[index].parts.extend(parts);
        }
        Ok(entries)
    }
}
fn entry(id: String, role: MessageRole, at: &str) -> SessionMessageEntry {
    SessionMessageEntry {
        id,
        role,
        parts: Vec::new(),
        created_at: DateTime::parse_from_rfc3339(at)
            .map(|d| d.timestamp_millis())
            .unwrap_or(0),
        device_id: String::new(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    }
}
fn sandbox(mode: Option<&str>) -> SandboxLevel {
    match mode {
        Some("full-access") => SandboxLevel::DangerFullAccess,
        Some("auto") => SandboxLevel::Auto,
        _ => SandboxLevel::WorkspaceWrite,
    }
}
fn private_json(path: &Path, value: &Value) -> Result<()> {
    use std::io::Write;
    let pending = path.with_extension(format!("{}.pending", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&pending)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    std::fs::rename(pending, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    pub(super) fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("Electron");
        std::fs::create_dir(&source).unwrap();
        let db = Connection::open(source.join("zuse.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE projects(id TEXT,path TEXT,name TEXT,created_at TEXT);CREATE TABLE chats(id TEXT,project_id TEXT,title TEXT,active_session_id TEXT,archived_at TEXT,last_read_at TEXT);CREATE TABLE sessions(id TEXT,chat_id TEXT,project_id TEXT,provider_id TEXT,model TEXT,title TEXT,created_at TEXT,runtime_mode TEXT,active_provider_thread_id TEXT);CREATE TABLE worktrees(id TEXT,path TEXT,branch TEXT);CREATE TABLE session_provider_threads(id TEXT,provider TEXT,native_thread_id TEXT,status TEXT);CREATE TABLE attachments(id TEXT,abs_path TEXT,original_name TEXT,mime_type TEXT);CREATE TABLE message_attachments(message_id TEXT,attachment_id TEXT);CREATE TABLE messages(id TEXT,session_id TEXT,role TEXT,kind TEXT,content_json TEXT,created_at TEXT,sequence INTEGER);").unwrap();
        db.execute(
            "INSERT INTO projects VALUES('project',?,'Project','2026-01-01T00:00:00Z')",
            [source.to_str().unwrap()],
        )
        .unwrap();
        db.execute_batch("INSERT INTO chats VALUES('thread','project','Original thread','first','2026-01-01T00:00:00Z',NULL);INSERT INTO sessions VALUES('first','thread','project','codex','model','First','2026-01-01T00:00:00Z','approval-required','native');INSERT INTO sessions VALUES('second','thread','project','claude','model','Second','2026-01-02T00:00:00Z','full-access',NULL);INSERT INTO session_provider_threads VALUES('native','codex','original-cursor','idle');").unwrap();
        for (position, (message, role, kind, content)) in [
            ("user", "user", "user", json!({"text":"Original question"})),
            (
                "thinking",
                "assistant",
                "thinking",
                json!({"text":"Original reasoning"}),
            ),
            (
                "tool",
                "assistant",
                "tool_use",
                json!({"itemId":"call","tool":"Read","input":{"file":"README.md"}}),
            ),
            (
                "result",
                "tool",
                "tool_result",
                json!({"itemId":"call","output":"Exact original tool output","isError":false}),
            ),
            (
                "assistant",
                "assistant",
                "assistant",
                json!({"text":"Original response"}),
            ),
            ("usage", "system", "usage", json!({"inputTokens":100})),
        ]
        .into_iter()
        .enumerate()
        {
            db.execute(
                "INSERT INTO messages VALUES(?,'first',?,?,?,'2026-01-01T00:00:00Z',?)",
                rusqlite::params![message, role, kind, content.to_string(), position as i64],
            )
            .unwrap();
        }
        (temp, source)
    }
    pub(super) fn assemble(root: &Path) -> crate::EngineCore {
        crate::EngineCore::assemble(
            root,
            Arc::new(crate::HarnessRegistry::new()),
            HarnessId::ClaudeCode,
            None,
        )
        .unwrap()
    }
    #[tokio::test]
    async fn archived_threads_tabs_tools_continuity_and_replay_survive_restart() {
        let (temp, source) = fixture();
        let snapshot = source.join("zuse.sqlite");
        let before = std::fs::read(&snapshot).unwrap();
        let destination = temp.path().join("GPUI");
        let core = assemble(&destination);
        let importer = DesktopImport::open(&source, &snapshot).unwrap();
        let report = importer
            .apply(
                &core,
                &destination,
                &destination.join("orgs/dev-org/dev-user"),
            )
            .unwrap();
        assert_eq!(
            (report.imported_sessions, report.resumable_sessions),
            (2, 1)
        );
        assert_eq!(report.retained_message_kinds["usage"], 1);
        let first = core
            .workspace
            .chat(&id("session", "first"))
            .unwrap()
            .unwrap();
        let second = core
            .workspace
            .chat(&id("session", "second"))
            .unwrap()
            .unwrap();
        assert!(first.archived && second.archived);
        assert!(
            first.thread_id.is_none(),
            "The sidebar only lists main chats with no threadId"
        );
        assert_eq!(second.thread_id.as_deref(), Some(first.id.as_str()));
        assert_eq!(first.thread_id(), second.thread_id());
        assert_eq!(first.harness_session_id.as_deref(), Some("original-cursor"));
        let handle = core.doc_host.open_local(&first.id).unwrap();
        let messages = handle.doc().read_entries().unwrap();
        assert_eq!(messages.len(), 2);
        assert!(
            matches!(&messages[1].parts[0],MessagePart::Reasoning{text,..} if text=="Original reasoning")
        );
        let output_ref = match &messages[1].parts[1] {
            MessagePart::Tool {
                output_ref: Some(reference),
                resolved: true,
                ..
            } => reference,
            _ => panic!("Tool lost its original output"),
        };
        let client = zuse_rpc::memory_client(core.rpc_service());
        let output = client
            .call(
                "FetchToolBlob",
                json!({"chatId":first.id,"blobRef":output_ref,"offset":0,"limit":65536}),
            )
            .await
            .unwrap();
        assert_eq!(output["text"], "Exact original tool output");
        core.workspace
            .rename_chat(&first.id, "Changed in GPUI")
            .unwrap();
        let replay = importer
            .apply(
                &core,
                &destination,
                &destination.join("orgs/dev-org/dev-user"),
            )
            .unwrap();
        assert_eq!((replay.imported_sessions, replay.skipped_sessions), (0, 2));
        assert_eq!(
            core.workspace
                .chat(&first.id)
                .unwrap()
                .unwrap()
                .title
                .as_deref(),
            Some("Changed in GPUI")
        );
        assert_eq!(std::fs::read(&snapshot).unwrap(), before);
        drop(client);
        drop(handle);
        core.shutdown().await;
        drop(core);
        let reopened = assemble(&destination);
        assert_eq!(
            reopened
                .doc_host
                .open_local(&first.id)
                .unwrap()
                .doc()
                .read_entries()
                .unwrap(),
            messages
        );
        reopened.shutdown().await;
    }
    #[tokio::test]
    async fn malformed_messages_do_not_publish_an_empty_chat() {
        let (temp, source) = fixture();
        let snapshot = source.join("zuse.sqlite");
        Connection::open(&snapshot)
            .unwrap()
            .execute(
                "UPDATE messages SET content_json='bad json' WHERE id='user'",
                [],
            )
            .unwrap();
        let destination = temp.path().join("GPUI");
        let core = assemble(&destination);
        let importer = DesktopImport::open(&source, &snapshot).unwrap();
        assert!(
            importer
                .apply(
                    &core,
                    &destination,
                    &destination.join("orgs/dev-org/dev-user")
                )
                .is_err()
        );
        assert!(
            core.workspace
                .chat(&id("session", "first"))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn unknown_providers_are_rejected_before_import() {
        let (_temp, source) = fixture();
        let snapshot = source.join("zuse.sqlite");
        Connection::open(&snapshot)
            .unwrap()
            .execute("UPDATE sessions SET provider_id='unknown'", [])
            .unwrap();
        assert!(DesktopImport::open(&source, &snapshot).is_err());
    }

    #[tokio::test]
    async fn archived_thread_without_an_active_pointer_has_one_main_chat() {
        let (temp, source) = fixture();
        let snapshot = source.join("zuse.sqlite");
        Connection::open(&snapshot)
            .unwrap()
            .execute("UPDATE chats SET active_session_id=NULL", [])
            .unwrap();
        let target = temp.path().join("GPUI");
        let core = assemble(&target);
        DesktopImport::open(&source, &snapshot)
            .unwrap()
            .apply(&core, &target, &target.join("orgs/dev-org/dev-user"))
            .unwrap();
        let rows = core.workspace.read_chats().unwrap();
        assert_eq!(rows.iter().filter(|c| c.thread_id.is_none()).count(), 1);
        assert_eq!(rows.iter().filter(|c| c.thread_id.is_some()).count(), 1);
        core.shutdown().await;
    }

    #[tokio::test]
    async fn settings_merge_preserves_gpui_keys_and_reports_unmapped_preferences() {
        let (temp, source) = fixture();
        let old = json!({"appearanceMode":"light","defaultProviderId":"codex","defaultModelByProvider":{"codex":"original-model"},"defaultRuntimeMode":"full-access","completionSoundEnabled":false,"providerEnabled":{"claude":false,"codex":true},"modelEnabledByProvider":{"codex":{"other-model":false}}});
        std::fs::write(source.join("settings.json"), old.to_string()).unwrap();
        let target = temp.path().join("GPUI");
        let core = assemble(&target);
        std::fs::write(
            target.join("ui-settings.json"),
            json!({"sidebarWidth":249,"appearance":"dark"}).to_string(),
        )
        .unwrap();
        let importer = DesktopImport::open(&source, &source.join("zuse.sqlite")).unwrap();
        let report = importer
            .apply(&core, &target, &target.join("orgs/dev-org/dev-user"))
            .unwrap();
        let ui: Value =
            serde_json::from_slice(&std::fs::read(target.join("ui-settings.json")).unwrap())
                .unwrap();
        assert_eq!(ui["appearance"], "light");
        assert_eq!(ui["sidebarWidth"], 249);
        assert_eq!(ui["defaultPermissionMode"], "full-access");
        assert_eq!(ui["defaultModel"]["model"], "original-model");
        assert_eq!(report.settings_retained, vec!["modelEnabledByProvider"]);
        let retained: Value = serde_json::from_slice(
            &std::fs::read(
                target.join("orgs/dev-org/dev-user/desktop-import/original-settings.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(retained, old);
        std::fs::write(
            target.join("ui-settings.json"),
            json!({"appearance":"dark"}).to_string(),
        )
        .unwrap();
        importer
            .apply(&core, &target, &target.join("orgs/dev-org/dev-user"))
            .unwrap();
        let ui: Value =
            serde_json::from_slice(&std::fs::read(target.join("ui-settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            ui["appearance"], "dark",
            "Replaying migration must retain subsequent GPUI edits"
        );
        core.shutdown().await;
    }
}
