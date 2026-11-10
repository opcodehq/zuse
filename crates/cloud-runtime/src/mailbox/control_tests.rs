//! Controlled slow-input admission; control commands must have reserved slots.
use super::*;
use axum::{Json, Router, extract::State, routing::post};
use std::sync::atomic::{AtomicUsize, Ordering};
#[derive(Default)]
struct SlowInputs {
    started: AtomicUsize,
    controls: AtomicUsize,
    blocked: tokio::sync::Notify,
}
#[async_trait]
impl Executor for SlowInputs {
    async fn admitted(&self, _: &MessageSend, _: &str) -> Result<bool> {
        Ok(false)
    }
    async fn native_admitted(&self, _: &CloudSessionCommand, _: &str) -> Result<bool> {
        Ok(false)
    }
    async fn admit_native(&self, p: &CloudSessionCommand, _: &str, _: i64) -> Result<()> {
        if matches!(
            p.payload,
            zuse_protocol::session_commands::SessionCommandPayload::Run { .. }
        ) {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.blocked.notified().await;
        } else {
            self.controls.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn admit(&self, _: &MessageSend, _: &str, _: i64) -> Result<()> {
        bail!("native-only fixture")
    }
}
struct Server {
    runs: Mutex<Option<Vec<RuntimeLease>>>,
    stop: Mutex<Option<RuntimeLease>>,
    control_requests: AtomicUsize,
}
fn lease(key: &CommandKey, id: &str, control: bool) -> RuntimeLease {
    let payload = if control {
        json!({"kind":"interrupt"})
    } else {
        json!({"kind":"run","messageId":format!("message-{id}"),"request":{"prompt":"blocked input preparation","cwd":".","harness":"mock","model":"fixture","sandbox":"workspace-write"}})
    };
    let command: CloudSessionCommand = serde_json::from_value(
        json!({"commandId":id,"sessionId":format!("session-{id}"),"payload":payload}),
    )
    .unwrap();
    let plain = serde_json::to_vec(&command).unwrap();
    let mut envelope:CommandEnvelope=serde_json::from_value(json!({"protocolVersion":3,"workspaceId":"workspace","sessionId":command.session_id,"commandId":id,"kind":if control{"zuse.session.interrupt"}else{"zuse.session.run"},"fingerprint":key.fingerprint(&plain),"schemaVersion":1,"keyVersion":1,"destructionFence":0,"createdAt":now_ms(),"iv":"","ciphertext":"","dependencies":[]})).unwrap();
    (envelope.iv, envelope.ciphertext) = key.encrypt(&envelope, &plain).unwrap();
    RuntimeLease {
        command: envelope,
        workspace_sequence: 1,
        lease_token: format!("lease-{id}"),
        lease_deadline: now_ms() + 60000,
        runtime_generation: 1,
        provider_sandbox_id: "machine".into(),
        storage_incarnation_id: "disk".into(),
    }
}
async fn leases(State(state): State<Arc<Server>>, Json(request): Json<Value>) -> Json<LeasePage> {
    let max = request["maxItems"].as_u64().unwrap();
    assert!((1..=4).contains(&max));
    let control = request["controlOnly"] == true;
    let leases = if control {
        state.control_requests.fetch_add(1, Ordering::SeqCst);
        state.stop.lock().unwrap().take().into_iter().collect()
    } else {
        state.runs.lock().unwrap().take().unwrap_or_default()
    };
    if leases.is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Json(LeasePage {
        leases,
        nonterminal_count: 5,
        mailbox_revision: 1,
        fence_required: false,
    })
}
#[tokio::test]
async fn slow_input_admissions_leave_a_bounded_control_lane_open() {
    let key = CommandKey::parse(&B64.encode([19; 32])).unwrap();
    let state = Arc::new(Server {
        runs: Mutex::new(Some(
            (0..4)
                .map(|i| lease(&key, &format!("run-{i}"), false))
                .collect(),
        )),
        stop: Mutex::new(Some(lease(&key, "stop", true))),
        control_requests: AtomicUsize::new(0),
    });
    let executor = Arc::new(SlowInputs::default());
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("receipts.sqlite3");
    drop(Connection::open(&database).unwrap());
    let boot:CloudRuntimeBootstrap=serde_json::from_value(json!({"workspaceId":"workspace","zuseAccountId":"owner","chatId":"thread","initialSessionId":"session","runtimeCredential":"synthetic","runtimeGeneration":1,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9007199254740000i64,"gatewayUrl":"wss://fixture.invalid","gatewayProtocol":"fixture","sealedTranscriptKey":"synthetic","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"},"providerSandboxId":"machine","commandAdmissionVersion":2,"commandDeliveryMode":"server-admitted-v2"})).unwrap();
    let mailbox = Mailbox::new(&database, &boot, key, executor.clone()).unwrap();
    let app = Router::new()
        .route(
            "/v1/cloud/workspaces/workspace/runtime/commands/lease",
            post(leases),
        )
        .route(
            "/v1/cloud/workspaces/workspace/runtime/commands/ack",
            post(|| async { Json(json!({"ok":true})) }),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let authority = Authority::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let (_tx, rx) = watch::channel(boot);
    let runtime = tokio::spawn(async move { mailbox.run(authority, rx, token).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while executor.controls.load(Ordering::SeqCst) != 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(executor.started.load(Ordering::SeqCst), 4);
    assert!(state.control_requests.load(Ordering::SeqCst) > 0);
    cancel.cancel();
    runtime.await.unwrap().unwrap();
    server.abort();
    let db = Connection::open(database).unwrap();
    let admitted: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM zuse_cloud_command_receipts WHERE acknowledgment IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(admitted, 1, "blocked inputs have no false admission ACK");
}

#[tokio::test]
async fn input_deadline_preserves_unknown_and_never_repeats_admission() {
    let key = CommandKey::parse(&B64.encode([19; 32])).unwrap();
    let mut delivery = lease(&key, "expired-input", false);
    delivery.lease_deadline = now_ms() + 25;
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("receipts.sqlite3");
    drop(Connection::open(&database).unwrap());
    let bootstrap:CloudRuntimeBootstrap=serde_json::from_value(json!({"workspaceId":"workspace","zuseAccountId":"owner","chatId":"thread","initialSessionId":"session","runtimeCredential":"synthetic","runtimeGeneration":1,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9007199254740000i64,"gatewayUrl":"wss://fixture.invalid","gatewayProtocol":"fixture","sealedTranscriptKey":"synthetic","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"},"providerSandboxId":"machine","commandAdmissionVersion":2,"commandDeliveryMode":"server-admitted-v2"})).unwrap();
    let executor = Arc::new(SlowInputs::default());
    let mailbox = Mailbox::new(&database, &bootstrap, key, executor.clone()).unwrap();
    delivery.lease_deadline = now_ms() + 50;
    let ack = tokio::time::timeout(Duration::from_secs(1), mailbox.apply(&delivery))
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(ack.state, AcknowledgmentState::OutcomeUnknown));
    let repeated = mailbox.apply(&delivery).await.unwrap().unwrap();
    assert!(matches!(
        repeated.state,
        AcknowledgmentState::OutcomeUnknown
    ));
    assert_eq!(executor.started.load(Ordering::SeqCst), 1);
}
