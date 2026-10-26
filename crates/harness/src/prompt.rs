//! Prompt accounting and compaction boundaries preserve complete tool exchanges.
use crate::{Compaction, Message, Role};
use serde_json::Value;
pub fn render(history: &[Message], checkpoint: Option<&Compaction>) -> Vec<Message> {
    let Some(c) = checkpoint else {
        return history.to_vec();
    };
    let split = c.covered.min(history.len());
    let mut result = history[..split]
        .iter()
        .filter(|m| m.role == Role::System)
        .cloned()
        .collect::<Vec<_>>();
    if let Some(task) = history[..split].iter().find(|m| m.role == Role::User) {
        result.push(task.clone());
    }
    result.push(Message::text(
        Role::User,
        format!("Durable context summary:\n{}", c.summary),
    ));
    result.extend_from_slice(&history[split..]);
    result
}
pub fn boundary(history: &[Message]) -> usize {
    history
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, m)| m.role == Role::User || m.role == Role::Assistant)
        .nth(1)
        .map_or(0, |(i, _)| i)
}
pub fn complete_prefix(history: &[Message]) -> Vec<Message> {
    let end = history
        .iter()
        .rposition(Message::has_tool_calls)
        .unwrap_or(history.len());
    history[..end].to_vec()
}
pub struct Limits {
    pub window: u64,
    pub usable: u64,
    pub compact_at: u64,
}
pub fn limits(window: u64, cap: Option<u64>, percent: f64) -> Limits {
    let window = if window == 0 { 32000 } else { window };
    let percent = if !percent.is_finite() || percent <= 0. || percent > 100. {
        95.
    } else {
        percent
    };
    let threshold = window / 10 * 9 + window % 10 * 9 / 10;
    Limits {
        window,
        usable: ((window as f64 * (percent / 100.)).floor() as u64).min(window),
        compact_at: cap
            .filter(|v| *v > 0)
            .map_or(threshold, |v| v.min(threshold)),
    }
}
// Counting serialization avoids an owned JSON tree and output byte buffer.
#[derive(Default)]
struct ByteCount(u64);
impl std::io::Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn bytes(value: &impl serde::Serialize) -> u64 {
    let mut count = ByteCount::default();
    serde_json::to_writer(&mut count, value).expect("JSON values serialize to a counting writer");
    count.0
}
fn object_bytes(value: &Value, key: &str, replacement: u64) -> u64 {
    let Some(map) = value.as_object() else {
        return 2 + bytes(&key) + 1 + replacement;
    };
    let mut count = 2;
    let mut fields: u64 = 0;
    for (name, value) in map {
        count += bytes(name)
            + 1
            + if name == key {
                replacement
            } else {
                bytes(value)
            };
        fields += 1;
    }
    if !map.contains_key(key) {
        count += bytes(&key) + 1 + replacement;
        fields += 1;
    }
    count + fields.saturating_sub(1)
}
fn content_bytes(content: &Value) -> (u64, u64) {
    let Some(parts) = content.as_array() else {
        return (bytes(content), 0);
    };
    let mut count = 2 + parts.len().saturating_sub(1) as u64;
    let mut images = 0;
    for part in parts {
        count += if part["type"] == "image" {
            images += 8192;
            object_bytes(part, "image", 2)
        } else if part["type"] == "tool-result" && part["output"]["type"] == "content" {
            if let Some(files) = part["output"]["value"].as_array() {
                let mut length = 2 + files.len().saturating_sub(1) as u64;
                for file in files {
                    length += if file["mediaType"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("image/"))
                    {
                        images += 8192;
                        object_bytes(file, "data", object_bytes(&file["data"], "data", 2))
                    } else {
                        bytes(file)
                    };
                }
                object_bytes(
                    part,
                    "output",
                    object_bytes(&part["output"], "value", length),
                )
            } else {
                bytes(part)
            }
        } else {
            bytes(part)
        };
    }
    (count, images)
}
fn estimate_message(message: &Message) -> u64 {
    // Flattened metadata has the same last-key-wins behavior as to_value.
    let content = message.metadata.get("content").unwrap_or(&message.content);
    let (content, images) = content_bytes(content);
    let role = message
        .metadata
        .get("role")
        .map_or_else(|| bytes(&message.role), bytes);
    let mut length = 2 + bytes(&"role") + 1 + role + 1 + bytes(&"content") + 1 + content;
    for (key, value) in message.metadata.iter() {
        if key != "role" && key != "content" {
            length += 1 + bytes(key) + 1 + bytes(value);
        }
    }
    length.div_ceil(3) + 8 + images
}
pub fn estimate(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message).sum()
}
/// Account for a rendered prompt without copying history or inline images.
pub fn estimate_rendered(history: &[Message], checkpoint: Option<&Compaction>) -> u64 {
    let Some(c) = checkpoint else {
        return estimate(history);
    };
    let split = c.covered.min(history.len());
    let mut total = history[..split]
        .iter()
        .filter(|m| m.role == Role::System)
        .map(estimate_message)
        .sum::<u64>();
    if let Some(task) = history[..split].iter().find(|m| m.role == Role::User) {
        total += estimate_message(task);
    }
    total
        + estimate_message(&Message::text(
            Role::User,
            format!("Durable context summary:\n{}", c.summary),
        ))
        + estimate(&history[split..])
}

/// Reduction remains opt-in until live task-quality evaluation passes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContextPolicy {
    #[default]
    Preserve,
    BoundedTools,
}
pub fn apply_policy(messages: &mut [Message], policy: ContextPolicy, agent: &str) {
    if policy == ContextPolicy::Preserve {
        return;
    }
    let boundary = messages
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, m)| m.role == Role::User)
        .nth(1)
        .map_or(0, |(i, _)| i);
    for message in &mut messages[..boundary] {
        if message.role != Role::Tool {
            continue;
        }
        let Some(parts) = message.content.as_array() else {
            continue;
        };
        if !parts.iter().any(|p| {
            p["type"] == "tool-result"
                && p["output"]["type"] == "text"
                && p["output"]["value"]
                    .as_str()
                    .is_some_and(|s| s.len() > 12000)
        }) {
            continue;
        }
        for part in message.content.as_array_mut().unwrap() {
            if part["type"] != "tool-result" || part["output"]["type"] != "text" {
                continue;
            }
            let Some(text) = part["output"]["value"].as_str().filter(|s| s.len() > 12000) else {
                continue;
            };
            let mut end = 12000;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let preview = format!(
                "{}\n[Older output shortened from {} bytes. Use read_tool_output with agent_id={} call_id={} to retrieve the complete retained result.]",
                &text[..end],
                text.len(),
                agent,
                part["toolCallId"].as_str().unwrap_or("")
            );
            part["output"]["value"] = Value::String(preview);
        }
    }
}

/// Evaluate the opt-in view before deciding whether a paid summary is needed.
pub fn estimate_context(
    history: &[Message],
    checkpoint: Option<&Compaction>,
    policy: ContextPolicy,
    agent: &str,
) -> u64 {
    if policy == ContextPolicy::Preserve {
        return estimate_rendered(history, checkpoint);
    }
    let mut messages = render(history, checkpoint);
    apply_policy(&mut messages, policy, agent);
    estimate(&messages)
}
