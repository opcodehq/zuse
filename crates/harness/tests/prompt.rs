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
