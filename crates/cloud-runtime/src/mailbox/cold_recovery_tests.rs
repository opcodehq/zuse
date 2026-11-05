//! Real process-loss boundary: a fake ACP peer executes a real shell action and
//! SIGKILLs its Rust host before the host can write a terminal delivery ACK.
use super::*;
use crate::engine::EngineExecutor;
use zuse_engine::{EngineCore, EngineProfile, registry::HarnessRegistry};
use zuse_protocol::{HarnessId, SandboxLevel};
use zuse_rpc::RpcService;
fn bootstrap(generation: i64) -> CloudRuntimeBootstrap {
    serde_json::from_value(json!({"workspaceId":"workspace","zuseAccountId":"owner","chatId":"session","initialSessionId":"session","runtimeCredential":"fixture-runtime","runtimeGeneration":generation,"gatewayEpoch":generation,"runtimeCredentialExpiresAt":9007199254740000i64,"gatewayUrl":"wss://fixture.invalid","gatewayProtocol":"fixture","sealedTranscriptKey":"fixture","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"},"providerSandboxId":format!("machine-{generation}")})).unwrap()
}
fn lease(key: &CommandKey, generation: i64) -> RuntimeLease {
    let body=json!({"commandId":"command","sessionId":"session","clientMessageId":"message","text":"execute one action"}).to_string();
    let mut envelope:CommandEnvelope=serde_json::from_value(json!({"protocolVersion":3,"workspaceId":"workspace","sessionId":"session","commandId":"command","kind":"messages.send","fingerprint":key.fingerprint(body.as_bytes()),"schemaVersion":1,"keyVersion":1,"destructionFence":1,"createdAt":now_ms(),"iv":"","ciphertext":"","dependencies":[]})).unwrap();
    (envelope.iv, envelope.ciphertext) = key.encrypt(&envelope, body.as_bytes()).unwrap();
    RuntimeLease {
        command: envelope,
        workspace_sequence: 1,
        lease_token: format!("lease-{generation}"),
        lease_deadline: now_ms() + 120000,
        runtime_generation: generation,
        provider_sandbox_id: format!("machine-{generation}"),
        storage_incarnation_id: "disk".into(),
    }
}
fn profile(root: &Path) -> EngineProfile {
    EngineProfile::synced(root, "workspace", "owner")
        .with_durable_command_commits()
        .with_managed_recovery()
}
fn registry(root: &Path) -> Arc<HarnessRegistry> {
    let registry = Arc::new(HarnessRegistry::new());
    registry.register(Arc::new(
        zuse_providers::AcpHarness::grok().with_executable(root.join("fake-acp.py")),
    ));
    registry
}
const PEER: &str = r#"#!/usr/bin/env python3
import json,os,signal,subprocess,sys,time
from pathlib import Path
cwd=None
def emit(value):
    print(json.dumps(value),flush=True)
