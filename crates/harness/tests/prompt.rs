use serde_json::{Value, json};
use zuse_harness::{Call, Compaction, Message, Role, ToolOutput, prompt::*};
fn legacy_estimate(messages: &[Message]) -> u64 {
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

#[test]
fn borrowed_estimates_match_legacy_including_images_metadata_and_compaction() {
    let mut messages = vec![
        Message::text(Role::System, "instructions"),
        Message::text(Role::User, "task \"\\\n漢字"),
        Message::text(Role::Assistant, "output"),
    ];
    for content in [
        json!([]),
        json!([{"type":"image","image":"a".repeat(2_000_000),"mediaType":"image/png"}]),
        json!([{"type":"image"}]),
        json!([{"type":"tool-result","output":{"type":"content","value":[{"mediaType":"image/png","data":{"type":"data","data":"b".repeat(2_000_000)}},{"mediaType":"text/plain","data":{"data":"keep"}}]}}]),
    ] {
        let mut m = Message::text(Role::User, "");
        m.content = content.into();
        m.metadata
            .insert("providerOptions".into(), json!({"id":"abc"}));
        messages.push(m);
    }
    assert_eq!(estimate(&messages), legacy_estimate(&messages));
    for covered in 0..=messages.len() + 1 {
        let c = Compaction {
            covered,
            summary: "preserve task".into(),
        };
        assert_eq!(
            estimate_rendered(&messages, Some(&c)),
            legacy_estimate(&render(&messages, Some(&c)))
        );
    }
    messages[0]
        .metadata
        .insert("content".into(), json!([{"type":"image","image":"abc"}]));
    messages[0]
        .metadata
        .insert("role".into(), json!("assistant"));
    assert_eq!(estimate(&messages), legacy_estimate(&messages));
}

#[test]
fn bounded_context_preserves_instructions_recent_exchanges_and_full_shared_history() {
    use zuse_harness::{Call, ToolOutput};
    let call = Call {
        id: "old-call".into(),
        name: "read_file".into(),
        input: json!({}),
    };
    let history = vec![
        Message::text(Role::System, "never drop instructions"),
        Message::text(Role::User, "original task"),
        Message::tool(&call, &ToolOutput::Text("large output ".repeat(20000))),
        Message::text(Role::User, "recent task"),
        Message::tool(&call, &ToolOutput::Text("recent output ".repeat(20000))),
        Message::text(Role::User, "new task"),
    ];
    let original = serde_json::to_value(&history).unwrap();
    let mut full = history.clone();
    apply_policy(&mut full, ContextPolicy::Preserve, "root");
    assert_eq!(serde_json::to_value(full).unwrap(), original);
    let mut reduced = history.clone();
    apply_policy(&mut reduced, ContextPolicy::BoundedTools, "root");
    assert!(
        reduced[2].content[0]["output"]["value"]
            .as_str()
            .unwrap()
            .contains("read_tool_output")
    );
    assert!(
        reduced[2].content[0]["output"]["value"]
            .as_str()
            .unwrap()
            .len()
            < 13000
    );
    assert_eq!(
        serde_json::to_value(&reduced[4]).unwrap(),
        serde_json::to_value(&history[4]).unwrap()
    );
    assert_eq!(serde_json::to_value(history).unwrap(), original);
}

#[test]
fn mixed_tool_images_use_image_estimates_instead_of_counting_base64_as_text() {
    let make = |size| {
        Message::tool(
            &Call {
                id: "image".into(),
                name: "image".into(),
                input: serde_json::json!({}),
            },
            &ToolOutput::Mixed {
                r#type: "mixed".into(),
                parts: vec![
                    serde_json::json!({"type":"text","text":"Image result"}),
                    serde_json::json!({"type":"file","mediaType":"image/png","data":{"type":"data","data":"a".repeat(size)}}),
                ],
            },
        )
    };
    assert_eq!(
        zuse_harness::prompt::estimate(&[make(32)]),
        zuse_harness::prompt::estimate(&[make(100000)])
    );
}

mod merged_main {
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
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
                    let message = Message {
                        role,
                        content: content.clone().into(),
                        metadata: metadata.into(),
                    };
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
            let expected = if covered < history.len() {
                (*history.last().unwrap().content).clone()
            } else {
                json!("Durable context summary:\nProgress and unfinished work")
            };
            assert_eq!(&*rendered.last().unwrap().content, &expected);
        }
        assert_eq!(
            prompt::estimate_context(&history, None),
            prompt::estimate(&history)
        );
        assert_eq!(prompt::boundary(&history), 5);
        assert_eq!(prompt::boundary(&history[..1]), 0);
        assert_eq!(prompt::boundary(&[]), 0);
    }
}
