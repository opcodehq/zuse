//! Recognized text results are capped before entering provider events.
use serde_json::Value;
const LIMIT: usize = 256 * 1024;
pub(crate) fn text(value: &Value) -> Option<String> {
    let joined = match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| {
                (part["type"] == "text")
                    .then(|| part["text"].as_str())
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(object) => {
            if let Some(content) = object.get("content").and_then(text) {
                return Some(content);
            }
            if let Some(message) = object.get("message").and_then(Value::as_str) {
                return text(&Value::String(message.into()));
            }
            if let Some(structured) = object.get("structuredContent").filter(|v| !v.is_null()) {
                serde_json::to_string_pretty(structured).ok()?
            } else {
                return None;
            }
        }
        _ => return None,
    };
    if joined.trim().is_empty() {
        return None;
    }
    let mut end = LIMIT.min(joined.len());
    while !joined.is_char_boundary(end) {
        end -= 1;
    }
    Some(if end < joined.len() {
        format!("{}\n[Output truncated at 256 KiB.]", &joined[..end])
    } else {
        joined
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn text_blocks_exclude_images_and_preserve_utf8_at_the_limit() {
        assert_eq!(
            text(
                &json!([{"type":"image","data":"private binary"},{"type":"text","text":"Result 🙂"}])
            ),
            Some("Result 🙂".into())
        );
        let result = text(&json!("🙂".repeat(LIMIT))).unwrap();
        assert!(result.len() < LIMIT + 64);
        assert!(result.ends_with("[Output truncated at 256 KiB.]"));
        assert_eq!(
            text(&json!({"content":[{"type":"text","text":"MCP output"}]})),
            Some("MCP output".into())
        );
    }
}
