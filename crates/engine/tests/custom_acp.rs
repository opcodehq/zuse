#![cfg(unix)]
use serde_json::json;
use std::{sync::Arc, time::Duration};
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::{HarnessId, custom_acp::CustomAcpAgent};
use zuse_rpc::methods;

fn config(marker: &str) -> CustomAcpAgent {
    CustomAcpAgent {
        id: HarnessId::try_from(format!("acp-{marker}")).unwrap(),
        name: format!("Local {marker}"),
        command: "python3".into(),
        args: vec![
            format!(
                "{}/tests/fixtures/custom-acp.py",
                env!("CARGO_MANIFEST_DIR")
            ),
            marker.into(),
            "literal ; $(not-a-shell)".into(),
        ],
        env: [("ACP_TEST_MARKER".into(), marker.into())]
            .into_iter()
            .collect(),
        model_ids: vec![],
    }
}
fn text(core: &EngineCore, chat: &str, role: zuse_doc::MessageRole) -> Vec<String> {
    core.doc_host
        .open(chat)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .into_iter()
        .filter(|message| message.role == role)
        .map(|message| {
            message
                .parts
                .into_iter()
                .filter_map(|part| match part {
                    zuse_doc::MessagePart::Text { text, .. } => Some(text),
                    _ => None,
                })
                .collect::<String>()
        })
        .collect()
}
#[tokio::test]
async fn custom_acp_instances_discover_and_drain_independent_queues() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Arc::new(HarnessRegistry::new());
    let core = EngineCore::assemble(
        temp.path(),
        registry.clone(),
        config("first-agent").id,
        None,
    )
    .unwrap();
    let client = zuse_rpc::memory_client(core.rpc_service());
    for (index, marker) in ["first-agent", "second-agent"].into_iter().enumerate() {
        let definition = config(marker);
        client
            .call(
                methods::SET_CUSTOM_ACP_AGENT,
                json!({"agent":definition,"expectedRevision":index}),
            )
            .await
            .unwrap();
        let models = client
            .call(methods::LIST_MODELS, json!({"harness":definition.id}))
            .await
            .unwrap();
        // Model inventory wraps a catalog on current hosts; accept its canonical rows.
        let rows = models
            .as_array()
            .or_else(|| models["models"].as_array())
            .unwrap();
        assert!(
            rows.iter()
                .any(|row| row["id"] == format!("{marker}/model"))
        );
        client.call(methods::MUTATE, json!({"op":"createChat","chatId":marker,"deviceId":core.device_id,"cwd":temp.path(),"config":{"harness":definition.id,"sandbox":"workspace-write","model":format!("{marker}/model")}})).await.unwrap();
        core.workspace
            .rename_chat(marker, &format!("Local {marker}"))
            .unwrap();
    }
    let contexts = ["first-agent", "second-agent"].map(|marker| {
        registry
            .resolve(config(marker).id)
            .unwrap()
            .model_context()
            .unwrap()
            .unwrap()
            .hash
    });
    assert_ne!(
        contexts[0], contexts[1],
        "separate executable definitions must never share credential/catalog context"
    );
    let descriptors: Vec<_> = registry
        .descriptors()
        .into_iter()
        .filter(|row| matches!(row.id, HarnessId::CustomAcp(_)))
        .collect();
    assert_eq!(descriptors.len(), 2);
    assert!(
        descriptors
            .iter()
            .all(|row| row.installed && !row.can_install)
    );
    for marker in ["first-agent", "second-agent"] {
        core.doc_host
            .queue_message(marker, "first", vec![])
            .unwrap();
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        if ["first-agent", "second-agent"]
            .iter()
            .all(|chat| !text(&core, chat, zuse_doc::MessageRole::User).is_empty())
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "initial custom turns did not start"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    for marker in ["first-agent", "second-agent"] {
        core.doc_host
            .queue_message(marker, "second", vec![])
            .unwrap();
        core.doc_host
            .queue_message(marker, "third", vec![])
            .unwrap();
    }
    loop {
        let finished = ["first-agent", "second-agent"].iter().all(|chat| {
            let users = text(&core, chat, zuse_doc::MessageRole::User);
            let replies = text(&core, chat, zuse_doc::MessageRole::Assistant).join("\n");
            users == ["first", "second", "third"]
                && replies.contains(&format!("{chat}:third"))
                && core
                    .doc_host
                    .open(chat)
                    .unwrap()
                    .doc()
                    .read_queue()
                    .unwrap()
                    .is_empty()
        });
        if finished {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "custom queue delivery did not settle"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
    assert!(
        !text(&core, "first-agent", zuse_doc::MessageRole::Assistant)
            .join("\n")
            .contains("second-agent:")
    );
    assert!(
        !text(&core, "second-agent", zuse_doc::MessageRole::Assistant)
            .join("\n")
            .contains("first-agent:")
    );
    core.shutdown().await;
}
