//! INTERNAL PILOT observations, not an execution channel. Default off at the harness.
//! Shape: native0.166 RawResponseItemCompletedNotification / ResponseItem.
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use zuse_protocol::{AgentEvent, ToolCall};
const INPUT_LIMIT: usize = 64 * 1024;
const OUTPUT_LIMIT: usize = 256 * 1024;
#[derive(Default)]
pub(super) struct RawTools {
    active: HashMap<(String, String), String>,
    completed: VecDeque<(String, String)>,
    warned: bool,
}
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) if s.len() <= OUTPUT_LIMIT => Some(s.clone()),
        Value::String(_) => Some("Code-mode output omitted: observation exceeds 256 KiB.".into()),
        Value::Array(parts) => {
            let mut result = String::new();
            for part in parts {
                if !matches!(part["type"].as_str(), Some("input_text" | "text")) {
                    continue;
                }
                let Some(part) = part["text"].as_str() else {
                    continue;
                };
                if result.len().saturating_add(part.len()).saturating_add(1) > OUTPUT_LIMIT {
                    return Some("Code-mode output omitted: observation exceeds 256 KiB.".into());
                }
                if !result.is_empty() {
                    result.push('\n')
                }
                result.push_str(part);
            }
            (!result.is_empty()).then_some(result)
        }
        _ => None,
    }
}
fn failed(output: Option<&str>) -> bool {
    output.is_some_and(|text| {
        let text = text.trim_start();
        text.starts_with("Error:")
            || text.starts_with("Script failed\n")
            || text == "Script failed"
            || text.starts_with("failed to spawn code-mode host")
    })
}
impl RawTools {
    pub(super) fn map(&mut self, params: &Value, current_turn: Option<&str>) -> Vec<AgentEvent> {
        let Some(turn) = params["turnId"]
            .as_str()
            .filter(|id| Some(*id) == current_turn && !id.is_empty() && id.len() <= 512)
        else {
            return vec![];
        };
        self.active
            .retain(|(active_turn, _), _| active_turn == turn);
        let item = &params["item"];
        let Some(id) = item["call_id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 512)
        else {
            return vec![];
        };
        let key = (turn.to_owned(), id.to_owned());
        if self.completed.contains(&key) {
            return vec![];
        }
        let kind = item["type"].as_str().unwrap_or_default();
        if matches!(kind, "custom_tool_call" | "function_call") {
            if self.active.contains_key(&key) {
                return vec![];
            }
            let namespace = item["namespace"].as_str();
            if namespace.is_some_and(|ns| ns != "functions") {
                return vec![];
            }
            let name = item["name"].as_str().unwrap_or_default();
            let mut input = json!({});
            match (kind, name) {
                ("custom_tool_call", "exec") => {
                    let Some(code) = item["input"].as_str() else {
                        return vec![];
                    };
                    // Omit whole oversized data: slicing could expose a partial secret
                    // before the shared presentation redactor sees the original value.
                    if code.len() <= INPUT_LIMIT {
                        input["code"] = json!(code)
                    } else {
                        input["inputOmitted"] = json!(true);
                        input["inputBytes"] = json!(code.len());
                    }
                }
                ("function_call", "wait") => {
                    let Some(args) = item["arguments"]
                        .as_str()
                        .filter(|s| s.len() <= INPUT_LIMIT)
                    else {
                        return vec![];
                    };
                    let Ok(args) = serde_json::from_str::<Value>(args) else {
                        return vec![];
                    };
                    if !args["cell_id"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty() && s.len() <= 512)
                    {
                        return vec![];
                    }
                    input["arguments"] = args;
                }
                _ => return vec![],
            }
            if self.active.len() >= 256 {
                if !self.warned {
                    self.warned = true;
                    tracing::warn!(
                        "Internal Codex tool observation capacity reached; additional raw wrappers omitted"
                    );
                }
                return vec![];
            }
            let mut context = json!({"turnId":turn,"callId":id});
            for (source, target) in [
                (params.get("threadId"), "threadId"),
                (item.get("id"), "responseItemId"),
                (item.get("namespace"), "namespace"),
            ] {
                if let Some(value) = source.and_then(Value::as_str).filter(|s| s.len() <= 512) {
                    context[target] = json!(value)
                }
            }
            input["context"] = context;
            self.active.insert(key, name.into());
            return vec![AgentEvent::ToolCall {
                id: id.into(),
                call: ToolCall::Unknown {
                    name: name.into(),
                    input: Some(input),
                },
            }];
        }
        if matches!(kind, "custom_tool_call_output" | "function_call_output") {
            let Some(name) = self.active.get(&key) else {
                // No historical/orphan tool output is promoted into a new public
                // transcript. In particular do not republish cold native history.
                return vec![];
            };
            if (name == "exec") != (kind == "custom_tool_call_output")
                || item["name"].as_str().is_some_and(|actual| actual != name)
            {
                return vec![];
            }
            let output = item.get("output").and_then(text);
            let is_error = failed(output.as_deref());
            self.active.remove(&key);
            if self.completed.len() == 512 {
                self.completed.pop_front();
            }
            self.completed.push_back(key);
            return vec![AgentEvent::ToolResult {
                id: id.into(),
                is_error,
                output,
                diff: None,
            }];
        }
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(item: Value) -> Value {
        json!({"threadId":"parent","turnId":"current","item":item})
    }
    fn call() -> Value {
        frame(
            json!({"type":"custom_tool_call","call_id":"outer","id":"response","name":"exec","input":"await tools.exec_command({cmd: 'echo private-token'});","encrypted_content":"must-not-copy","internal_chat_message_metadata_passthrough":{"hidden":"must-not-copy"}}),
        )
    }
    #[test]
    fn internal_raw_tools_are_current_paired_deduplicated_and_presentation_redacted() {
        let mut raw = RawTools::default();
        let original = call();
        assert!(raw.map(&original, None).is_empty());
        assert!(raw.map(&original, Some("another")).is_empty());
        let mut event = raw.map(&original, Some("current")).pop().unwrap();
        let before = serde_json::to_string(&event).unwrap();
        assert!(before.contains("private-token"));
        assert!(!before.contains("must-not-copy"));
        let secrets = std::sync::Arc::new(crate::credentials::redaction::Secrets::default());
        secrets.record("private-token").unwrap();
        crate::credentials::redaction::Events::new(secrets.clone()).mask_event(&mut event);
        assert!(
            !serde_json::to_string(&event)
                .unwrap()
                .contains("private-token")
        );
        assert!(
            original["item"]["input"]
                .as_str()
                .unwrap()
                .contains("private-token"),
            "private native operational input untouched"
        );
        assert!(raw.map(&original, Some("current")).is_empty());
        let output = frame(
            json!({"type":"custom_tool_call_output","call_id":"outer","output":[{"type":"input_image","image_url":"private-image"},{"type":"input_text","text":"Script completed\nprivate-token"}]}),
        );
        let mut result = raw.map(&output, Some("current")).pop().unwrap();
        assert!(
            matches!(&result,AgentEvent::ToolResult{is_error:false,output:Some(t),..} if t.contains("private-token") && !t.contains("private-image"))
        );
        crate::credentials::redaction::Events::new(secrets).mask_event(&mut result);
        assert!(
            !serde_json::to_string(&result)
                .unwrap()
                .contains("private-token")
        );
        assert!(raw.map(&output, Some("current")).is_empty());
        assert!(raw.map(&original, Some("current")).is_empty());
    }
    #[test]
    fn internal_raw_tools_ignore_unrelated_history_and_bound_observations() {
        let mut raw = RawTools::default();
        for item in [
            json!({"type":"message","call_id":"x","name":"exec","input":"private"}),
            json!({"type":"reasoning","call_id":"x","name":"exec","input":"private"}),
            json!({"type":"custom_tool_call","call_id":"x","name":"exec","namespace":"untrusted","input":"private"}),
            json!({"type":"custom_tool_call_output","call_id":"orphan","output":"old-secret"}),
            json!({"type":"function_call","call_id":"generic-wait","name":"wait","arguments":"{}"}),
        ] {
            assert!(raw.map(&frame(item), Some("current")).is_empty());
        }
        let mut large = call();
        large["item"]["input"] = json!("x".repeat(INPUT_LIMIT + 1));
        let event = raw.map(&large, Some("current")).pop().unwrap();
        assert!(
            matches!(event,AgentEvent::ToolCall{call:ToolCall::Unknown{input:Some(ref v),..},..} if v["inputOmitted"]==true && v.get("code").is_none())
        );
        let result=raw.map(&frame(json!({"type":"custom_tool_call_output","call_id":"outer","output":"x".repeat(OUTPUT_LIMIT+1)})),Some("current"));
        assert!(matches!(&result[0],AgentEvent::ToolResult{output:Some(t),..} if t.len()<100));
        let wait = frame(
            json!({"type":"function_call","call_id":"wait-id","name":"wait","arguments":"{\"cell_id\":\"cell-1\",\"yield_time_ms\":100}"}),
        );
        assert!(
            matches!(&raw.map(&wait,Some("current"))[0],AgentEvent::ToolCall{call:ToolCall::Unknown{name,..},..} if name=="wait")
        );
        assert!(matches!(&raw.map(&frame(json!({"type":"function_call_output","call_id":"wait-id","output":"Script failed\nScript error:\nfailed"})),Some("current"))[0],AgentEvent::ToolResult{is_error:true,..}));
    }
}
