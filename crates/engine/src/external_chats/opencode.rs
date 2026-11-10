//! OpenCode keeps sessions in one SQLite file: 2.x in `session_v2`/`session_message`,
//! 1.x in `session`/`message`/`part`. Opened read-only; the session id is the cursor.
use super::{Base, Conversation, append, meaningful, user};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};
use zuse_doc::SessionMessageEntry;
use zuse_protocol::{AgentEvent, HarnessId};

const LIMIT: usize = 100;

#[derive(Clone, Copy)]
enum Schema {
    V2,
    V1,
}
struct Session {
    id: String,
    directory: String,
    title: Option<String>,
    updated: i64,
}

pub(super) fn database() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".local/share")
        })
        .join("opencode/opencode.db")
}
fn open(path: &Path) -> Result<Connection, String> {
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| {
        "This provider chat is no longer readable. Reopen Import chat to refresh.".to_owned()
    })?;
    db.busy_timeout(Duration::from_millis(250))
        .map_err(|e| e.to_string())?;
    Ok(db)
}
fn sessions(db: &Connection, schema: Schema, id: Option<&str>) -> Vec<Session> {
    let table = match schema {
        Schema::V2 => "session_v2",
        Schema::V1 => "session",
    };
    let filter = if id.is_some() {
        "id = ?1"
    } else {
        "parent_id IS NULL AND ?1 IS NULL"
    };
    let sql = format!(
        "SELECT id, directory, title, time_updated FROM {table} WHERE {filter} ORDER BY time_updated DESC LIMIT {LIMIT}"
    );
    let Ok(mut statement) = db.prepare(&sql) else {
        return vec![];
    };
    statement
        .query_map(params![id], |row| {
            Ok(Session {
                id: row.get(0)?,
                directory: row.get(1)?,
                title: row.get(2)?,
                updated: row.get(3)?,
            })
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}
pub(super) fn discover(path: &Path) -> Vec<Conversation> {
    if !path.is_file() {
        return vec![];
    }
    let Ok(db) = open(path) else {
        return vec![];
    };
    [Schema::V2, Schema::V1]
        .into_iter()
        .flat_map(|schema| {
            sessions(&db, schema, None)
                .into_iter()
                .map(move |session| (schema, session))
        })
        .filter_map(|(schema, session)| load(&db, path, schema, session, false).ok())
        .collect()
}
pub(super) fn read(path: &Path, id: &str, history: bool) -> Result<Conversation, String> {
    let db = open(path)?;
    [Schema::V2, Schema::V1]
        .into_iter()
        .find_map(|schema| {
            sessions(&db, schema, Some(id))
                .into_iter()
                .next()
                .map(|session| (schema, session))
        })
        .ok_or_else(|| {
            "That provider chat is no longer available. Reopen Import chat to refresh.".to_owned()
        })
        .and_then(|(schema, session)| load(&db, path, schema, session, history))
}
fn query(db: &Connection, sql: &str, id: &str) -> Vec<(String, String, i64, String)> {
    let Ok(mut statement) = db.prepare(sql) else {
        return vec![];
    };
    statement
        .query_map(params![id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}
fn load(
    db: &Connection,
    path: &Path,
    schema: Schema,
    session: Session,
    history: bool,
) -> Result<Conversation, String> {
    if session.id.trim().is_empty() || session.directory.trim().is_empty() {
        return Err("Provider chat has no session or project identity.".into());
    }
    let mut preview = None;
    let mut messages = Vec::new();
    let mut sidecars = Vec::new();
    match schema {
        Schema::V2 => {
            let sql = if history {
                "SELECT id, type, time_created, data FROM session_message WHERE session_id = ?1 ORDER BY seq"
            } else {
                "SELECT id, type, time_created, data FROM session_message WHERE session_id = ?1 AND type = 'user' ORDER BY seq LIMIT 8"
            };
            for (id, kind, timestamp, data) in query(db, sql, &session.id) {
                let Ok(data) = serde_json::from_str::<Value>(&data) else {
                    continue;
                };
                match kind.as_str() {
                    "user" => {
                        let text = data["text"].as_str().unwrap_or_default();
                        if meaningful(text) {
                            preview.get_or_insert_with(|| text.to_owned());
                            if history {
                                user(&mut messages, text.to_owned(), timestamp);
                            }
                        }
                    }
                    "assistant" if history => {
                        let events = data["content"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .flat_map(|block| events(&id, block, false))
                            .collect();
                        append(&mut messages, &mut sidecars, events, timestamp);
                    }
                    // Subagent notices, compaction summaries and idle markers.
                    _ => {}
                }
            }
        }
        Schema::V1 => v1(
            db,
            &session.id,
            history,
            &mut preview,
            &mut messages,
            &mut sidecars,
        ),
    }
    let title = session.title.filter(|title| {
        !title.trim().is_empty()
            && !["New session - ", "Child session - "]
                .iter()
                .any(|prefix| title.starts_with(prefix))
    });
    let base = Base {
        provider: HarnessId::Opencode,
        cursor: session.id,
        project: session.directory,
        title,
        preview,
        updated: session.updated,
    };
    Ok(base.finish(path, messages, sidecars))
}
fn v1(
    db: &Connection,
    session: &str,
    history: bool,
    preview: &mut Option<String>,
    messages: &mut Vec<SessionMessageEntry>,
    sidecars: &mut Vec<(String, zuse_doc::SidecarPayload)>,
) {
    let sql = if history {
        "SELECT id, json_extract(data, '$.role'), time_created, '' FROM message WHERE session_id = ?1 ORDER BY time_created, id"
    } else {
        "SELECT id, json_extract(data, '$.role'), time_created, '' FROM message WHERE session_id = ?1 AND json_extract(data, '$.role') = 'user' ORDER BY time_created, id LIMIT 8"
    };
    let rows = query(db, sql, session);
    let mut parts: HashMap<String, Vec<Value>> = HashMap::new();
    let sql = if history {
        "SELECT message_id, '', 0, data FROM part WHERE session_id = ?1 ORDER BY message_id, id"
    } else {
        "SELECT p.message_id, '', 0, p.data FROM part p JOIN message m ON m.id = p.message_id WHERE p.session_id = ?1 AND json_extract(m.data, '$.role') = 'user' AND json_extract(p.data, '$.type') = 'text' ORDER BY m.time_created, m.id, p.id LIMIT 32"
    };
    for (message, _, _, data) in query(db, sql, session) {
        if let Ok(data) = serde_json::from_str(&data) {
            parts.entry(message).or_default().push(data);
        }
    }
    for (id, role, timestamp, _) in rows {
        let parts = parts.remove(&id).unwrap_or_default();
        if role == "user" {
            let text = parts
                .iter()
                .filter(|part| part["type"] == "text" && !visible_only(part))
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if meaningful(&text) {
                preview.get_or_insert_with(|| text.clone());
                if history {
                    user(messages, text, timestamp);
                }
            }
        } else if role == "assistant" && history {
            let events = parts
                .iter()
                .flat_map(|part| events(&id, part, true))
                .collect();
            append(messages, sidecars, events, timestamp);
        }
    }
}
/// Synthetic or ignored text was never typed by the user.
fn visible_only(part: &Value) -> bool {
    part["synthetic"] == true || part["ignored"] == true
}
/// One saved block: 2.x assistant `content[]` or a 1.x part. Call ids are scoped to their
/// message because providers repeat them across steps.
fn events(message: &str, block: &Value, legacy: bool) -> Vec<AgentEvent> {
    let text = block["text"].as_str().filter(|t| !t.trim().is_empty());
    match block["type"].as_str() {
        Some("text") if !visible_only(block) => text
            .map(|text| AgentEvent::TextDelta {
                text: format!("{}\n\n", text.trim_end()),
            })
            .into_iter()
            .collect(),
        Some("reasoning") => text
            .map(|text| AgentEvent::ReasoningDelta { text: text.into() })
            .into_iter()
            .collect(),
        Some("tool") => {
            let (name, call) = if legacy {
                (
                    &block["tool"],
                    block["callID"].as_str().or(block["id"].as_str()),
                )
            } else {
                (&block["name"], block["id"].as_str())
            };
            let id = format!("{message}:{}", call.unwrap_or_default());
            let state = &block["state"];
            let mut events = vec![AgentEvent::ToolCall {
                id: id.clone(),
                call: zuse_providers::opencode::tool_call(
                    name.as_str().unwrap_or("Tool"),
                    &state["input"],
                ),
            }];
            let status = state["status"].as_str().unwrap_or_default();
            if matches!(status, "completed" | "error") {
                let content = state["content"].as_array().map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| b["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                });
                let output = [
                    state["output"].as_str().map(str::to_owned),
                    content,
                    state["error"]["message"].as_str().map(str::to_owned),
                    state["error"].as_str().map(str::to_owned),
                ]
                .into_iter()
                .flatten()
                .find(|output| !output.is_empty());
                events.push(AgentEvent::ToolResult {
                    id,
                    is_error: status == "error",
                    output,
                    diff: None,
                });
            }
            events
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zuse_doc::{MessagePart, MessageRole};
    use zuse_protocol::ToolCall;

    const SESSION: &str = "id text PRIMARY KEY, project_id text NOT NULL, workspace_id text, parent_id text, slug text NOT NULL, directory text NOT NULL, path text, title text, version text NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL, time_archived integer";
    fn fixture(dir: &Path) -> PathBuf {
        let path = dir.join("opencode/opencode.db");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Connection::open(&path).unwrap();
        db.execute_batch(&format!(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE session_v2 ({SESSION}, fork_session_id text, time_idle integer);
             CREATE TABLE session_message (id text PRIMARY KEY, session_id text NOT NULL, type text NOT NULL, seq integer NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);
             CREATE TABLE session ({SESSION});
             CREATE TABLE message (id text PRIMARY KEY, session_id text NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);
             CREATE TABLE part (id text PRIMARY KEY, message_id text NOT NULL, session_id text NOT NULL, time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);"
        ))
        .unwrap();
        path
    }
    fn session(
        db: &Connection,
        table: &str,
        id: &str,
        parent: Option<&str>,
        dir: &Path,
        title: &str,
        updated: i64,
    ) {
        db.execute(
            &format!("INSERT INTO {table} (id, project_id, parent_id, slug, directory, path, title, version, time_created, time_updated) VALUES (?1, 'p', ?2, 'slug', ?3, '', ?4, '2.0.21', 1, ?5)"),
            params![id, parent, dir.to_str().unwrap(), title, updated],
        )
        .unwrap();
    }
    fn message(db: &Connection, session: &str, seq: i64, kind: &str, data: Value) {
        db.execute(
            "INSERT INTO session_message VALUES (?1, ?2, ?3, ?4, ?4, ?4, ?5)",
            params![
                format!("{session}_{seq}"),
                session,
                kind,
                seq,
                data.to_string()
            ],
        )
        .unwrap();
    }
    fn texts(conversation: &Conversation) -> String {
        serde_json::to_string(&conversation.messages).unwrap()
    }

    #[test]
    fn opencode_v2_sessions_list_top_level_and_import_tools() {
        let temp = tempfile::tempdir().unwrap();
        let path = fixture(temp.path());
        let db = Connection::open(&path).unwrap();
        session(
            &db,
            "session_v2",
            "ses_main",
            None,
            temp.path(),
            "Security audit",
            20,
        );
        session(
            &db,
            "session_v2",
            "ses_child",
            Some("ses_main"),
            temp.path(),
            "Child",
            30,
        );
        session(
            &db,
            "session_v2",
            "ses_new",
            None,
            temp.path(),
            "New session - 2026-11-10T10:00:00.000Z",
            10,
        );
        message(
            &db,
            "ses_main",
            1,
            "user",
            json!({"time":{"created":1},"text":"<system_instruction>hidden","files":[]}),
        );
        message(
            &db,
            "ses_main",
            2,
            "user",
            json!({"time":{"created":2},"text":"Audit the repo","files":[],"agents":[]}),
        );
        message(
            &db,
            "ses_main",
            3,
            "assistant",
            json!({"agent":"build","content":[
                {"type":"reasoning","text":"Look around first"},
                {"type":"text","text":"Listing files."},
                {"type":"tool","id":"call_1","name":"shell","executed":false,"state":{"status":"completed","input":{"command":"ls"},"content":[{"type":"text","text":"README.md"}],"metadata":{"exit":0}}},
                {"type":"tool","id":"call_2","name":"execute","executed":false,"state":{"status":"error","input":{},"error":{"type":"tool.execution","message":"Missing key"}}}
            ]}),
        );
        message(
            &db,
            "ses_main",
            4,
            "assistant",
            json!({"content":[
                {"type":"tool","id":"call_1","name":"read","state":{"status":"completed","input":{"path":"README.md"},"content":[{"type":"text","text":"1: hello"}]}},
                {"type":"text","text":"Done."}
            ]}),
        );
        message(
            &db,
            "ses_main",
            5,
            "synthetic",
            json!({"text":"<subagent sessionID=\"ses_child\">","metadata":{"source":"subagent"}}),
        );
        message(
            &db,
            "ses_main",
            6,
            "compaction",
            json!({"summary":"## Objective","status":"completed"}),
        );
        message(&db, "ses_main", 7, "idle", json!({"outcome":"succeeded"}));
        message(&db, "ses_main", 8, "user", json!({"text":"Thanks"}));
        message(
            &db,
            "ses_new",
            1,
            "user",
            json!({"text":"Untitled question"}),
        );
        drop(db);

        let found = discover(&path);
        assert_eq!(found.len(), 2, "subagent sessions stay out of the list");
        let main = found.iter().find(|c| c.cursor == "ses_main").unwrap();
        assert_eq!(main.row.provider, HarnessId::Opencode);
        assert_eq!(main.row.title, "Security audit");
        assert_eq!(main.row.preview, "Audit the repo");
        assert_eq!(main.row.updated_at, 20);
        assert_eq!(main.row.project_path, temp.path().to_str().unwrap());
        assert!(main.row.importable() && main.row.matches("opencode"));
        assert!(main.messages.is_empty(), "listing never reads transcripts");
        let untitled = found.iter().find(|c| c.cursor == "ses_new").unwrap();
        assert_eq!(untitled.row.title, "Untitled question");

        let full = super::super::history(main).unwrap();
        assert_eq!(full.row.id, main.row.id);
        let roles: Vec<_> = full.messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            [MessageRole::User, MessageRole::Assistant, MessageRole::User]
        );
        let tools: Vec<_> = full.messages[1]
            .parts
            .iter()
            .filter_map(|part| match part {
                MessagePart::Tool { id, call, .. } => Some((id.clone(), call.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(tools.len(), 3, "repeated call ids stay distinct: {tools:?}");
        assert!(matches!(&tools[0].1, ToolCall::Exec { command } if command == "ls"));
        assert!(matches!(&tools[2].1, ToolCall::ReadFile { path } if path == "README.md"));
        let history = texts(&full);
        for text in ["Look around first", "Listing files.", "Done.", "Thanks"] {
            assert!(history.contains(text), "Missing {text}: {history}");
        }
        for text in ["hidden", "subagent sessionID", "## Objective"] {
            assert!(!history.contains(text), "Leaked {text}: {history}");
        }
        let outputs: Vec<_> = full
            .sidecars
            .iter()
            .filter_map(|(_, p)| p.output.clone())
            .collect();
        assert!(outputs.contains(&"README.md".to_owned()), "{outputs:?}");
        assert!(outputs.contains(&"Missing key".to_owned()), "{outputs:?}");
        assert!(read(&path, "ses_missing", true).is_err());
    }

    #[test]
    fn opencode_v1_parts_map_to_messages() {
        let temp = tempfile::tempdir().unwrap();
        let path = fixture(temp.path());
        let db = Connection::open(&path).unwrap();
        session(
            &db,
            "session",
            "ses_old",
            None,
            temp.path(),
            "Legacy chat",
            5,
        );
        let insert_message = |id: &str, at: i64, data: Value| {
            db.execute(
                "INSERT INTO message VALUES (?1, 'ses_old', ?2, ?2, ?3)",
                params![id, at, data.to_string()],
            )
            .unwrap();
        };
        insert_message("msg_a", 1, json!({"role":"user","time":{"created":1}}));
        insert_message(
            "msg_b",
            2,
            json!({"role":"assistant","time":{"created":2},"modelID":"m","providerID":"p"}),
        );
        let insert_part = |id: &str, message: &str, data: Value| {
            db.execute(
                "INSERT INTO part VALUES (?1, ?2, 'ses_old', 1, 1, ?3)",
                params![id, message, data.to_string()],
            )
            .unwrap();
        };
        insert_part(
            "prt_1",
            "msg_a",
            json!({"type":"text","text":"Fix the build"}),
        );
        insert_part(
            "prt_2",
            "msg_a",
            json!({"type":"text","text":"Called the Read tool","synthetic":true}),
        );
        insert_part("prt_3", "msg_b", json!({"type":"step-start"}));
        insert_part(
            "prt_4",
            "msg_b",
            json!({"type":"tool","callID":"toolu_1","tool":"bash","state":{"status":"completed","input":{"command":"cargo build"},"output":"ok","title":"cargo build"}}),
        );
        insert_part(
            "prt_5",
            "msg_b",
            json!({"type":"text","text":"Build passes."}),
        );
        drop(db);

        let found = discover(&path);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].row.preview, "Fix the build");
        let full = read(&path, "ses_old", true).unwrap();
        assert_eq!(full.messages.len(), 2);
        let history = texts(&full);
        assert!(history.contains("Build passes.") && !history.contains("Called the Read tool"));
        assert!(matches!(
            &full.messages[1].parts[0],
            MessagePart::Tool { call: ToolCall::Exec { command }, .. } if command == "cargo build"
        ));
        assert_eq!(full.sidecars[0].1.output.as_deref(), Some("ok"));
    }

    #[test]
    fn opencode_missing_or_foreign_database_lists_nothing() {
        let temp = tempfile::tempdir().unwrap();
        assert!(discover(&temp.path().join("absent.db")).is_empty());
        let path = temp.path().join("other.db");
        Connection::open(&path)
            .unwrap()
            .execute_batch("CREATE TABLE t (x)")
            .unwrap();
        assert!(discover(&path).is_empty());
    }
}
