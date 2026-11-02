//! Muse transcript items → Zuse's normalized agent events.
//!
//! Muse re-emits an item whole at a strictly higher `revision` on every
//! change (`item/started` / `item/updated` / `item/completed`) and streams
//! text through `item/delta`. The fold here keeps each item at its latest
//! revision and remembers what has already been shown, so a redelivered or
//! gap-filled frame never duplicates text or tool chips.
use serde_json::Value;
use std::collections::HashMap;
use zuse_protocol::{AgentEvent, BackgroundTaskKind, ToolCall, ToolDiff};

use crate::LiveBackgroundTask;

#[derive(Default)]
struct ItemState {
    kind: String,
    revision: u64,
    /// Text already shown (assistant replies and reasoning).
    shown: String,
    /// The reasoning summary part last streamed.
    part: Option<String>,
    announced: bool,
    finished: bool,
    /// A backgrounded tool still running: (description, shell-like).
    background: Option<(String, bool)>,
}

#[derive(Default)]
pub(super) struct Items {
    items: HashMap<String, ItemState>,
}

impl Items {
    /// Fold one full item (`item/started` / `item/updated` / `item/completed`).
    pub fn item(&mut self, item: &Value) -> Vec<AgentEvent> {
        let Some(id) = item["itemId"].as_str() else {
            return vec![];
        };
        let revision = item["revision"].as_u64().unwrap_or(0);
        let kind = item["kind"].as_str().unwrap_or("").to_owned();
        let state = self.items.entry(id.to_owned()).or_default();
        if state.revision != 0 && revision <= state.revision {
            return vec![];
        }
        state.revision = revision;
        state.kind = kind.clone();
        let terminal = item["status"].as_str().is_some_and(|s| s != "inProgress");
        let mut events = vec![];
        match kind.as_str() {
            "agentMessage" => {
                if let Some(text) = item["text"].as_str() {
                    if let Some(suffix) = unseen(&state.shown, text) {
                        events.push(AgentEvent::TextDelta {
                            text: suffix.to_owned(),
                        });
                        state.shown = text.to_owned();
                    }
                }
            }
            "reasoning" => {
                // Streamed summaries are already on screen; a reply that was
                // never streamed shows once, on its terminal revision.
                if terminal && state.shown.is_empty() {
                    let text = item["text"]
                        .as_str()
                        .filter(|t| !t.is_empty())
                        .map(str::to_owned)
                        .or_else(|| {
                            let parts: Vec<&str> = item["summary"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .collect();
                            (!parts.is_empty()).then(|| parts.join("\n\n"))
                        });
                    if let Some(text) = text {
                        state.shown = text.clone();
                        events.push(AgentEvent::ReasoningDelta { text });
                    }
                }
            }
            // The user's own message is the host's echo; steering
            // confirmation reads it separately.
            "userMessage" => {}
            "compaction" => {
                if terminal && !state.finished {
                    state.finished = true;
                    let text = match item["outcome"].as_str() {
                        Some("noop") => "Nothing to compact.".to_owned(),
                        Some("failed") | Some("cancelled") => format!(
                            "Context compaction did not complete{}.",
                            item["reason"]
                                .as_str()
                                .map(|r| format!(": {r}"))
                                .unwrap_or_default()
                        ),
                        _ => "Context compacted.".to_owned(),
                    };
                    events.push(AgentEvent::TextDelta { text });
                }
            }
            _ => {
                if !state.announced {
                    state.announced = true;
                    events.push(AgentEvent::ToolCall {
                        id: id.to_owned(),
                        call: tool_call(item),
                    });
                }
                state.background = (item["background"] == true && !terminal).then(|| {
                    let call = tool_call(item);
                    let shell = matches!(call, ToolCall::Exec { .. });
                    let description = match call {
                        ToolCall::Exec { command } => command,
                        _ => tool_name(item),
                    };
                    (description, shell)
                });
                if terminal && !state.finished {
                    state.finished = true;
                    events.push(AgentEvent::ToolResult {
                        id: id.to_owned(),
                        is_error: item["status"] != "completed",
                        output: output(item),
                        diff: (item["status"] == "completed")
                            .then(|| diff(item))
                            .flatten(),
                    });
                }
            }
        }
        events
    }

    /// Fold one `item/delta`.
    pub fn delta(&mut self, params: &Value) -> Vec<AgentEvent> {
        let (Some(id), Some(delta)) = (params["itemId"].as_str(), params["delta"].as_str()) else {
            return vec![];
        };
        if delta.is_empty() {
            return vec![];
        }
        let field = params["field"].as_str().unwrap_or("text");
        let Some(state) = self.items.get_mut(id) else {
            // A delta for an item not yet opened: the item's own revision
            // will carry the text, reconciled against what was shown.
            return vec![];
        };
        match (state.kind.as_str(), field) {
            ("agentMessage", "text") => {
                state.shown.push_str(delta);
                vec![AgentEvent::TextDelta {
                    text: delta.to_owned(),
                }]
            }
            ("reasoning", field) if field == "text" || field.starts_with("summary.") => {
                let mut text = String::new();
                if !state.shown.is_empty() && state.part.as_deref() != Some(field) {
                    text.push_str("\n\n");
                }
                text.push_str(delta);
                state.part = Some(field.to_owned());
                state.shown.push_str(&text);
                vec![AgentEvent::ReasoningDelta { text }]
            }
            _ => vec![],
        }
    }

    /// Backgrounded tools still running.
    pub fn background(&self) -> Vec<LiveBackgroundTask> {
        let mut tasks: Vec<_> = self
            .items
            .iter()
            .filter_map(|(id, state)| {
                let (description, shell) = state.background.clone()?;
                Some(LiveBackgroundTask {
                    id: id.clone(),
                    kind: if shell {
                        BackgroundTaskKind::Shell
                    } else {
                        BackgroundTaskKind::Other
                    },
                    description,
                    stoppable: true,
                })
            })
            .collect();
        tasks.sort_by(|a, b| a.id.cmp(&b.id));
        tasks
    }

    /// Forget settled items at a turn boundary so a long-lived session does
    /// not grow without bound. Open items (background work) are kept.
    pub fn prune(&mut self) {
        self.items.retain(|_, state| {
            state.background.is_some()
                || (!state.finished && !matches!(state.kind.as_str(), "agentMessage" | "reasoning"))
        });
    }
}

/// The part of `text` not yet shown, when `shown` is its prefix.
fn unseen<'a>(shown: &str, text: &'a str) -> Option<&'a str> {
    text.strip_prefix(shown).filter(|rest| !rest.is_empty())
}

