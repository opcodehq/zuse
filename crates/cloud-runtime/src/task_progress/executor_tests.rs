//! Real Core, command crypto and HTTP, with an explicitly controlled harness.
use super::*;
use crate::{
    engine::{EngineExecutor, TaskContext},
    mailbox::{CommandKey, Mailbox},
};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use futures::stream::BoxStream;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::Semaphore;
use zuse_engine::{EngineProfile, HarnessRegistry};
use zuse_protocol::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
    cloud_assets::{Asset, Download},
    cloud_commands::{CommandEnvelope, LeasePage, RuntimeLease},
    methods,
};
use zuse_providers::{Harness, HarnessError, RunControls};
use zuse_rpc::RpcService;
struct ControlledCodex {
    requests: Mutex<Vec<RunRequest>>,
}
#[async_trait::async_trait]
impl Harness for ControlledCodex {
    fn id(&self) -> HarnessId {
        HarnessId::Codex
    }
    fn display_name(&self) -> &str {
        "Controlled fixture; no vendor calls"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }
    async fn models(&self) -> std::result::Result<Vec<Model>, HarnessError> {
        Ok(vec![serde_json::from_value(json!({"id":"fixture","label":"Controlled fixture","reasoningLevels":[],"options":[]})).unwrap()])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> std::result::Result<
        BoxStream<'static, std::result::Result<AgentEvent, HarnessError>>,
        HarnessError,
    > {
        self.requests.lock().unwrap().push(request.clone());
        if request.prompt == "controlled startup failure" {
            return Err(HarnessError::Protocol("controlled startup failure".into()));
        }

        if request.prompt == "wait for exact stop" {
            return Ok(Box::pin(futures::stream::once(async move {
                controls.interrupt.cancelled().await;
                Ok(AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    result: None,
                    error: None,
                    session_id: None,
                })
            })));
        }

        zuse_providers::mock::MockHarness {
            script: vec![
                AgentEvent::TextDelta {
                    text: "controlled completion".into(),
                },
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ],
        }
        .run(request, controls)
        .await
    }
}
struct Server {
    leases: Mutex<VecDeque<RuntimeLease>>,
    asset: Asset,
    asset_entered: Semaphore,
    release: Semaphore,
    acks: Mutex<Vec<Value>>,
    reports: Mutex<Vec<Value>>,
    asset_reads: AtomicUsize,
}
async fn lease(State(state): State<Arc<Server>>, Json(body): Json<Value>) -> Json<LeasePage> {
    assert!(body["maxItems"].as_u64().is_some_and(|n| n <= 4));
    let leases = state
        .leases
        .lock()
        .unwrap()
        .pop_front()
        .into_iter()
        .collect::<Vec<_>>();
    if leases
        .first()
        .is_some_and(|lease| lease.command.kind == "zuse.session.interrupt")
    {
        while state.asset_reads.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }
    if leases.is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Json(LeasePage {
        leases,
        nonterminal_count: 2,
        mailbox_revision: 1,
        fence_required: false,
    })
}
async fn download_asset(State(state): State<Arc<Server>>) -> Json<Download> {
    state.asset_reads.fetch_add(1, Ordering::SeqCst);
    state.asset_entered.add_permits(1);
    state.release.acquire().await.unwrap().forget();
    Json(Download {
        asset: state.asset.clone(),
        bytes: B64.encode(b"verified attachment"),
    })
}
async fn ack(State(state): State<Arc<Server>>, Json(body): Json<Value>) -> Json<Value> {
    state.acks.lock().unwrap().push(body);
    Json(json!({"ok":true}))
}
async fn report(State(state): State<Arc<Server>>, Json(body): Json<Value>) -> Json<Value> {
    state.reports.lock().unwrap().push(body.clone());
    Json(json!({"commandId":body["commandId"],"revision":body["revision"]}))
}
fn sealed(key: &CommandKey, command: &CloudSessionCommand, sequence: i64) -> RuntimeLease {
    let plain = serde_json::to_vec(command).unwrap();
    let mut envelope:CommandEnvelope=serde_json::from_value(json!({"protocolVersion":3,"workspaceId":"task","sessionId":command.session_id,"commandId":command.command_id,"kind":zuse_protocol::cloud_commands::session_command_kind(command.payload.kind()),"fingerprint":key.fingerprint(&plain),"schemaVersion":1,"keyVersion":1,"destructionFence":0,"createdAt":crate::now_ms()+sequence,"iv":"","ciphertext":"","dependencies":[]})).unwrap();
    (envelope.iv, envelope.ciphertext) = key.encrypt(&envelope, &plain).unwrap();
    RuntimeLease {
        command: envelope,
        workspace_sequence: sequence,
        lease_token: format!("lease-{sequence}"),
        lease_deadline: crate::now_ms() + 30000,
        runtime_generation: 1,
        provider_sandbox_id: "machine".into(),
        storage_incarnation_id: "disk".into(),
    }
}
async fn exercise_asset_turn(stop_inputs: bool) {
    let dir = tempfile::tempdir().unwrap();
    let checkout = dir.path().join("repo");
    std::fs::create_dir(&checkout).unwrap();
    let profile = EngineProfile::synced(dir.path(), "task", "owner")
        .with_durable_command_commits()
        .with_managed_recovery()
        .with_preparation_pending();
    let database = profile.store_root().join("docs.sqlite3");
    let harness = Arc::new(ControlledCodex {
        requests: Mutex::new(Vec::new()),
    });
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let core = Arc::new(
        EngineCore::assemble_with_profile(profile, registry, HarnessId::Codex, None).unwrap(),
    );
    core.rpc_service().handle(methods::MUTATE,json!({"op":"createChat","chatId":"thread","deviceId":core.device_id,"cwd":checkout,"config":{"harness":"codex","model":"fixture","sandbox":"workspace-write"}})).await.unwrap();
    core.rpc_service()
        .handle(
            methods::CREATE_THREAD_SESSION,
            json!({"threadId":"thread","chatId":"session"}),
        )
        .await
        .unwrap();
    let boot:CloudRuntimeBootstrap=serde_json::from_value(json!({"workspaceId":"task","zuseAccountId":"owner","chatId":"thread","initialSessionId":"session","runtimeCredential":"synthetic","runtimeGeneration":1,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9007199254740000i64,"gatewayUrl":"wss://fixture.invalid","gatewayProtocol":"fixture","sealedTranscriptKey":"synthetic","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"},"providerSandboxId":"machine","agent":"codex","commandAdmissionVersion":2,"commandPolicyRevision":1,"commandDeliveryMode":"server-admitted-v2"})).unwrap();
    let progress = Publisher::open(&database, &boot, &core).unwrap();
    core.doc_host.require_canonical_cloud_commands();
    core.sessions
        .complete_preparation(core.sessions.preparation_epoch())
        .unwrap();
    let asset = Asset {
        asset_id: "asset".into(),
        mime_type: "text/plain".into(),
        original_name: "proof.txt".into(),
        size_bytes: b"verified attachment".len(),
        sha256: B64.encode(Sha256::digest(b"verified attachment")),
    };
    let command:CloudSessionCommand=serde_json::from_value(json!({"commandId":"run","sessionId":"session","runtimeBinding":{"version":2,"policyRevision":1,"cwdRelative":".","attachments":[asset]},"payload":{"kind":"run","messageId":"message","request":{"prompt":"read attachment","harness":"codex","model":"fixture","cwd":".","sandbox":"workspace-write"}}})).unwrap();
    // Target a past, unrelated turn: this control must not cancel the later
    // explicitly admitted run, but its admission must not wait on bulk input.
    let stop:CloudSessionCommand=serde_json::from_value(json!({"commandId":"stop","sessionId":"session","basedOn":{"turnId":"past-turn"},"runtimeBinding":{"version":2,"policyRevision":1,"cwdRelative":"."},"payload":{"kind":"interrupt"}})).unwrap();
    let stop = if stop_inputs {
        serde_json::from_value(json!({"commandId":"stop","sessionId":"session","runtimeBinding":{"version":2,"policyRevision":1,"cwdRelative":".","interruptCommandId":"run","interruptMessageId":"message"},"payload":{"kind":"interrupt"}})).unwrap()
    } else {
        stop
    };
    let key = CommandKey::parse(&B64.encode([49; 32])).unwrap();
    let initial = sealed(&key, &command, 1);
    let state = Arc::new(Server {
        leases: Mutex::new(VecDeque::from([initial.clone(), sealed(&key, &stop, 2)])),
        asset,
        asset_entered: Semaphore::new(0),
        release: Semaphore::new(0),
        acks: Mutex::new(Vec::new()),
        reports: Mutex::new(Vec::new()),
        asset_reads: AtomicUsize::new(0),
    });
    let app = Router::new()
        .route(
            "/v1/cloud/workspaces/task/runtime/commands/lease",
            post(lease),
        )
        .route("/v1/cloud/workspaces/task/runtime/commands/ack", post(ack))
        .route(
            "/v1/cloud/workspaces/task/runtime/commands/progress",
            post(report),
        )
        .route(
            "/v1/cloud/workspaces/task/runtime/attachments/asset",
            get(download_asset),
        )
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let authority = Authority::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (_credentials, updates) = watch::channel(boot.clone());
    let executor = Arc::new(EngineExecutor {
        core: core.clone(),
        thread: "thread".into(),
        checkout: checkout.clone(),
        allow_test: false,
        task_context: Some(
            TaskContext::new(authority.clone(), updates.clone(), &checkout).unwrap(),
        ),
        progress: Some(progress.clone()),
    });
    let mailbox = Arc::new(Mailbox::new(&database, &boot, key, executor).unwrap());
    let cancel = CancellationToken::new();
    let execution = tokio::spawn({
        let mailbox = mailbox.clone();
        let authority = authority.clone();
        let updates = updates.clone();
        let cancel = cancel.clone();
        async move { mailbox.run(authority, updates, cancel).await }
    });
    let reporting = tokio::spawn({
        let progress = progress.clone();
        let authority = authority.clone();
        let updates = updates.clone();
        let cancel = cancel.clone();
        async move { progress.run(authority, updates, cancel).await }
    });
    tokio::time::timeout(Duration::from_secs(2), state.asset_entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if state
                .acks
                .lock()
                .unwrap()
                .iter()
                .any(|ack| ack["commandId"] == "stop")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(harness.requests.lock().unwrap().is_empty());
    state.release.add_permits(1);
    if stop_inputs {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state
                    .acks
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|ack| ack["commandId"] == "run" && ack["state"] == "outcome-unknown")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let previous = mailbox.apply(&initial).await.unwrap().unwrap();
        assert!(matches!(
            previous.state,
            zuse_protocol::cloud_commands::AcknowledgmentState::OutcomeUnknown
        ));
        assert!(
            harness.requests.lock().unwrap().is_empty(),
            "stopped input preparation reached the harness"
        );
        // A later explicit send has its own identity, and is not cancelled by
        // the old turn's durable Stop or its delayed receipt.
        let mut later = serde_json::to_value(&command).unwrap();
        later["commandId"] = json!("later");
        later["payload"]["messageId"] = json!("later-message");
        later["runtimeBinding"]["attachments"] = json!([]);
        let later: CloudSessionCommand = serde_json::from_value(later).unwrap();
        let key = CommandKey::parse(&B64.encode([49; 32])).unwrap();
        mailbox.apply(&sealed(&key, &later, 3)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while !state
                .reports
                .lock()
                .unwrap()
                .iter()
                .any(|report| report["commandId"] == "later" && report["state"] == "completed")
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(harness.requests.lock().unwrap().len(), 1);
    } else {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if state
                    .reports
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|report| report["commandId"] == "run" && report["state"] == "completed")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(harness.requests.lock().unwrap().len(), 1);
        assert_eq!(state.asset_reads.load(Ordering::SeqCst), 1);
        let request = harness.requests.lock().unwrap()[0].clone();
        assert_eq!(
            std::fs::read(&request.attachments[0]).unwrap(),
            b"verified attachment"
        );
        assert_eq!(
            std::fs::canonicalize(request.cwd).unwrap(),
            std::fs::canonicalize(&checkout).unwrap()
        );
        assert!(mailbox.apply(&initial).await.unwrap().is_some());
        assert_eq!(
            harness.requests.lock().unwrap().len(),
            1,
            "lost response retry must not dispatch again"
        );
    }
    let guard = crate::engine::CanonicalTaskRpc::new(core.rpc_service());
    assert!(
        guard
            .handle(methods::QUEUE_COMMAND, json!({"chatId":"session"}))
            .await
            .is_err()
    );
    cancel.cancel();
    execution.await.unwrap().unwrap();
    reporting.await.unwrap().unwrap();
    server.abort();
    core.shutdown().await;
}

