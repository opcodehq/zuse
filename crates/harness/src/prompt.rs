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
    let boundaries = history
        .iter()
        .enumerate()
        .filter(|(_, m)| m.role == Role::User || m.role == Role::Assistant)
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    boundaries.len().checked_sub(2).map_or(0, |i| boundaries[i])
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
pub fn limits(window: u64, cap: Option<u64>, percent: u8) -> Limits {
    let window = if window == 0 { 32000 } else { window };
    let percent = if percent == 0 || percent > 100 {
        95
    } else {
        percent
    };
    Limits {
        window,
        usable: window.saturating_mul(percent as u64) / 100,
        compact_at: cap
            .filter(|v| *v > 0)
            .map_or(window * 9 / 10, |v| v.min(window * 9 / 10)),
    }
}
pub fn estimate(messages: &[Message]) -> u64 {
    messages
        .iter()
        .map(|m| {
            let mut copy = serde_json::to_value(m).unwrap_or(Value::Null);
            let mut images = 0;
            if let Some(parts) = copy["content"].as_array_mut() {
                for part in parts {
                    if part["type"] == "image" {
                        images += 8192;
                        part["image"] = Value::String(String::new());
                    }
                    if part["type"] == "tool-result" && part["output"]["type"] == "content" {
                        if let Some(files) = part["output"]["value"].as_array_mut() {
                            for f in files {
                                if f["mediaType"]
                                    .as_str()
                                    .is_some_and(|s| s.starts_with("image/"))
                                {
                                    images += 8192;
                                    f["data"]["data"] = Value::String(String::new());
                                }
                            }
                        }
                    }
                }
            }
            serde_json::to_vec(&copy).map_or(0, |v| (v.len() as u64).div_ceil(3)) + 8 + images
        })
        .sum()
}