for line in sys.stdin:
    frame=json.loads(line);method=frame.get('method');ident=frame.get('id');params=frame.get('params',{})
    if method=='initialize':
        emit({'id':ident,'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True}}})
    elif method in ('session/new','session/load'):
        cwd=Path(params.get('cwd',os.getcwd()))
        emit({'id':ident,'result':{'sessionId':'retained-native-session','models':{'currentModelId':'fixture','availableModels':[{'modelId':'fixture','name':'Fixture'}]}}})
    elif method=='session/prompt':
        emit({'method':'session/update','params':{'sessionId':'retained-native-session','update':{'sessionUpdate':'tool_call','toolCallId':'native-tool','title':'Write one durable marker','kind':'execute','status':'in_progress','rawInput':{'command':'write fixture side effect'}}}})
        (cwd/'waiting-tool').write_text('ready')
        if (cwd/'mode').read_text()=='crash':
            deadline=time.monotonic()+10
            while not (cwd/'flushed').exists():
                assert time.monotonic()<deadline,'host did not flush retained transcript'
                time.sleep(.005)
        subprocess.run(['/bin/sh','-c','printf x >> "$1"','_',str(cwd/'side-effects')],check=True)
        with (cwd/'side-effects').open('r+b') as output:os.fsync(output.fileno())
        if (cwd/'mode').read_text()=='crash':
            os.kill(int((cwd/'parent.pid').read_text()),signal.SIGKILL)
            os._exit(0)
        emit({'id':ident,'result':{'stopReason':'end_turn'}})
    elif ident is not None:emit({'id':ident,'result':{}})
"#;
struct LostAdmissionReply(EngineExecutor);
#[async_trait]
impl Executor for LostAdmissionReply {
    async fn admitted(&self, p: &MessageSend, f: &str) -> Result<bool> {
        self.0.admitted(p, f).await
    }
    async fn native_admitted(&self, p: &CloudSessionCommand, f: &str) -> Result<bool> {
        self.0.native_admitted(p, f).await
    }
    async fn admit_native(&self, p: &CloudSessionCommand, f: &str, at: i64) -> Result<()> {
        self.0.admit_native(p, f, at).await?;
        std::future::pending().await
    }
    async fn admit(&self, p: &MessageSend, f: &str, at: i64) -> Result<()> {
        self.0.admit(p, f, at).await?;
        std::future::pending().await
    }
}
#[tokio::test]
async fn managed_process_loss_keeps_unknown_receipts_and_never_repeats_tool() {
    if let Some(root) = std::env::var_os("ZUSE_MANAGED_CRASH_FIXTURE") {
        let root = std::path::PathBuf::from(root);
        let checkout = root.join("repo");
        std::fs::write(checkout.join("parent.pid"), std::process::id().to_string()).unwrap();
        let profile = profile(&root);
        let database = profile.store_root().join("docs.sqlite3");
        let core = Arc::new(
            EngineCore::assemble_with_profile(profile, registry(&root), HarnessId::Grok, None)
                .unwrap(),
        );
        core.rpc_service().handle(zuse_protocol::methods::MUTATE,json!({"op":"createChat","chatId":"session","deviceId":core.device_id,"cwd":checkout,"config":{"harness":HarnessId::Grok,"model":"fixture","modelOptions":{},"sandbox":SandboxLevel::WorkspaceWrite}})).await.unwrap();
        let db = Connection::open(&database).unwrap();
        db.execute_batch("CREATE TABLE zuse_cloud_storage_identity(singleton INTEGER PRIMARY KEY,workspace_id TEXT,account_id TEXT,storage_incarnation_id TEXT); INSERT INTO zuse_cloud_storage_identity VALUES(1,'workspace','owner','disk')").unwrap();
        drop(db);
        let flusher = core.clone();
        let directory = checkout.clone();
        tokio::spawn(async move {
            while !directory.join("waiting-tool").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let handle = flusher.doc_host.open_local("session").unwrap();
            assert!(
                handle
                    .doc()
                    .read_entries()
                    .unwrap()
                    .iter()
                    .any(|entry| entry.id == "message")
            );
            flusher.doc_host.flush_all_checked().unwrap();
            std::fs::write(directory.join("flushed"), "durable").unwrap();
        });
        let key = CommandKey::parse(&B64.encode([73; 32])).unwrap();
        let lease = lease(&key, 1);
        let mailbox = Mailbox::new(
            &database,
            &bootstrap(1),
            key,
            Arc::new(LostAdmissionReply(EngineExecutor {
                core,
                thread: "session".into(),
                checkout,
                allow_test: false,
                task_context: None,
                progress: None,
            })),
        )
        .unwrap();
        let _ = mailbox.apply(&lease).await;
        panic!("fixture must lose its process before ACK");
    }
    let root = tempfile::tempdir().unwrap();
    let checkout = root.path().join("repo");
    std::fs::create_dir(&checkout).unwrap();
    std::fs::write(checkout.join("mode"), "crash").unwrap();
    std::fs::write(checkout.join("dirty"), "preserve uncommitted work").unwrap();
    use std::os::unix::fs::PermissionsExt;
    let peer = root.path().join("fake-acp.py");
    std::fs::write(&peer, PEER).unwrap();
    std::fs::set_permissions(&peer, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut child=tokio::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","mailbox::cold_recovery_tests::managed_process_loss_keeps_unknown_receipts_and_never_repeats_tool","--nocapture"]).env("ZUSE_MANAGED_CRASH_FIXTURE",root.path()).kill_on_drop(true).spawn().unwrap();
    let status = tokio::time::timeout(Duration::from_secs(25), child.wait())
        .await
        .expect("actual fake ACP tool did not reach crash point")
        .unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(libc::SIGKILL));
    assert_eq!(std::fs::read(checkout.join("side-effects")).unwrap(), b"x");
    std::fs::write(checkout.join("mode"), "observe").unwrap();
    let profile = profile(root.path());
    let database = profile.store_root().join("docs.sqlite3");
    let key = CommandKey::parse(&B64.encode([73; 32])).unwrap();
    assert_eq!(
        cold_recovery::quarantine_missing_ack(&database, &bootstrap(2), &key).unwrap(),
        1
    );
    assert_eq!(
        cold_recovery::quarantine_missing_ack(&database, &bootstrap(2), &key).unwrap(),
        0,
        "durable quarantine is idempotent"
    );
    let core = Arc::new(
        EngineCore::assemble_with_profile(profile, registry(root.path()), HarnessId::Grok, None)
            .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(750)).await;
    assert_eq!(
        std::fs::read(checkout.join("side-effects")).unwrap(),
        b"x",
        "cold boot must not resubmit a possibly executed prompt"
    );
    let (journal, _) = core.sessions.subscribe("session", 0).unwrap();
    assert!(journal.iter().any(|entry|matches!(&entry.event,zuse_protocol::AgentEvent::Done{status:zuse_protocol::DoneStatus::Interrupted,error:Some(note),..} if note.contains("send a new message"))));
    let mailbox = Mailbox::new(
        &database,
        &bootstrap(2),
        key,
        Arc::new(EngineExecutor {
            core: core.clone(),
            thread: "session".into(),
            checkout: checkout.clone(),
            allow_test: false,
            task_context: None,
            progress: None,
        }),
    )
    .unwrap();
    let saved = mailbox.receipts.pending().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].state, AcknowledgmentState::OutcomeUnknown);
    let mut replay = mailbox.receipts.unfinished().unwrap();
    assert!(replay.is_empty());
    let original: RuntimeLease = serde_json::from_str(
        &Connection::open(&database)
            .unwrap()
            .query_row(
                "SELECT lease_json FROM zuse_cloud_command_receipts WHERE command_id='command'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
    )
    .unwrap();
    replay.push(RuntimeLease {
        runtime_generation: 2,
        provider_sandbox_id: "machine-2".into(),
        lease_token: "new-delivery-token".into(),
        ..original
    });
    for _ in 0..2 {
        assert_eq!(
            mailbox.apply(&replay[0]).await.unwrap().unwrap().state,
            AcknowledgmentState::OutcomeUnknown
        );
    }
    assert_eq!(std::fs::read(checkout.join("side-effects")).unwrap(), b"x");
    assert_eq!(
        std::fs::read_to_string(checkout.join("dirty")).unwrap(),
        "preserve uncommitted work"
    );
    core.doc_host.shutdown_workers().await;
}

#[test]
fn missing_ack_quarantine_preserves_terminal_receipts_and_rolls_back_cross_scope() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("retained.sqlite3");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE processed_commands(command_id TEXT PRIMARY KEY,processed_at INTEGER); CREATE TABLE zuse_cloud_storage_identity(singleton INTEGER PRIMARY KEY,workspace_id TEXT,account_id TEXT,storage_incarnation_id TEXT); INSERT INTO zuse_cloud_storage_identity VALUES(1,'workspace','owner','disk');").unwrap();
    drop(db);
    let key = CommandKey::parse(&B64.encode([73; 32])).unwrap();
    let lease = lease(&key, 1);
    let receipts = Receipts::open(&path).unwrap();
    receipts.bind(&lease).unwrap();
    let ack = RuntimeAcknowledgment {
        command_id: "command".into(),
        lease_token: "lease-1".into(),
        fingerprint: lease.command.fingerprint.clone(),
        state: AcknowledgmentState::Applied,
        category: None,
        result_iv: Some("original-iv".into()),
        result_ciphertext: Some("original-encrypted-terminal-result".into()),
    };
    receipts.save(&ack).unwrap();
    receipts.delivered(&ack).unwrap();
    assert_eq!(
        cold_recovery::quarantine_missing_ack(&path, &bootstrap(2), &key).unwrap(),
        0
    );
    let db = Connection::open(&path).unwrap();
    let (saved, delivered): (String, i64) = db
        .query_row(
            "SELECT acknowledgment,delivered FROM zuse_cloud_command_receipts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(saved, serde_json::to_string(&ack).unwrap());
    assert_eq!(delivered, 1);
    db.execute(
        "UPDATE zuse_cloud_command_receipts SET acknowledgment=NULL,delivered=0",
        [],
    )
    .unwrap();
    let mut other = bootstrap(2);
    other.zuse_account_id = "different-owner".into();
    assert!(cold_recovery::quarantine_missing_ack(&path, &other, &key).is_err());
    let pending: i64 = db
        .query_row(
            "SELECT count(*) FROM zuse_cloud_command_receipts WHERE acknowledgment IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending, 1);
    let claims: i64 = db
        .query_row("SELECT count(*) FROM processed_commands", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(claims, 0, "rejected scope must not mutate command claims");
    assert_eq!(
        cold_recovery::quarantine_missing_ack(&path, &bootstrap(2), &key).unwrap(),
        1
    );
    let ack: RuntimeAcknowledgment = serde_json::from_str(
        &db.query_row(
            "SELECT acknowledgment FROM zuse_cloud_command_receipts",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(ack.state, AcknowledgmentState::OutcomeUnknown);
}
