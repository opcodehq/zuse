//! Conversations share one thread checkout while their histories remain independent.
use serde_json::json;
use std::sync::Arc;
use zuse_engine::{EngineCore, HarnessRegistry};
use zuse_protocol::{Chat, HarnessId};
use zuse_rpc::methods;

#[tokio::test]
async fn thread_conversations_persist_and_archive_with_their_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("profile");
    let checkout = temp.path().join("project");
    std::fs::create_dir_all(&checkout).unwrap();
    std::fs::write(checkout.join("keep.txt"), "user changes").unwrap();
    let core = EngineCore::assemble(
        &profile,
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let client = zuse_rpc::memory_client(core.rpc_service());
    client.call(methods::MUTATE, json!({"op":"createSpace","spaceId":"project","deviceId":core.device_id,"path":checkout,"name":"Project"})).await.unwrap();
    client
        .call(
            methods::MUTATE,
            json!({"op":"createChat","chatId":"thread","spaceId":"project","cwd":checkout}),
        )
        .await
        .unwrap();
    core.doc_host.open("thread").unwrap().doc().push_message(&serde_json::from_value(json!({
        "id":"root-only", "role":"user", "parts":[{"id":"root-text","kind":"text","text":"Keep this in the first conversation"}],
        "createdAt": 1, "deviceId":core.device_id
    })).unwrap()).unwrap();
    let first: Chat = client
        .call_as(
            methods::CREATE_THREAD_SESSION,
            json!({"chatId":"conversation-a","threadId":"thread"}),
        )
        .await
        .unwrap();
    let second: Chat = client
        .call_as(
            methods::CREATE_THREAD_SESSION,
            json!({"chatId":"conversation-b","threadId":"thread"}),
        )
        .await
        .unwrap();
    assert_eq!(first.thread_id(), "thread");
    assert_eq!(first.cwd, second.cwd);
    assert_eq!(first.cwd.as_deref(), checkout.to_str());
    assert!(first.parent_chat_id.is_none());
    assert_eq!(first.room_gen, Some(2));
    assert!(first.harness_session_id.is_none());
    assert!(
        core.doc_host
            .open(&first.id)
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
            .is_empty()
    );
    assert!(
        core.doc_host
            .open(&second.id)
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
            .is_empty()
    );
    let again: Chat = client
        .call_as(
            methods::CREATE_THREAD_SESSION,
            json!({"chatId":first.id,"threadId":"thread"}),
        )
        .await
        .unwrap();
    assert_eq!(
        again.created_at.timestamp_millis(),
        first.created_at.timestamp_millis()
    );
    assert!(
        client
            .call(
                methods::CREATE_THREAD_SESSION,
                json!({"chatId":"thread","threadId":"thread"})
            )
            .await
            .is_err()
    );
    client
        .call(
            methods::MUTATE,
            json!({"op":"setChatArchived","chatId":"conversation-a","archived":true}),
        )
        .await
        .unwrap();
    for id in ["thread", "conversation-a", "conversation-b"] {
        assert!(core.workspace.chat(id).unwrap().unwrap().archived);
    }
    assert_eq!(
        std::fs::read_to_string(checkout.join("keep.txt")).unwrap(),
        "user changes"
    );
    assert!(
        client
            .call(
                methods::CREATE_THREAD_SESSION,
                json!({"chatId":"archived-new","threadId":"thread"})
            )
            .await
            .is_err()
    );
    client
        .call(
            methods::MUTATE,
            json!({"op":"setChatArchived","chatId":"thread","archived":false}),
        )
        .await
        .unwrap();
    for id in ["thread", "conversation-a", "conversation-b"] {
        assert!(!core.workspace.chat(id).unwrap().unwrap().archived);
    }
    drop(client);
    core.shutdown().await;
    drop(core);
    let reopened = EngineCore::assemble(
        &profile,
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    let saved = reopened.workspace.chat("conversation-a").unwrap().unwrap();
    assert_eq!(saved.thread_id(), "thread");
    assert_eq!(saved.cwd.as_deref(), checkout.to_str());
    reopened.workspace.delete_chat("thread").unwrap();
    assert!(
        reopened
            .workspace
            .chat("conversation-a")
            .unwrap()
            .unwrap()
            .thread_id
            .is_none()
    );
    reopened.shutdown().await;
}
