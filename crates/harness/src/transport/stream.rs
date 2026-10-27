use super::*;
use crate::{Call, Message, Role, Usage};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) const FRAME_LIMIT: usize = 16 * 1024 * 1024;
const RESPONSE_LIMIT: usize = 64 * 1024 * 1024;
/// Incremental SSE decoder. Frames are UTF-8 decoded only once complete.
#[derive(Default)]
pub(super) struct Sse {
    line: Vec<u8>,
    event: Vec<u8>,
    total: usize,
}
impl Sse {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, Error> {
        self.total = self
            .total
            .checked_add(bytes.len())
            .ok_or_else(|| failed("response_too_large"))?;
        if self.total > RESPONSE_LIMIT {
            return Err(failed("response_too_large"));
        }
        let mut result = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                if self.line.last() == Some(&b'\r') {
                    self.line.pop();
                }
                if self.line.is_empty() {
                    if !self.event.is_empty() {
                        if self.event.last() == Some(&b'\n') {
                            self.event.pop();
                        }
                        result.push(std::mem::take(&mut self.event));
                    }
                } else if self.line.starts_with(b"data:") {
                    let data = &self.line[5..];
                    let data = data.strip_prefix(b" ").unwrap_or(data);
                    if self.event.len() + data.len() + 1 > FRAME_LIMIT {
                        return Err(failed("stream_frame_too_large"));
                    }
                    self.event.extend_from_slice(data);
                    self.event.push(b'\n');
                }
                self.line.clear();
            } else {
                if self.line.len() >= FRAME_LIMIT {
                    return Err(failed("stream_frame_too_large"));
                }
                self.line.push(byte);
            }
        }
        Ok(result)
    }
    pub fn complete(&self) -> bool {
        self.line.is_empty() && self.event.is_empty()
    }
}
#[derive(Default)]
pub(super) struct Assembly {
    pub text: String,
    reasoning: String,
    items: BTreeMap<usize, Value>,
    pub usage: Value,
    pub done: bool,
    stop: Option<String>,
    pub response_id: Option<String>,
    pub first_token_ms: Option<f64>,
    pub first_content_ms: Option<f64>,
    started: Option<std::time::Instant>,
}
impl Assembly {
    pub fn new(started: std::time::Instant) -> Self {
        Self {
            started: Some(started),
            ..Default::default()
        }
    }
    fn emit(&mut self, input: &ModelInput, text: &str) -> Result<(), Error> {
        if self.text.len() + text.len() > 16 * 1024 * 1024 {
            return Err(failed("model_text_too_large"));
        }
        if !text.is_empty() {
            self.first_token_ms
                .get_or_insert_with(|| self.started.unwrap().elapsed().as_secs_f64() * 1000.);
            (input.text)(text.to_owned());
            self.text.push_str(text);
        }
        Ok(())
    }
    fn merge_usage(&mut self, usage: &Value) {
        if let Some(fields) = usage.as_object() {
            if !self.usage.is_object() {
                self.usage = json!({});
            }
            for (k, v) in fields {
                self.usage[k] = v.clone();
            }
        }
    }
    pub fn event(
        &mut self,
        input: &ModelInput,
        protocol: Protocol,
        event: Value,
    ) -> Result<(), Error> {
        if self.done {
            if matches!(event["type"].as_str(), Some("ping" | "rate_limits.updated")) {
                return Ok(());
            }
            if protocol == Protocol::GenerateContent
                && event.get("candidates").is_none()
                && event["usageMetadata"].is_object()
            {
                self.merge_usage(&event["usageMetadata"]);
                return Ok(());
            }
            return Err(failed("stream_after_completion"));
        }
        if event.get("error").is_some() || event["type"] == "error" {
            return Err(failed("provider_stream_error"));
        }
        let meaningful = match protocol {
            Protocol::Messages => {
                event["content_block"]["type"] == "tool_use"
                    || ["text", "thinking", "partial_json"]
                        .iter()
                        .any(|k| event["delta"][*k].as_str().is_some_and(|s| !s.is_empty()))
            }
            Protocol::Responses => {
                event["item"]["type"] == "function_call"
                    || event["delta"].as_str().is_some_and(|s| !s.is_empty())
            }
            Protocol::GenerateContent => event["candidates"].as_array().is_some_and(|v| {
                v.iter().any(|c| {
                    c["content"]["parts"]
                        .as_array()
                        .is_some_and(|parts| !parts.is_empty())
                })
            }),
            Protocol::ChatCompletions => event["choices"].as_array().is_some_and(|v| {
                v.iter().any(|c| {
                    c["delta"]["content"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty())
                        || c["delta"]["tool_calls"]
                            .as_array()
                            .is_some_and(|parts| !parts.is_empty())
                })
            }),
        };
        if meaningful {
            self.first_content_ms
                .get_or_insert_with(|| self.started.unwrap().elapsed().as_secs_f64() * 1000.);
        }
        match protocol {
            Protocol::Responses => match event["type"].as_str().unwrap_or("") {
                "response.created" => {
                    self.response_id = event["response"]["id"].as_str().map(str::to_owned);
                }
                "response.output_text.delta" => self.emit(
                    input,
                    event["delta"]
                        .as_str()
                        .ok_or_else(|| failed("invalid_stream_delta"))?,
                )?,
                "response.output_item.done" => {
                    let index = event["output_index"]
                        .as_u64()
                        .ok_or_else(|| failed("invalid_output_index"))?
                        as usize;
                    if index >= 256
                        || !event["item"].is_object()
                        || self.items.insert(index, event["item"].clone()).is_some()
                    {
                        return Err(failed("invalid_output_item"));
                    }
                }
                "response.completed" => {
                    let response = &event["response"];
                    if response["incomplete_details"].is_object()
                        || response["status"]
                            .as_str()
                            .is_some_and(|s| s != "completed")
                    {
                        return Err(failed("response_incomplete"));
                    }
                    if let Some(output) = response["output"].as_array() {
                        if output.len() > 256 {
                            return Err(failed("too_many_output_items"));
                        }
                        self.items = output.iter().cloned().enumerate().collect();
                    }
                    self.merge_usage(&response["usage"]);
                    self.done = true;
                }
                "response.failed" | "response.incomplete" => {
                    return Err(failed("response_incomplete"));
                }
                _ => {}
            },
            Protocol::Messages => match event["type"].as_str().unwrap_or("") {
                "message_start" => {
                    self.response_id = event["message"]["id"].as_str().map(str::to_owned);
                    self.merge_usage(&event["message"]["usage"]);
                }
                "content_block_start" => {
                    let index = event["index"]
                        .as_u64()
                        .ok_or_else(|| failed("invalid_output_index"))?
                        as usize;
                    if index >= 256
                        || !event["content_block"].is_object()
                        || self
                            .items
                            .insert(index, event["content_block"].clone())
                            .is_some()
                    {
                        return Err(failed("invalid_output_item"));
                    }
                    if let Some(text) = event["content_block"]["text"].as_str() {
                        self.emit(input, text)?;
                    }
                }
                "content_block_delta" => {
                    let index = event["index"]
                        .as_u64()
                        .ok_or_else(|| failed("invalid_output_index"))?
                        as usize;
                    let delta = &event["delta"];
                    let (field, value) = match delta["type"].as_str() {
                        Some("text_delta") => {
                            let text = delta["text"]
                                .as_str()
                                .ok_or_else(|| failed("invalid_stream_delta"))?;
                            self.emit(input, text)?;
                            ("text", text)
                        }
                        Some("thinking_delta") => (
                            "thinking",
                            delta["thinking"]
                                .as_str()
                                .ok_or_else(|| failed("invalid_stream_delta"))?,
                        ),
                        Some("signature_delta") => (
                            "signature",
                            delta["signature"]
                                .as_str()
                                .ok_or_else(|| failed("invalid_stream_delta"))?,
                        ),
                        Some("input_json_delta") => (
                            "zuseArguments",
                            delta["partial_json"]
                                .as_str()
                                .ok_or_else(|| failed("invalid_stream_delta"))?,
                        ),
                        _ => return Err(failed("unsupported_stream_delta")),
                    };
                    let block = self
                        .items
                        .get_mut(&index)
                        .ok_or_else(|| failed("unknown_content_block"))?;
                    // Parameterless calls may send only an empty JSON delta.
                    // Preserve initial input, but still validate the block index.
                    if field != "zuseArguments" || !value.is_empty() {
                        append(block, field, value, "content_block_too_large")?;
                    }
                }
                "message_delta" => {
                    self.merge_usage(&event["usage"]);
                    self.stop = event["delta"]["stop_reason"].as_str().map(str::to_owned);
                }
                "message_stop" => {
                    if !matches!(
                        self.stop.as_deref(),
                        Some("end_turn" | "tool_use" | "stop_sequence")
                    ) {
                        return Err(failed("response_incomplete"));
                    }
                    self.done = true;
                }
                _ => {}
            },
            Protocol::GenerateContent => {
                self.merge_usage(&event["usageMetadata"]);
                if let Some(candidates) = event["candidates"].as_array() {
                    if candidates.len() > 1 {
                        return Err(failed("unsupported_multiple_candidates"));
                    }
                    for candidate in candidates {
                        if let Some(parts) = candidate["content"]["parts"].as_array() {
                            for part in parts {
                                if let Some(text) =
                                    part["text"].as_str().filter(|_| part["thought"] != true)
                                {
                                    self.emit(input, text)?;
                                }
                                if self.items.len() >= 256 {
                                    return Err(failed("too_many_output_items"));
                                }
                                self.items.insert(self.items.len(), part.clone());
                            }
                        }
                        if let Some(reason) = candidate["finishReason"].as_str() {
                            if reason != "STOP" {
                                return Err(failed("response_incomplete"));
                            }
                            self.done = true;
                        }
                    }
                }
            }
            Protocol::ChatCompletions => {
                self.merge_usage(&event["usage"]);
                if let Some(choices) = event["choices"].as_array() {
                    if choices.len() > 1 {
                        return Err(failed("unsupported_multiple_candidates"));
                    }
                    for choice in choices {
                        let delta = &choice["delta"];
                        if let Some(text) = delta["content"].as_str() {
                            self.emit(input, text)?;
                        }
                        if let Some(reasoning) = delta["reasoning_content"].as_str() {
                            if self.reasoning.len() + reasoning.len() > 16 * 1024 * 1024 {
                                return Err(failed("reasoning_too_large"));
                            }
                            self.reasoning.push_str(reasoning);
                        }
                        if let Some(calls) = delta["tool_calls"].as_array() {
                            for call in calls {
                                let index = call["index"]
                                    .as_u64()
                                    .ok_or_else(|| failed("invalid_output_index"))?
                                    as usize;
                                if index >= 128 {
                                    return Err(failed("too_many_tool_calls"));
                                }
                                let item=self.items.entry(index).or_insert_with(||json!({"id":"","type":"function_call","name":"","arguments":""}));
                                for (key, value) in [
                                    ("id", &call["id"]),
                                    ("name", &call["function"]["name"]),
                                    ("arguments", &call["function"]["arguments"]),
                                ] {
                                    if let Some(next) = value.as_str() {
                                        append(item, key, next, "tool_arguments_too_large")?;
                                    }
                                }
                            }
                        }
                        if let Some(reason) = choice["finish_reason"].as_str() {
                            if !matches!(reason, "stop" | "tool_calls") {
                                return Err(failed("response_incomplete"));
                            }
                            self.stop = Some(reason.into());
                        }
                    }
                }
            }
        }
        Ok(())
    }
    pub fn partial_usage(&self, protocol: Protocol) -> Option<Usage> {
        self.usage.as_object().filter(|v| !v.is_empty()).map(|_| {
            let mut usage = usage(protocol, &self.usage);
            usage.first_token_ms = self.first_token_ms;
            usage
        })
    }
    pub fn end_chat(&mut self) -> Result<(), Error> {
        if !matches!(self.stop.as_deref(), Some("stop" | "tool_calls")) {
            return Err(failed("stream_incomplete"));
        }
        self.done = true;
        Ok(())
    }
    pub fn finish(
        mut self,
        input: &ModelInput,
        credential: &Credential,
        request_id: String,
    ) -> Result<ModelOutput, Error> {
        if !self.done {
            return Err(failed("stream_incomplete"));
        }
        let protocol = credential.route.protocol;
        let mut content = Vec::new();
        let mut calls = Vec::new();
        if protocol == Protocol::ChatCompletions && !self.reasoning.is_empty() {
            content.push(json!({"type":"reasoning","text":self.reasoning}));
        }
        if protocol == Protocol::ChatCompletions && !self.text.is_empty() {
            content.push(json!({"type":"text","text":self.text}));
        }
        let mut final_text = String::new();
        for item in self.items.values_mut() {
            if protocol == Protocol::Responses
                && !matches!(
                    item["type"].as_str(),
                    Some("message" | "reasoning" | "function_call")
                )
            {
                return Err(failed("unsupported_output_item"));
            }
            if protocol == Protocol::Responses
                && item["type"] == "function_call"
                && item["namespace"]
                    .as_str()
                    .is_some_and(|n| Some(n) != credential.route.tool_namespace.as_deref())
            {
                return Err(failed("unexpected_tool_namespace"));
            }
            match protocol {
                Protocol::Responses if item["type"] == "message" => {
                    for block in item["content"]
                        .as_array()
                        .ok_or_else(|| failed("invalid_message_output"))?
                    {
                        if block["type"] != "output_text" {
                            return Err(failed("unsupported_message_output"));
                        }
                        let text = block["text"]
                            .as_str()
                            .ok_or_else(|| failed("invalid_message_output"))?;
                        final_text.push_str(text);
                        let mut extra = block.as_object().unwrap().clone();
                        extra.remove("text");
                        content.push(
                            json!({"type":"text","text":text,"providerOptions":{"native":extra}}),
                        );
                    }
                }
                Protocol::Responses if item["type"] == "reasoning" => content
                    .push(json!({"type":"reasoning","text":"","providerOptions":{"native":item}})),
                Protocol::Messages if item["type"] == "text" => {
                    let text = item["text"]
                        .as_str()
                        .ok_or_else(|| failed("invalid_message_output"))?;
                    final_text.push_str(text);
                    let mut extra = item.as_object().unwrap().clone();
                    extra.remove("text");
                    content.push(
                        json!({"type":"text","text":text,"providerOptions":{"native":extra}}),
                    );
                }
                Protocol::Messages
                    if item["type"] == "thinking" || item["type"] == "redacted_thinking" =>
                {
                    let mut extra = item.as_object().unwrap().clone();
                    let text = extra.remove("thinking").unwrap_or(json!(""));
                    content.push(
                        json!({"type":"reasoning","text":text,"providerOptions":{"native":extra}}),
                    );
                }
                Protocol::GenerateContent if item.get("text").is_some() => {
                    let mut extra = item.as_object().unwrap().clone();
                    let text = extra.remove("text").unwrap();
                    if item["thought"] != true {
                        final_text.push_str(
                            text.as_str()
                                .ok_or_else(|| failed("invalid_message_output"))?,
                        );
                    }
                    content.push(json!({"type":if item["thought"]==true{"reasoning"}else{"text"},"text":text,"providerOptions":{"native":extra}}));
                }
                _ => {}
            }
            let call = match protocol {
                Protocol::Responses if item["type"] == "function_call" => Some((
                    item["call_id"].as_str().unwrap_or(""),
                    item["name"].as_str().unwrap_or(""),
                    serde_json::from_str(item["arguments"].as_str().unwrap_or("{}"))
                        .map_err(|_| failed("invalid_tool_arguments"))?,
                )),
                Protocol::Messages if item["type"] == "tool_use" => {
                    if let Some(arguments) = item.as_object_mut().unwrap().remove("zuseArguments") {
                        item["input"] = serde_json::from_str(arguments.as_str().unwrap_or(""))
                            .map_err(|_| failed("invalid_tool_arguments"))?;
                    }
                    Some((
                        item["id"].as_str().unwrap_or(""),
                        item["name"].as_str().unwrap_or(""),
                        item["input"].clone(),
                    ))
                }
                Protocol::GenerateContent if item.get("functionCall").is_some() => {
                    if item["functionCall"]["id"].as_str().is_none() {
                        item["functionCall"]["id"] = json!(uuid::Uuid::new_v4().to_string());
                    }
                    let call = &item["functionCall"];
                    Some((
                        call["id"].as_str().unwrap_or(""),
                        call["name"].as_str().unwrap_or(""),
                        call["args"].clone(),
                    ))
                }
                Protocol::ChatCompletions => Some((
                    item["id"].as_str().unwrap_or(""),
                    item["name"].as_str().unwrap_or(""),
                    serde_json::from_str(item["arguments"].as_str().unwrap_or("{}"))
                        .map_err(|_| failed("invalid_tool_arguments"))?,
                )),
                _ => None,
            };
            if let Some((id, name, arguments)) = call {
                if id.is_empty()
                    || !input.tools.contains_key(name)
                    || !arguments.is_object()
                    || calls.iter().any(|c: &Call| c.id == id)
                {
                    return Err(failed("invalid_tool_call"));
                }
                let mut extra = item.as_object().cloned().unwrap_or_default();
                for key in ["arguments", "input", "functionCall"] {
                    extra.remove(key);
                }
                content.push(json!({"type":"tool-call","toolCallId":id,"toolName":name,"input":arguments,"providerOptions":{"native":extra}}));
                calls.push(Call {
                    id: id.into(),
                    name: name.into(),
                    input: arguments,
                });
            }
        }
        if protocol != Protocol::ChatCompletions {
            if final_text.is_empty() && !self.text.is_empty() {
                content.insert(0, json!({"type":"text","text":self.text}));
            } else if !self.text.is_empty() && self.text != final_text {
                return Err(failed("stream_text_mismatch"));
            } else if self.text.is_empty() && !final_text.is_empty() {
                self.emit(input, &final_text)?;
            }
        }
        if content.is_empty() || (self.text.is_empty() && calls.is_empty()) {
            return Err(failed("empty_model_response"));
        }
        let mut message = Message {
            role: Role::Assistant,
            content: json!(content).into(),
            metadata: Default::default(),
        };
        {
            message
                .metadata
                .insert("providerOptions".into(), json!({"protocol":protocol}));
            message.metadata.insert(
                "zuseOrigin".into(),
                json!({"connection":credential.connection_id,"model":input.model,"route":super::request::route_key(&credential.route),"authorizationGeneration":credential.authorization_generation}),
            );
        }
        let mut usage = usage(protocol, &self.usage);
        usage.connection_id = credential.connection_id.clone();
        usage.request_id = Some(request_id);
        usage.purpose = input.purpose;
        usage.billing_route = Some(
            if credential.route.subscription {
                "subscription"
            } else {
                "api"
            }
            .into(),
        );
        usage.prefix_fingerprint = Some(super::request::fingerprint(input, credential));
        usage.first_token_ms = self.first_token_ms;
        Ok(ModelOutput {
            messages: vec![message],
            calls,
            text: self.text,
            usage,
        })
    }
}
fn number(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}
pub(super) fn usage(protocol: Protocol, value: &Value) -> Usage {
    let (total, cached, written, output, reasoning) = match protocol {
        Protocol::Messages => {
            let uncached = number(value, "input_tokens");
            let read = number(value, "cache_read_input_tokens");
            let write = number(value, "cache_creation_input_tokens");
            (
                uncached
                    .and_then(|n| n.checked_add(read.unwrap_or(0)))
                    .and_then(|n| n.checked_add(write.unwrap_or(0))),
                read,
                write,
                number(value, "output_tokens"),
                number(&value["output_tokens_details"], "thinking_tokens"),
            )
        }
        Protocol::GenerateContent => (
            number(value, "promptTokenCount"),
            number(value, "cachedContentTokenCount"),
            None,
            number(value, "candidatesTokenCount")
                .map(|n| n + number(value, "thoughtsTokenCount").unwrap_or(0)),
            number(value, "thoughtsTokenCount"),
        ),
        Protocol::Responses => (
            number(value, "input_tokens"),
            number(&value["input_tokens_details"], "cached_tokens"),
            number(&value["input_tokens_details"], "cache_write_tokens"),
            number(value, "output_tokens"),
            number(&value["output_tokens_details"], "reasoning_tokens"),
        ),
        Protocol::ChatCompletions => (
            number(value, "prompt_tokens"),
            number(&value["prompt_tokens_details"], "cached_tokens"),
            None,
            number(value, "completion_tokens"),
            number(&value["completion_tokens_details"], "reasoning_tokens"),
        ),
    };
    Usage {
        input_tokens: total.unwrap_or(0),
        output_tokens: output.unwrap_or(0),
        cached_tokens: cached,
        cache_write_tokens: written,
        uncached_input_tokens: if protocol == Protocol::Messages {
            number(value, "input_tokens")
        } else {
            total
                .zip(cached)
                .map(|(t, c)| t.saturating_sub(c).saturating_sub(written.unwrap_or(0)))
        },
        reasoning_tokens: reasoning,
        ..Default::default()
    }
}

fn append(value: &mut Value, key: &str, next: &str, code: &str) -> Result<(), Error> {
    let map = value
        .as_object_mut()
        .ok_or_else(|| failed("invalid_content_block"))?;
    let value = map.entry(key).or_insert_with(|| json!(""));
    let Value::String(text) = value else {
        return Err(failed("invalid_stream_delta"));
    };
    if text.len() + next.len() > FRAME_LIMIT {
        return Err(failed(code));
    }
    text.push_str(next);
    Ok(())
}