#[tokio::test]
async fn sealed_asset_turn_uses_real_executor_and_reports_factual_completion_once() {
    exercise_asset_turn(false).await;
}
#[tokio::test]
async fn exact_stop_during_asset_preparation_never_runs_later_or_cancels_a_new_send() {
    exercise_asset_turn(true).await;
}

#[tokio::test]
async fn late_exact_stop_cannot_terminate_a_newer_active_message() {
    let dir = tempfile::tempdir().unwrap();
    let profile = EngineProfile::synced(dir.path(), "task", "owner")
        .with_durable_command_commits()
        .with_managed_recovery();
    let harness = Arc::new(ControlledCodex {
        requests: Mutex::new(Vec::new()),
    });
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let core = Arc::new(
        EngineCore::assemble_with_profile(profile, registry, HarnessId::Codex, None).unwrap(),
    );
    core.rpc_service().handle(methods::MUTATE,json!({"op":"createChat","chatId":"session","deviceId":core.device_id,"cwd":dir.path(),"config":{"harness":"codex","model":"fixture","sandbox":"workspace-write"}})).await.unwrap();
    let request:RunRequest=serde_json::from_value(json!({"prompt":"wait for exact stop","harness":"codex","model":"fixture","cwd":dir.path(),"sandbox":"workspace-write"})).unwrap();
    core.sessions
        .dispatch(
            "session",
            HarnessId::Codex,
            request.clone(),
            Some("new-message".into()),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while harness.requests.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        !core
            .sessions
            .stop_admitted_message("session", "old-message")
            .await
            .unwrap()
    );
    assert!(core.sessions.turn_in_flight("session"));
    assert!(
        core.sessions
            .stop_admitted_message("session", "new-message")
            .await
            .unwrap()
    );
    let cancelled = core
        .sessions
        .dispatch(
            "session",
            HarnessId::Codex,
            request,
            Some("old-message".into()),
        )
        .await
        .unwrap();
    assert!(cancelled.is_empty());
    assert_eq!(harness.requests.lock().unwrap().len(), 1);
    core.shutdown().await;
}