fn str_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| value[*key].as_str())
}

fn args(item: &Value) -> Value {
    match &item["args"] {
        Value::String(raw) => serde_json::from_str::<Value>(raw)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| serde_json::json!({ "raw": raw })),
        Value::Object(_) => item["args"].clone(),
        _ => Value::Object(Default::default()),
    }
}

fn tool_name(item: &Value) -> String {
    match item["kind"].as_str() {
        Some("userShell") => "Bash".into(),
        Some("toolCall") | None => item["tool"].as_str().unwrap_or("Tool").to_owned(),
        Some(kind) => item["tool"].as_str().unwrap_or(kind).to_owned(),
    }
}

/// Map a Muse tool to the shared chip vocabulary.
pub(super) fn tool_call(item: &Value) -> ToolCall {
    let input = args(item);
    let path = || str_field(&input, &["path", "file_path", "filePath"]).map(str::to_owned);
    match item["kind"].as_str() {
        Some("userShell") => {
            return ToolCall::Exec {
                command: item["commandText"].as_str().unwrap_or("").into(),
            };
        }
        Some("subagent") => {
            let objective = item["objective"]
                .as_str()
                .or_else(|| item["role"].as_str())
                .map(str::trim)
                .filter(|o| !o.is_empty());
            return ToolCall::Unknown {
                name: match objective {
                    Some(objective) => {
                        format!("Agent: {}", objective.lines().next().unwrap_or(objective))
                    }
                    None => "Agent".into(),
                },
                input: Some(input),
            };
        }
        Some("webSearch") => {
            return ToolCall::WebSearch {
                query: str_field(&input, &["query", "q"]).unwrap_or("").into(),
            };
        }
        _ => {}
    }
    let name = tool_name(item);
    let lower = name.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("mcp__") {
        if let Some((server, tool)) = rest.split_once("__") {
            let offset = name.len() - tool.len();
            return ToolCall::Mcp {
                server: name[5..5 + server.len()].to_owned(),
                tool: name[offset..].to_owned(),
                input: Some(input),
            };
        }
    }
    for prefix in ["zuse__", "zuse_", "zuse."] {
        if lower.starts_with(prefix) && name.len() > prefix.len() {
            return ToolCall::Mcp {
                server: "zuse".into(),
                tool: name[prefix.len()..].to_owned(),
                input: Some(input),
            };
        }
    }
    match lower.as_str() {
        "bash" | "shell" | "exec_command" | "run_command" | "execute" | "terminal"
        | "shell_command" => ToolCall::Exec {
            command: str_field(&input, &["command", "cmd"]).unwrap_or("").into(),
        },
        "read" | "read_file" | "readfile" | "view" => match path() {
            Some(path) => ToolCall::ReadFile { path },
            None => ToolCall::Unknown {
                name,
                input: Some(input),
            },
        },
        "write" | "write_file" | "writefile" | "create_file" => ToolCall::WriteFile {
            path: path().unwrap_or_default(),
            content: str_field(&input, &["content", "contents", "text"]).map(str::to_owned),
        },
        "edit" | "edit_file" | "editfile" | "str_replace" | "search_replace"
        | "str_replace_editor" | "multi_edit" | "multiedit" => ToolCall::EditFile {
            path: path().unwrap_or_default(),
            old_string: str_field(&input, &["old_string", "oldString", "oldText", "old_str"])
                .map(str::to_owned),
            new_string: str_field(&input, &["new_string", "newString", "newText", "new_str"])
                .map(str::to_owned),
        },
        "apply_patch" | "patch" => ToolCall::ApplyPatch { path: path() },
        "grep" | "search" | "grep_search" | "search_files" | "rg" => ToolCall::Search {
            pattern: str_field(&input, &["pattern", "query", "regex"])
                .unwrap_or("")
                .into(),
            path: path(),
        },
        "glob" | "find" | "glob_files" | "list_files" => ToolCall::Glob {
            pattern: str_field(&input, &["pattern", "glob"]).unwrap_or("").into(),
        },
        "web_fetch" | "webfetch" | "fetch" | "fetch_url" => ToolCall::WebFetch {
            url: str_field(&input, &["url"]).unwrap_or("").into(),
            prompt: str_field(&input, &["prompt"]).map(str::to_owned),
        },
        "web_search" | "websearch" => ToolCall::WebSearch {
            query: str_field(&input, &["query", "q"]).unwrap_or("").into(),
        },
        _ => ToolCall::Unknown {
            name,
            input: Some(input),
        },
    }
}

