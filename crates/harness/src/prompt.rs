//! Prompt accounting and compaction boundaries preserve complete tool exchanges.
use crate::{Compaction, Message, Messages, Role};
use serde::Serialize;
use serde_json::Value;
use std::{borrow::Cow, io};

pub fn render(history: &[Message], checkpoint: Option<&Compaction>) -> Messages {
    context(history, checkpoint).map(Cow::into_owned).collect()
}

// Rendering and accounting must retain exactly the same context. Borrow the
// history for accounting; model requests share the immutable message contents.
fn context<'a>(
    history: &'a [Message],
    checkpoint: Option<&Compaction>,
) -> impl Iterator<Item = Cow<'a, Message>> {
    let split = checkpoint.map_or(0, |c| c.covered.min(history.len()));
    let task = history[..split].iter().find(|m| m.role == Role::User);
    let summary = checkpoint.map(|c| {
        Message::text(
            Role::User,
            format!("Durable context summary:\n{}", c.summary),
        )
    });
    history[..split]
        .iter()
        .filter(|m| m.role == Role::System)
        .chain(task)
        .map(Cow::Borrowed)
        .chain(summary.map(Cow::Owned))
        .chain(history[split..].iter().map(Cow::Borrowed))
}

pub fn estimate_context(history: &[Message], checkpoint: Option<&Compaction>) -> u64 {
    context(history, checkpoint)
        .map(|m| estimate_message(&m))
        .sum()
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
pub fn complete_prefix(history: &[Message]) -> Messages {
    let end = history
        .iter()
        .rposition(|message| message.has_tool_calls())
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
pub fn estimate(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|message| estimate_message(message))
        .sum()
}

// Count the same JSON bytes as the wire estimate without copying messages or
// scanning inline image payloads that are charged at a fixed token allowance.
struct ByteCount(usize);
impl io::Write for ByteCount {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn json_len(value: &(impl Serialize + ?Sized)) -> usize {
    let mut count = ByteCount(0);
    // Only JSON values and strings reach this helper; the writer cannot fail.
    serde_json::to_writer(&mut count, value).expect("JSON byte counting is infallible");
    count.0
}

fn replaced_len(value: &Value, key: &str, replacement: usize) -> usize {
    let Some(object) = value.as_object() else {
        return if value.is_null() {
            2 + json_len(key) + 1 + replacement
        } else {
            json_len(value)
        };
    };
    let missing = !object.contains_key(key);
    let fields = object.len() + usize::from(missing);
    2 + fields.saturating_sub(1)
        + object
            .iter()
            .map(|(name, value)| {
                json_len(name)
                    + 1
                    + if name == key {
                        replacement
                    } else {
                        json_len(value)
                    }
            })
            .sum::<usize>()
        + if missing {
            json_len(key) + 1 + replacement
        } else {
            0
        }
}

fn part_len(part: &Value, images: &mut u64) -> usize {
    if part["type"] == "image" {
        *images += 8192;
        return replaced_len(part, "image", 2);
    }
    if part["type"] == "tool-result"
        && matches!(part["output"]["type"].as_str(), Some("content" | "mixed"))
        && let Some(files) = part["output"][if part["output"]["type"] == "mixed" {
            "parts"
        } else {
            "value"
        }]
        .as_array()
    {
        let length = 2
            + files.len().saturating_sub(1)
            + files
                .iter()
                .map(|file| {
                    if file["mediaType"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("image/"))
                    {
                        *images += 8192;
                        replaced_len(file, "data", replaced_len(&file["data"], "data", 2))
                    } else {
                        json_len(file)
                    }
                })
                .sum::<usize>();
        return replaced_len(
            part,
            "output",
            replaced_len(
                &part["output"],
                if part["output"]["type"] == "mixed" {
                    "parts"
                } else {
                    "value"
                },
                length,
            ),
        );
    }
    json_len(part)
}

fn estimate_message(message: &Message) -> u64 {
    let mut images = 0;
    // Flattened metadata has the same last-field-wins behavior as to_value.
    let content = message.metadata.get("content").unwrap_or(&message.content);
    let content_len = content.as_array().map_or_else(
        || json_len(content),
        |parts| {
            2 + parts.len().saturating_sub(1)
                + parts
                    .iter()
                    .map(|part| part_len(part, &mut images))
                    .sum::<usize>()
        },
    );
    let role_len = message
        .metadata
        .get("role")
        .map_or_else(|| json_len(&message.role), json_len);
    let extra = message
        .metadata
        .iter()
        .filter(|(key, _)| key.as_str() != "role" && key.as_str() != "content")
        .map(|(key, value)| 1 + json_len(key) + 1 + json_len(value))
        .sum::<usize>();
    (20 + role_len + content_len + extra).div_ceil(3) as u64 + 8 + images
}

/// Backwards-compatible borrowed estimate used by native journal tooling.
pub fn estimate_rendered(history: &[Message], checkpoint: Option<&Compaction>) -> u64 {
    estimate_context(history, checkpoint)
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
pub fn estimate_policy_context(
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
