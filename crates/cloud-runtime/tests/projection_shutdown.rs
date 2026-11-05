#![cfg(feature = "engine")]

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use zuse_cloud_projection::{Head, TranscriptKey, Upload};
use zuse_cloud_runtime::{Authority, projection::Publisher};
use zuse_engine::{EngineCore, EngineProfile, HarnessRegistry};
use zuse_protocol::{HarnessId, cloud::CloudRuntimeBootstrap, methods};
use zuse_rpc::RpcService;

#[derive(Default)]
struct Published {
    references: HashSet<String>,
    order: Vec<String>,
    rejected_chunks: usize,
}
#[derive(Clone)]
struct Server {
    key: String,
    published: Arc<Mutex<Published>>,
}
async fn head(State(server): State<Server>, Json(upload): Json<Upload>) -> Json<Value> {
    let head: Head = TranscriptKey::parse(&server.key)
        .unwrap()
        .open(
            "task",
            &upload.session_id,
            &upload.cursor,
            upload.before_sequence,
            &upload.ciphertext,
        )
        .unwrap();
    let mut state = server.published.lock().unwrap();
    state.order.push(format!("head:{}", upload.cursor.version));
    let zuse_doc::TranscriptFrame::Reset { reset } = head.transcript.frame else {
        panic!("complete head required");
    };
    for entry in reset.iter() {
        for part in serde_json::to_value(entry).unwrap()["parts"]
            .as_array()
            .unwrap()
        {
            if part["kind"] == "tool" && part["resolved"] == true {
                for field in ["outputRef", "diffRef"] {
                    if let Some(reference) = part[field].as_str() {
                        state.references.insert(reference.to_owned());
                    }
                }
            }
        }
    }
    Json(json!({"stored": true}))
}
async fn chunk(
    State(server): State<Server>,
    Json(chunk): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let mut state = server.published.lock().unwrap();
    let reference = chunk["blobRef"].as_str().unwrap();
    // This is the real tool_output_upload contract: an authenticated guest may
    // upload a chunk only after a transcript has authorized its reference.
    if !state.references.contains(reference) {
        state.rejected_chunks += 1;
        return (
            StatusCode::CONFLICT,
            Json(json!({"error":"tool_output_reference_not_published"})),
        );
    }
    state.order.push("chunk".into());
    (StatusCode::OK, Json(json!({"stored":true})))
}
async fn setup(root: &Path) -> Arc<EngineCore> {
    let core = Arc::new(
        EngineCore::assemble_with_profile(
            EngineProfile::synced(root, "task", "owner")
                .with_durable_command_commits()
                .with_managed_recovery()
                .with_preparation_pending(),
            Arc::new(HarnessRegistry::new()),
            HarnessId::Mock,
            None,
        )
        .unwrap(),
    );
    core.rpc_service().handle(methods::MUTATE, json!({
        "op":"createChat", "chatId":"session", "deviceId":core.device_id,
        "cwd":root, "config":{"harness":"mock","model":"fixture","sandbox":"workspace-write"}
    })).await.unwrap();
    core.workspace
        .rename_chat("session", "Publication fixture")
        .unwrap();
    core
}
fn boot() -> CloudRuntimeBootstrap {
    serde_json::from_value(json!({
        "workspaceId":"task","zuseAccountId":"owner","chatId":"session","initialSessionId":"session",
        "runtimeCredential":"synthetic","runtimeGeneration":1,"gatewayEpoch":1,
        "runtimeCredentialExpiresAt":9007199254740000i64,"gatewayUrl":"wss://fixture.invalid",
        "gatewayProtocol":"fixture","sealedTranscriptKey":"synthetic",
        "runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"},
        "commandAdmissionVersion":2,"rustToolOutputVersion":1
    })).unwrap()
}
fn append_tool(core: &EngineCore, handle: &Arc<zuse_engine::doc_host::ChatDocHandle>) -> String {
    let (reference, _) = core
        .doc_host
        .persist_tool_sidecar(
            "session",
            "assistant",
            &zuse_doc::SidecarPayload {
                part_id: "tool".into(),
                output: Some("completed shell output\n".into()),
                diff: None,
            },
        )
        .unwrap();
    let reference = reference.unwrap();
    let entry: zuse_doc::SessionMessageEntry = serde_json::from_value(json!({
        "id":"assistant","role":"assistant","deviceId":core.device_id,"createdAt":1,
        "parts":[{"kind":"tool","id":"tool","call":{"kind":"exec","command":"true"},
                  "resolved":true,"outputRef":reference}]
    }))
    .unwrap();
    handle.doc_arc().push_message(&entry).unwrap();
    reference
}