fn output(item: &Value) -> Option<String> {
    let text = [
        &item["visibleOutput"],
        &item["result"]["text"],
        &item["result"]["summary"],
        &item["finalSummary"],
        &item["message"],
        &item["failureReason"],
        &item["fallbackText"],
    ]
    .into_iter()
    .find_map(|value| value.as_str().filter(|s| !s.trim().is_empty()))?;
    crate::tool_output::text(&Value::String(text.to_owned()))
}

fn diff(item: &Value) -> Option<ToolDiff> {
    match tool_call(item) {
        ToolCall::EditFile {
            path,
            old_string: Some(old),
            new_string: Some(new),
        } if !path.is_empty() => Some(ToolDiff {
            path,
            old_text: Some(cap(&old)),
            new_text: cap(&new),
        }),
        ToolCall::WriteFile {
            path,
            content: Some(content),
        } if !path.is_empty() => Some(ToolDiff {
            path,
            old_text: None,
            new_text: cap(&content),
        }),
        _ => None,
    }
}

fn cap(text: &str) -> String {
    const LIMIT: usize = 64 * 1024;
    if text.len() <= LIMIT {
        return text.into();
    }
    let mut at = LIMIT;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    format!("{}\n… [truncated]", &text[..at])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn texts(events: &[AgentEvent]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn streamed_replies_reconcile_without_duplicates_and_ignore_stale_revisions() {
        let mut items = Items::default();
        let started = json!({"itemId":"m","kind":"agentMessage","status":"inProgress","revision":1,"text":""});
        assert!(items.item(&started).is_empty());
        let streamed = items.delta(&json!({"itemId":"m","delta":"Hello "}));
        assert_eq!(texts(&streamed), "Hello ");
        let done = json!({"itemId":"m","kind":"agentMessage","status":"completed","revision":2,"text":"Hello 🌍"});
        assert_eq!(texts(&items.item(&done)), "🌍");
        // A redelivered (gap-filled) completion changes nothing.
        assert!(items.item(&done).is_empty());
        assert!(items.item(&started).is_empty());
    }

    #[test]
    fn a_reply_never_streamed_shows_once() {
        let mut items = Items::default();
        let done = json!({"itemId":"m","kind":"agentMessage","status":"completed","revision":3,"text":"Recovered"});
        assert_eq!(texts(&items.item(&done)), "Recovered");
        assert!(items.item(&done).is_empty());
    }

    #[test]
    fn reasoning_parts_are_separated() {
        let mut items = Items::default();
        items.item(&json!({"itemId":"r","kind":"reasoning","status":"inProgress","revision":1}));
        let mut all = items.delta(&json!({"itemId":"r","field":"summary.0","delta":"one"}));
        all.extend(items.delta(&json!({"itemId":"r","field":"summary.1","delta":"two"})));
        let text: String = all
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ReasoningDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "one\n\ntwo");
        assert!(
            items
                .item(&json!({"itemId":"r","kind":"reasoning","status":"completed","revision":2,"summary":["one","two"]}))
                .is_empty()
        );
    }

    #[test]
    fn tools_announce_once_and_settle_with_output_and_diff() {
        let mut items = Items::default();
        let args = json!({"path":"a.rs","old_string":"old","new_string":"new"}).to_string();
        let start = json!({"itemId":"t","kind":"toolCall","tool":"edit","status":"inProgress","revision":1,"args":args});
        let events = items.item(&start);
        assert!(
            matches!(&events[..], [AgentEvent::ToolCall { id, call: ToolCall::EditFile { path, .. } }] if id == "t" && path == "a.rs")
        );
        let done = json!({"itemId":"t","kind":"toolCall","tool":"edit","status":"completed","revision":2,"args":args,"visibleOutput":"ok"});
        let events = items.item(&done);
        assert!(matches!(
            &events[..],
            [AgentEvent::ToolResult { id, is_error: false, output: Some(out), diff: Some(diff) }]
                if id == "t" && out == "ok" && diff.new_text == "new"
        ));
        let failed = json!({"itemId":"x","kind":"toolCall","tool":"Bash","status":"failed","revision":1,"args":"{\"command\":\"false\"}","failureReason":"exit 1"});
        let events = items.item(&failed);
        assert!(
            matches!(&events[0], AgentEvent::ToolCall { call: ToolCall::Exec { command }, .. } if command == "false")
        );
        assert!(
            matches!(&events[1], AgentEvent::ToolResult { is_error: true, output: Some(o), .. } if o == "exit 1")
        );
    }

    #[test]
    fn mcp_subagent_and_background_tools_map_to_shared_shapes() {
        let item = |tool: &str| json!({"itemId":"t","kind":"toolCall","tool":tool,"status":"inProgress","revision":1,"args":"{}"});
        assert!(matches!(
            tool_call(&item("mcp__zuse__spawn_chat")),
            ToolCall::Mcp { server, tool, .. } if server == "zuse" && tool == "spawn_chat"
        ));
        assert!(matches!(
            tool_call(&item("zuse__show_visual")),
            ToolCall::Mcp { server, tool, .. } if server == "zuse" && tool == "show_visual"
        ));
        let spawn = tool_call(
            &json!({"itemId":"s","kind":"subagent","objective":"scan the repo\nthoroughly","status":"inProgress","revision":1}),
        );
        assert!(spawn.is_subagent_spawn());
        let mut items = Items::default();
        items.item(&json!({"itemId":"b","kind":"toolCall","tool":"bash","status":"inProgress","revision":1,"args":"{\"command\":\"npm run dev\"}","background":true}));
        let live = items.background();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].description, "npm run dev");
        assert_eq!(live[0].kind, BackgroundTaskKind::Shell);
        items.item(&json!({"itemId":"b","kind":"toolCall","tool":"bash","status":"cancelled","revision":2,"background":true}));
        assert!(items.background().is_empty());
    }
}
