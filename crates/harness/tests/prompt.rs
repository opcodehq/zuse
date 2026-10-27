use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use zuse_harness::{Compaction, Message, Role, prompt};

// Compatibility oracle: the original JSON-based accounting also defines image
// allowances and metadata override behavior. Keep it out of the runtime.
fn serialized_estimate(message: &Message) -> u64 {
    let mut value = serde_json::to_value(message).unwrap();
    let mut images = 0;
    if let Some(parts) = value["content"].as_array_mut() {
        for part in parts {
            if part["type"] == "image" {
                images += 8192;
                part["image"] = json!("");
            }
            if part["type"] == "tool-result"
                && part["output"]["type"] == "content"
                && let Some(files) = part["output"]["value"].as_array_mut()
            {
                for file in files {
                    if file["mediaType"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("image/"))
                    {
                        images += 8192;
                        file["data"]["data"] = json!("");
                    }
                }
            }
        }
    }
    (serde_json::to_vec(&value).unwrap().len() as u64).div_ceil(3) + 8 + images
}

#[test]
fn accounting_matches_serialized_messages_without_mutating_images() {
    let mut texts = vec![String::new(), "Unicode λ🦀\u{2028}\u{2029}".into()];
    texts.push((0..=127).map(char::from).collect());
    texts.push("\\\"\n\t/rust/source.rs".repeat(1000));
    let payload = "a".repeat(1024 * 1024);
    let mut contents: Vec<Value> = texts.into_iter().map(Value::String).collect();
    contents.extend([
        json!([]),
        json!([{"type":"image", "image":payload, "mediaType":"image/png"}]),
        json!([{"type":"image"}]),
        json!([{"type":"text", "text":"hi"}, {"type":"tool-call", "input":{
            "null":null, "bool":false, "negative":-12, "fraction":1.25,
            "exponent":1e100, "nested":[true, ["x"], {}]}}]),
        json!([{"type":"tool-result", "output":{"type":"content", "value":[
            {"type":"file", "mediaType":"image/png", "data":{"type":"data", "data":payload}},
            {"type":"file", "mediaType":"text/plain", "data":{"type":"data", "data":"retained text"}},
            {"mediaType":"image/jpeg"},
            {"mediaType":"image/webp", "data":{"other":"retained metadata"}}
        ]}}]),
    ]);
    for role in [Role::System, Role::User, Role::Assistant, Role::Tool] {
        for content in &contents {
            for metadata in [
                BTreeMap::new(),
                BTreeMap::from([("provider".into(), json!({"cached":true, "number":1e-9}))]),
                BTreeMap::from([("role".into(), json!("override"))]),
                BTreeMap::from([("content".into(), content.clone())]),
            ] {
                let message = Arc::new(Message {
                    role,
                    content: content.clone(),
                    metadata,
                });
                let before = serde_json::to_vec(&message).unwrap();
                assert_eq!(
                    prompt::estimate(std::slice::from_ref(&message)),
                    serialized_estimate(&message)
                );
                assert_eq!(serde_json::to_vec(&message).unwrap(), before);
            }
        }
    }
}

#[test]
fn borrowed_context_matches_rendering_at_every_boundary() {
    let history: Vec<_> = vec![
        Message::text(Role::System, "Always retain instructions"),
        Message::text(Role::User, "Original task"),
        Message::text(Role::Assistant, "Tool request"),
        Message::text(Role::Tool, "Tool output"),
        Message::text(Role::System, "Later instruction"),
        Message::text(Role::User, "Continuation"),
        Message::text(Role::Assistant, "Answer"),
    ]
    .into_iter()
    .map(Arc::new)
    .collect();
    for covered in 0..=history.len() + 1 {
        let checkpoint = Compaction {
            covered,
            summary: "Progress and unfinished work".into(),
        };
        let rendered = prompt::render(&history, Some(&checkpoint));
        assert_eq!(
            prompt::estimate_context(&history, Some(&checkpoint)),
            prompt::estimate(&rendered)
        );
        assert_eq!(
            rendered.last().unwrap().content,
            if covered < history.len() {
                history.last().unwrap().content.clone()
            } else {
                json!("Durable context summary:\nProgress and unfinished work")
            }
        );
    }
    assert_eq!(
        prompt::estimate_context(&history, None),
        prompt::estimate(&history)
    );
    assert_eq!(prompt::boundary(&history), 5);
    assert_eq!(prompt::boundary(&history[..1]), 0);
    assert_eq!(prompt::boundary(&[]), 0);
}