#[tokio::test]
async fn native_start_error_before_stream_is_unknown_and_never_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let profile = EngineProfile::synced(dir.path(), "task", "owner")
        .with_durable_command_commits()
        .with_managed_recovery();
    let database = profile.store_root().join("docs.sqlite3");
    let harness = Arc::new(ControlledCodex {
        requests: Mutex::new(Vec::new()),
    });
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(harness.clone());
    let core = Arc::new(
        EngineCore::assemble_with_profile(profile, registry, HarnessId::Codex, None).unwrap(),
    );
    core.rpc_service().handle(methods::MUTATE,json!({"op":"createChat","chatId":"session","deviceId":core.device_id,"cwd":dir.path(),"config":{"harness":"codex","model":"fixture","sandbox":"workspace-write"}})).await.unwrap();
    let boot:CloudRuntimeBootstrap=serde_json::from_value(json!({"workspaceId":"task","zuseAccountId":"owner","chatId":"session","initialSessionId":"session","runtimeCredential":"synthetic","runtimeGeneration":1,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9007199254740000i64,"gatewayUrl":"wss://fixture.invalid","gatewayProtocol":"fixture","sealedTranscriptKey":"synthetic","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"},"commandAdmissionVersion":2})).unwrap();
    let progress = Publisher::open(&database, &boot, &core).unwrap();
    let command:CloudSessionCommand=serde_json::from_value(json!({"commandId":"run","sessionId":"session","runtimeBinding":{"version":2,"policyRevision":1,"cwdRelative":"."},"payload":{"kind":"run","messageId":"message","request":{"prompt":"controlled startup failure","harness":"codex","model":"fixture","cwd":dir.path(),"sandbox":"workspace-write"}}})).unwrap();
    progress.register(&command, "hmac-sha256:fixture").unwrap();
    let entry:zuse_doc::SessionCommandEntry=serde_json::from_value(json!({"id":"run","payload":command.payload,"issuedBy":"cloud-mailbox:hmac-sha256:fixture","issuedAt":crate::now_ms(),"expiresAt":9007199254740000i64,"status":"pending"})).unwrap();
    core.doc_host
        .admit_mailbox_command("session", entry.clone())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let state: String = progress
                .connect()
                .unwrap()
                .query_row(
                    "SELECT state FROM zuse_task_execution_progress WHERE command_id='run'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            if state == "outcome-unknown" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    core.doc_host
        .admit_mailbox_command("session", entry)
        .await
        .unwrap();
    let handle = core.doc_host.open_local("session").unwrap();
    core.doc_host.drain_commands(&handle).await;
    assert_eq!(harness.requests.lock().unwrap().len(), 1);
    core.shutdown().await;
}
