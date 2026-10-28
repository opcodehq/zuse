use super::*;
use crate::{Message, Role};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) fn cache_key(input: &ModelInput, credential: &Credential) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                "zuse-harness-v1",
                &credential.connection_id,
                credential.authorization_generation,
                &input.model,
                &input.root_id
            ))
            .unwrap()
        )
    )
}
pub(super) fn fingerprint(input: &ModelInput, credential: &Credential) -> String {
    let system = input
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .collect::<Vec<_>>();
    let mut hash = Sha256::new();
    hash.update(
        serde_json::to_vec(&(
            &input.model,
            &input.reasoning,
            &*input.tools,
            &credential.route,
            system,
        ))
        .unwrap(),
    );
    if let Some(attribution) = credential.route.client_attribution() {
        hash.update(attribution);
    }
    format!("{:x}", hash.finalize())
}
pub(super) fn route_key(route: &Route) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(route).expect("route serializes"))
    )
}
fn image(part: &Value) -> Result<(&str, &str), Error> {
    let mime = part["mediaType"]
        .as_str()
        .ok_or_else(|| failed("invalid_image"))?;
    let data = part["image"]
        .as_str()
        .ok_or_else(|| failed("invalid_image"))?;
    if !matches!(
        mime,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    ) {
        return Err(failed("unsupported_image"));
    }
    Ok((mime, data))
}
fn tool_parts(output: &Value) -> Option<&Vec<Value>> {
    match output["type"].as_str() {
        Some("content") => output["value"].as_array(),
        Some("mixed") => output["parts"].as_array(),
        _ => None,
    }
}
fn tool_images(output: &Value) -> bool {
    tool_parts(output).is_some_and(|parts| {
        parts
            .iter()
            .any(|p| matches!(p["type"].as_str(), Some("file" | "image")))
    })
}
fn text_result(output: &Value) -> String {
    if output["type"] == "mixed" {
        return tool_parts(output)
            .into_iter()
            .flatten()
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
    }
    if output["type"] == "text" {
        output["value"].as_str().unwrap_or("").to_owned()
    } else {
        output.to_string()
    }
}
fn role(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}
fn parts(message: &Message) -> &[Value] {
    if message.content.is_string() {
        std::slice::from_ref(&*message.content)
    } else {
        message
            .content
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    }
}
fn kind(part: &Value) -> Option<&str> {
    if part.is_string() {
        Some("text")
    } else {
        part["type"].as_str()
    }
}
fn text(part: &Value) -> &Value {
    if part.is_string() {
        part
    } else {
        &part["text"]
    }
}
fn native(message: &Message, input: &ModelInput, credential: &Credential) -> Option<Vec<Value>> {
    let origin = message.metadata.get("zuseOrigin")?;
    if origin["connection"] != credential.connection_id
        || origin["model"] != input.model
        || origin["route"] != route_key(&credential.route)
        || origin["authorizationGeneration"] != credential.authorization_generation
    {
        return None;
    }
    let options = message.metadata.get("providerOptions")?;
    if options["protocol"] != serde_json::to_value(credential.route.protocol).ok()? {
        return None;
    }
    let mut items = Vec::new();
    for part in message.content.as_array()? {
        let mut extra = part["providerOptions"]["native"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let item = match credential.route.protocol {
            Protocol::Responses => match part["type"].as_str()? {
                "text" => {
                    extra.insert("type".into(), json!("output_text"));
                    extra.insert("text".into(), part["text"].clone());
                    json!({"type":"message","role":"assistant","content":[extra]})
                }
                "reasoning" => Value::Object(extra),
                "tool-call" => {
                    extra.insert("type".into(), json!("function_call"));
                    extra.insert("call_id".into(), part["toolCallId"].clone());
                    extra.insert("name".into(), part["toolName"].clone());
                    extra.insert("arguments".into(), json!(part["input"].to_string()));
                    Value::Object(extra)
                }
                _ => return None,
            },
            Protocol::Messages => match part["type"].as_str()? {
                "text" => {
                    extra.insert("type".into(), json!("text"));
                    extra.insert("text".into(), part["text"].clone());
                    Value::Object(extra)
                }
                "reasoning" => {
                    if extra.get("type").and_then(Value::as_str) != Some("redacted_thinking") {
                        extra.insert("type".into(), json!("thinking"));
                        extra.insert("thinking".into(), part["text"].clone());
                    }
                    Value::Object(extra)
                }
                "tool-call" => {
                    json!({"type":"tool_use","id":part["toolCallId"],"name":part["toolName"],"input":part["input"]})
                }
                _ => return None,
            },
            Protocol::GenerateContent => match part["type"].as_str()? {
                "text" | "reasoning" => {
                    extra.insert("text".into(), part["text"].clone());
                    if part["type"] == "reasoning" {
                        extra.insert("thought".into(), json!(true));
                    }
                    Value::Object(extra)
                }
                "tool-call" => {
                    extra.insert("functionCall".into(),json!({"id":part["toolCallId"],"name":part["toolName"],"args":part["input"]}));
                    Value::Object(extra)
                }
                _ => return None,
            },
            Protocol::ChatCompletions => return None,
        };
        items.push(item);
    }
    Some(items)
}
pub(super) fn body(input: &ModelInput, credential: &Credential) -> Result<Value, Error> {
    let route = &credential.route;
    if input.messages.iter().any(|m| {
        (!m.content.is_string() && !m.content.is_array())
            || m.content
                .as_array()
                .is_some_and(|p| p.iter().any(|part| !part.is_object()))
    }) {
        return Err(failed("invalid_message_content"));
    }
    for message in input.messages.iter() {
        for part in parts(message) {
            match kind(part) {
                Some("text") if !text(part).is_string() => return Err(failed("invalid_text_part")),
                Some("tool-call")
                    if !part["toolCallId"].is_string()
                        || !part["toolName"].is_string()
                        || !part["input"].is_object() =>
                {
                    return Err(failed("invalid_tool_call_history"));
                }
                Some("tool-result") if !part["toolCallId"].is_string() => {
                    return Err(failed("invalid_tool_result_history"));
                }
                _ => {}
            }
        }
    }
    if let (Some(level), Some(allowed)) = (&input.reasoning, &route.allowed_reasoning)
        && !allowed.contains(level)
    {
        return Err(failed("unsupported_reasoning_level"));
    }
    if input.reasoning.is_some() && !route.capabilities.reasoning {
        return Err(failed("unsupported_reasoning"));
    }
    if !input.tools.is_empty() && !route.capabilities.tools {
        return Err(failed("unsupported_tools"));
    }
    if input.messages.iter().any(|m| {
        parts(m).iter().any(|p| {
            p["type"] == "image" || (p["type"] == "tool-result" && tool_images(&p["output"]))
        })
    }) && !route.capabilities.images
    {
        return Err(failed("unsupported_images"));
    }
    let system = input
        .messages
        .iter()
        .filter(|m| m.role == Role::System)
        .map(|m| {
            m.content
                .as_str()
                .ok_or_else(|| failed("invalid_system_content"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .join("\n\n");
    match route.protocol {
        Protocol::Responses => {
            let mut history = Vec::new();
            for message in input.messages.iter().filter(|m| m.role != Role::System) {
                if let Some(items) = native(message, input, credential) {
                    history.extend(items);
                    continue;
                }
                let mut content = Vec::new();
                for part in parts(message).iter() {
                    match kind(part) {
                        Some("text") => content.push(json!({"type":if message.role==Role::Assistant {"output_text"} else {"input_text"},"text":text(part)})),
                        Some("image") => { let (mime,data)=image(part)?; content.push(json!({"type":"input_image","image_url":format!("data:{mime};base64,{data}")})); },
                        Some("tool-call") => { let mut call=json!({"type":"function_call","call_id":part["toolCallId"],"name":part["toolName"],"arguments":part["input"].to_string()}); if let Some(namespace)=&route.tool_namespace{call["namespace"]=json!(namespace);} history.push(call); },
                        Some("tool-result") => {
                            let output = if let Some(files)=tool_parts(&part["output"]) {
                                Value::Array(files.iter().map(|f| if f["type"]=="text" {json!({"type":"input_text","text":f["text"]})} else {json!({"type":"input_image","image_url":format!("data:{};base64,{}",f["mediaType"].as_str().unwrap_or(""),f["data"]["data"].as_str().unwrap_or(""))})}).collect())
                            } else { json!(text_result(&part["output"])) };
                            history.push(json!({"type":"function_call_output","call_id":part["toolCallId"],"output":output}));
                        },
                        Some("reasoning") => {},
                        _ => return Err(failed("unsupported_message_part")),
                    }
                }
                if !content.is_empty() {
                    history.push(json!({"role":role(message.role),"content":content}));
                }
            }
            let functions=input.tools.iter().map(|(name,t)|json!({"type":"function","name":name,"description":t.description,"parameters":t.parameters,"strict":false})).collect::<Vec<_>>();
            let tools = if route.tool_namespace.is_some() && !functions.is_empty() {
                vec![
                    json!({"type":"namespace","name":route.tool_namespace,"description":"Local coding and collaboration tools","tools":functions}),
                ]
            } else {
                functions
            };
            let mut body = json!({"model":input.model,"input":history,"instructions":system,"tools":tools,"store":false,"stream":true});
            if route.capabilities.prompt_cache {
                body["prompt_cache_key"] = json!(cache_key(input, credential));
            }
            if let Some(reasoning) = &input.reasoning {
                body["reasoning"] = json!({"effort":reasoning});
            }
            if !route.subscription {
                body["max_output_tokens"] = json!(route.max_output_tokens);
            }
            if route.include_encrypted_reasoning {
                body["include"] = json!(["reasoning.encrypted_content"]);
            }
            Ok(body)
        }
        Protocol::Messages => {
            let mut history = Vec::new();
            for message in input.messages.iter().filter(|m| m.role != Role::System) {
                let content = if let Some(items) = native(message, input, credential) {
                    items
                } else {
                    let mut content = Vec::new();
                    for part in parts(message).iter() {
                        content.push(match kind(part) {
                            Some("text")=>json!({"type":"text","text":text(part)}),
                            Some("image")=>{let(mime,data)=image(part)?;json!({"type":"image","source":{"type":"base64","media_type":mime,"data":data}})},
                            Some("tool-call")=>json!({"type":"tool_use","id":part["toolCallId"],"name":part["toolName"],"input":part["input"]}),
                            Some("tool-result")=>{
                                let result=if let Some(files)=tool_parts(&part["output"]) {
                                    Value::Array(files.iter().map(|f|if f["type"]=="text" {json!({"type":"text","text":f["text"]})} else {json!({"type":"image","source":{"type":"base64","media_type":f["mediaType"],"data":f["data"]["data"]}})}).collect())
                                }else{json!(text_result(&part["output"]))};
                                json!({"type":"tool_result","tool_use_id":part["toolCallId"],"content":result})
                            },
                            Some("reasoning")=>continue,
                            _=>return Err(failed("unsupported_message_part")),
                        });
                    }
                    content
                };
                if content.is_empty() {
                    continue;
                }
                let role = if message.role == Role::Assistant {
                    "assistant"
                } else {
                    "user"
                };
                if let Some(last) = history
                    .last_mut()
                    .filter(|last: &&mut Value| last["role"] == role)
                {
                    last["content"].as_array_mut().unwrap().extend(content);
                } else {
                    history.push(json!({"role":role,"content":content}));
                }
            }
            let mut tools=input.tools.iter().map(|(name,t)|json!({"name":name,"description":t.description,"input_schema":t.parameters})).collect::<Vec<_>>();
            let mut instructions = vec![json!({"type":"text","text":system})];
            if route.capabilities.prompt_cache {
                if let Some(tool) = tools.last_mut() {
                    tool["cache_control"] = json!({"type":"ephemeral"});
                }
                if !system.is_empty() {
                    instructions[0]["cache_control"] = json!({"type":"ephemeral"});
                }
                if let Some(block) = history
                    .last_mut()
                    .and_then(|m| m["content"].as_array_mut())
                    .and_then(|p| {
                        p.iter_mut().rev().find(|block| {
                            !matches!(
                                block["type"].as_str(),
                                Some("thinking" | "redacted_thinking")
                            )
                        })
                    })
                {
                    block["cache_control"] = json!({"type":"ephemeral"});
                }
            }
            if system.is_empty() {
                instructions.clear();
            }
            if let Some(attribution) = route.client_attribution() {
                // This is stable provider routing metadata, not an SDK identity.
                // Keep the actual instruction block's cache breakpoint intact.
                instructions.insert(0, json!({"type":"text","text":attribution}));
            }
            let mut body = json!({"model":input.model,"system":instructions,"messages":history,"tools":tools,"max_tokens":route.max_output_tokens,"stream":true});
            if instructions.is_empty() {
                body.as_object_mut().unwrap().remove("system");
            }
            if tools.is_empty() {
                body.as_object_mut().unwrap().remove("tools");
            }
            if let Some(effort) = &input.reasoning {
                body["thinking"] = json!({"type":"adaptive"});
                body["output_config"] = json!({"effort":effort});
            }
            Ok(body)
        }
        Protocol::GenerateContent => {
            let mut history = Vec::new();
            for message in input.messages.iter().filter(|m| m.role != Role::System) {
                let content = if let Some(items) = native(message, input, credential) {
                    items
                } else {
                    let mut content = Vec::new();
                    for part in parts(message).iter() {
                        content.push(match kind(part){
                        Some("text")=>json!({"text":text(part)}),
                        Some("image")=>{let(mime,data)=image(part)?;json!({"inlineData":{"mimeType":mime,"data":data}})},
                        Some("tool-call")=>json!({"functionCall":{"id":part["toolCallId"],"name":part["toolName"],"args":part["input"]}}),
                        Some("tool-result")=>{
                            let mut response = json!({"id":part["toolCallId"],"name":part["toolName"],"response":{"output":text_result(&part["output"])}});
                            if let Some(files)=tool_parts(&part["output"]) {
                                let mut images = Vec::with_capacity(files.len());
                                for file in files.iter().filter(|f|matches!(f["type"].as_str(),Some("file"|"image"))) {
                                    let mime = file["mediaType"].as_str().ok_or_else(|| failed("invalid_image"))?;
                                    let data = file["data"]["data"].as_str().ok_or_else(|| failed("invalid_image"))?;
                                    if !matches!(mime, "image/png" | "image/jpeg" | "image/webp" | "image/gif") {
                                        return Err(failed("unsupported_image"));
                                    }
                                    images.push(json!({"inlineData":{"mimeType":mime,"data":data}}));
                                }
                                response["response"] = json!({"output":if part["output"]["type"]=="mixed" {text_result(&part["output"])} else {"Images returned by tool".into()}});
                                response["parts"] = json!(images);
                            }
                            json!({"functionResponse":response})
                        },
                        Some("reasoning")=>continue,
                        _=>return Err(failed("unsupported_message_part")),
                    });
                    }
                    content
                };
                if !content.is_empty() {
                    history.push(json!({"role":if message.role==Role::Assistant{"model"}else{"user"},"parts":content}));
                }
            }
            let declarations=input.tools.iter().map(|(name,t)|json!({"name":name,"description":t.description,"parametersJsonSchema":t.parameters})).collect::<Vec<_>>();
            let mut body = json!({"systemInstruction":{"parts":[{"text":system}]},"contents":history,"generationConfig":{"maxOutputTokens":route.max_output_tokens}});
            if system.is_empty() {
                body.as_object_mut().unwrap().remove("systemInstruction");
            }
            if !declarations.is_empty() {
                body["tools"] = json!([{"functionDeclarations":declarations}]);
            }
            if let Some(effort) = &input.reasoning {
                body["generationConfig"]["thinkingConfig"] = json!({"thinkingLevel":effort});
            }
            Ok(body)
        }
        Protocol::ChatCompletions => {
            let mut history = vec![json!({"role":"system","content":system})];
            for message in input.messages.iter().filter(|m| m.role != Role::System) {
                let mut content = Vec::new();
                let mut reasoning = String::new();
                let origin = message.metadata.get("zuseOrigin");
                let compatible = origin.is_some_and(|o| {
                    o["connection"] == credential.connection_id
                        && o["model"] == input.model
                        && o["route"] == route_key(route)
                        && o["authorizationGeneration"] == credential.authorization_generation
                });
                let mut calls = Vec::new();
                for part in parts(message).iter() {
                    match kind(part){
                    Some("text")=>content.push(json!({"type":"text","text":text(part)})),
                    Some("image")=>{let(mime,data)=image(part)?;content.push(json!({"type":"image_url","image_url":{"url":format!("data:{mime};base64,{data}")}}));},
                    Some("tool-call")=>calls.push(json!({"id":part["toolCallId"],"type":"function","function":{"name":part["toolName"],"arguments":part["input"].to_string()}})),
                    Some("tool-result")=>{if tool_images(&part["output"]){return Err(failed("unsupported_image_tool_result"));}history.push(json!({"role":"tool","tool_call_id":part["toolCallId"],"content":text_result(&part["output"])}));},
                    Some("reasoning")=>{if compatible{reasoning.push_str(part["text"].as_str().unwrap_or(""));}},
                    _=>return Err(failed("unsupported_message_part")),
                }
                }
                if !content.is_empty() || !calls.is_empty() {
                    let mut m = json!({"role":role(message.role),"content":if content.is_empty(){Value::Null}else{json!(content)}});
                    if !calls.is_empty() {
                        m["tool_calls"] = json!(calls);
                    }
                    if !reasoning.is_empty() {
                        m["reasoning_content"] = json!(reasoning);
                    }
                    history.push(m);
                }
            }
            let mut body = json!({"model":input.model,"messages":history,"stream":true,"stream_options":{"include_usage":true},"max_tokens":route.max_output_tokens});
            if !input.tools.is_empty() {
                body["tools"]=json!(input.tools.iter().map(|(name,t)|json!({"type":"function","function":{"name":name,"description":t.description,"parameters":t.parameters}})).collect::<Vec<_>>());
            }
            if let Some(effort) = &input.reasoning {
                body["reasoning_effort"] = json!(effort);
            }
            Ok(body)
        }
    }
}