#[tokio::test]
async fn cold_finish_publishes_new_tool_reference_before_its_chunk_when_old_head_is_pending() {
    let dir = tempfile::tempdir().unwrap();
    let core = setup(dir.path()).await;
    let database = EngineProfile::synced(dir.path(), "task", "owner")
        .store_root()
        .join("docs.sqlite3");
    let key = URL_SAFE_NO_PAD.encode([7_u8; 32]);
    let publisher = Publisher::open(&database, &key).unwrap();
    let boot = boot();
    let handle = core.doc_host.open_local("session").unwrap();
    publisher.capture(&core, &boot, &[handle.clone()]).unwrap();
    let reference = append_tool(&core, &handle);
    // A previous durable head is still pending while the new tool completes.
    publisher.capture(&core, &boot, &[handle.clone()]).unwrap();
    let published = Arc::new(Mutex::new(Published::default()));
    let server = Server {
        key,
        published: published.clone(),
    };
    let app = Router::new()
        .route(
            "/v1/cloud/workspaces/task/runtime/transcript-checkpoint",
            post(head),
        )
        .route("/v1/cloud/workspaces/task/runtime/tool-output", post(chunk))
        .with_state(server);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let authority = Authority::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let http = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::timeout(Duration::from_secs(3), core.shutdown())
        .await
        .expect("real engine shutdown must complete");
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        publisher.finish(&core, &authority, &boot, &[handle]),
    )
    .await
    .unwrap();
    let (rejected, order, authorized) = {
        let state = published.lock().unwrap();
        (
            state.rejected_chunks,
            state.order.clone(),
            state.references.contains(&reference),
        )
    };
    http.abort();
    result.expect(
        "cold shutdown must publish the current reference before attempting its tool chunk",
    );
    assert_eq!(rejected, 0, "never send an unreferenced chunk");
    assert!(authorized);
    assert_eq!(order, ["head:1", "head:2", "chunk"]);
}

#[tokio::test]
async fn committed_head_recovers_failed_tool_capture_without_holding_a_sqlite_writer() {
    let dir = tempfile::tempdir().unwrap();
    let core = setup(dir.path()).await;
    let database = EngineProfile::synced(dir.path(), "task", "owner")
        .store_root()
        .join("docs.sqlite3");
    let key = URL_SAFE_NO_PAD.encode([8_u8; 32]);
    let publisher = Publisher::open(&database, &key).unwrap();
    let boot = boot();
    let handle = core.doc_host.open_local("session").unwrap();
    let reference = append_tool(&core, &handle);
    assert!(!zuse_cloud_runtime::tool_output::guest::pending(&database).unwrap());
    let db = rusqlite::Connection::open(&database).unwrap();
    db.execute_batch("CREATE TRIGGER reject_tool_capture BEFORE INSERT ON zuse_cloud_tool_output_outbox BEGIN SELECT RAISE(ABORT,'fixture_capture_failure'); END;").unwrap();
    assert!(publisher.capture(&core, &boot, &[handle.clone()]).is_err());
    let (version, pending_head): (i64, bool) = db.query_row(
        "SELECT version,uploads IS NOT NULL FROM zuse_cloud_projection_outbox WHERE session_id='session'",
        [], |r| Ok((r.get(0)?, r.get(1)?)),
    ).expect("the authorizing head must commit before fallible tool capture");
    assert_eq!(version, 1);
    assert!(pending_head);
    assert!(!zuse_cloud_runtime::tool_output::guest::pending(&database).unwrap());
    let published = Arc::new(Mutex::new(Published::default()));
    let server = Server {
        key: key.clone(),
        published: published.clone(),
    };
    let app = Router::new()
        .route(
            "/v1/cloud/workspaces/task/runtime/transcript-checkpoint",
            post(head),
        )
        .route("/v1/cloud/workspaces/task/runtime/tool-output", post(chunk))
        .with_state(server);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let authority = Authority::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let http = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    publisher.publish(&authority, &boot).await.unwrap();
    db.execute_batch("DROP TRIGGER reject_tool_capture;")
        .unwrap();
    // Reopening models a process loss after head persistence but before chunks.
    let reopened = Publisher::open(&database, &key).unwrap();
    reopened
        .capture(&core, &boot, &[handle])
        .expect("unchanged fingerprint must reconcile missing chunks after releasing its writer");
    let version_after: i64 = db
        .query_row(
            "SELECT version FROM zuse_cloud_projection_outbox WHERE session_id='session'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        version_after, version,
        "recovery must not create a duplicate head"
    );
    assert!(zuse_cloud_runtime::tool_output::guest::pending(&database).unwrap());
    reopened.publish(&authority, &boot).await.unwrap();
    let state = published.lock().unwrap();
    assert!(state.references.contains(&reference));
    assert_eq!(state.rejected_chunks, 0);
    assert_eq!(state.order, ["head:1", "chunk"]);
    drop(state);
    core.shutdown().await;
    http.abort();
}
