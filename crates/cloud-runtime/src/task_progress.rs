//! A durable factual progress outbox; never a command replay mechanism.
use crate::Authority;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;
use zuse_engine::{
    EngineCore,
    execution_progress::{Fact, State},
};
use zuse_protocol::{
    cloud::CloudRuntimeBootstrap,
    cloud_commands::CloudSessionCommand,
    cloud_task_commands::{ExecutionState, Progress},
    session_commands::SessionCommandPayload,
};
static PROCESS: LazyLock<String> = LazyLock::new(|| uuid::Uuid::new_v4().to_string());
pub struct Publisher {
    database: PathBuf,
    workspace: String,
    generation: i64,
    disk: String,
    changed: Notify,
    fatal: AtomicBool,
}
impl Publisher {
    pub fn open(
        database: &Path,
        boot: &CloudRuntimeBootstrap,
        core: &Arc<EngineCore>,
    ) -> Result<Arc<Self>> {
        ensure!(
            boot.extra.get("commandAdmissionVersion") == Some(&json!(2)),
            "task_progress_protocol_required"
        );
        let disk = boot
            .runtime_storage
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("task_progress_storage_required"))?
            .storage_incarnation_id
            .clone();
        let out = Arc::new(Self {
            database: database.into(),
            workspace: boot.workspace_id.clone(),
            generation: boot.runtime_generation,
            disk,
            changed: Notify::new(),
            fatal: AtomicBool::new(false),
        });
        let mut db = out.connect()?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS zuse_task_execution_progress(command_id TEXT PRIMARY KEY,workspace_id TEXT NOT NULL,session_id TEXT NOT NULL,message_id TEXT,fingerprint TEXT NOT NULL,generation INTEGER NOT NULL,disk TEXT NOT NULL,process_nonce TEXT NOT NULL,state TEXT NOT NULL,revision INTEGER NOT NULL,assistant_id TEXT);CREATE UNIQUE INDEX IF NOT EXISTS zuse_task_execution_mid ON zuse_task_execution_progress(workspace_id,session_id,message_id) WHERE message_id IS NOT NULL;CREATE TABLE IF NOT EXISTS zuse_task_execution_outbox(ordinal INTEGER PRIMARY KEY AUTOINCREMENT,command_id TEXT NOT NULL,revision INTEGER NOT NULL,body TEXT NOT NULL,UNIQUE(command_id,revision));")?;
        // An admission ACK is not proof of execution. A cold process may have
        // died with an admitted local queue entry, so retire that entry before
        // recording uncertainty; opening the preparation gate must never replay it.
        let stale: Vec<(String, String, String)> = {
            let mut rows = db.prepare("SELECT command_id,session_id,fingerprint FROM zuse_task_execution_progress WHERE workspace_id=?1 AND generation=?2 AND disk=?3 AND process_nonce<>?4 AND state IN ('starting','running','waiting-input')")?;
            let collected = rows
                .query_map(
                    params![out.workspace, out.generation, out.disk, PROCESS.as_str()],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?
                .collect::<rusqlite::Result<_>>()?;
            collected
        };
        for (command, session, fingerprint) in &stale {
            if core.workspace.chat(session)?.is_some() {
                core.doc_host
                    .quarantine_mailbox_command(session, command, fingerprint)?;
            }
        }
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for (command, _, _) in stale {
            out.append_tx(
                &tx,
                &command,
                ExecutionState::OutcomeUnknown,
                None,
                Some("runtime-process-lost"),
            )?;
        }
        tx.commit()?;
        let sink = out.clone();
        core.sessions.set_execution_observer(Arc::new(move |fact| {
            sink.record(fact).map_err(|_| {
                sink.fatal.store(true, Ordering::Release);
                sink.changed.notify_one();
                zuse_engine::EngineError::Other("runtime_execution_progress_unavailable".into())
            })
        }))?;
        Ok(out)
    }
    fn connect(&self) -> Result<Connection> {
        ensure!(self.database.is_file(), "task_progress_storage_missing");
        let db = Connection::open_with_flags(
            &self.database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "synchronous", "FULL")?;
        Ok(db)
    }
    pub fn register(&self, payload: &CloudSessionCommand, fingerprint: &str) -> Result<()> {
        let binding = payload
            .runtime_binding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("task_progress_binding_required"))?;
        ensure!(binding.version == 2, "task_progress_protocol_required");
        let message = match &payload.payload {
            SessionCommandPayload::Run { message_id, .. } => Some(message_id.as_str()),
            SessionCommandPayload::Steer { message_id, .. } => Some(
                message_id
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("task_progress_message_required"))?,
            ),
            _ => None,
        };
        let mut db = self.connect()?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let prior:Option<(String,String,Option<String>,String,i64,String)>=tx.query_row("SELECT workspace_id,session_id,message_id,fingerprint,generation,disk FROM zuse_task_execution_progress WHERE command_id=?1",[&payload.command_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?))).optional()?;
        if let Some(prior) = prior {
            ensure!(
                prior
                    == (
                        self.workspace.clone(),
                        payload.session_id.clone(),
                        message.map(str::to_owned),
                        fingerprint.into(),
                        self.generation,
                        self.disk.clone()
                    ),
                "task_progress_identity_conflict"
            );
            return Ok(());
        }
        tx.execute("INSERT INTO zuse_task_execution_progress VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'queued',0,NULL)",params![payload.command_id,self.workspace,payload.session_id,message,fingerprint,self.generation,self.disk,PROCESS.as_str()])?;
        self.append_tx(
            &tx,
            &payload.command_id,
            ExecutionState::Starting,
            None,
            None,
        )?;
        tx.commit()?;
        self.changed.notify_one();
        Ok(())
    }
    fn record(&self, fact: &Fact) -> Result<()> {
        let mut db = self.connect()?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let command: Option<String> = if let Some(command) = &fact.command_id {
            tx.query_row("SELECT command_id FROM zuse_task_execution_progress WHERE command_id=?1 AND workspace_id=?2 AND session_id=?3 AND generation=?4 AND disk=?5",params![command,self.workspace,fact.chat_id,self.generation,self.disk],|r|r.get(0)).optional()?
        } else {
            tx.query_row("SELECT command_id FROM zuse_task_execution_progress WHERE workspace_id=?1 AND session_id=?2 AND message_id=?3 AND generation=?4 AND disk=?5",params![self.workspace,fact.chat_id,fact.message_id,self.generation,self.disk],|r|r.get(0)).optional()?
        };
        let command =
            command.ok_or_else(|| anyhow::anyhow!("task_progress_unregistered_execution"))?;
        let (state, code) = match fact.state {
            State::Running => (ExecutionState::Running, None),
            State::WaitingInput => (ExecutionState::WaitingInput, None),
            State::Completed => (ExecutionState::Completed, None),
            State::Cancelled => (ExecutionState::Cancelled, Some("turn-interrupted")),
            State::Failed => (ExecutionState::Failed, Some("agent-turn-failed")),
            State::OutcomeUnknown => (
                ExecutionState::OutcomeUnknown,
                Some("execution-outcome-unknown"),
            ),
        };
        self.append_tx(
            &tx,
            &command,
            state,
            fact.assistant_message_id.as_deref(),
            code,
        )?;
        tx.commit()?;
        self.changed.notify_one();
        Ok(())
    }
    fn append_tx(
        &self,
        tx: &rusqlite::Transaction<'_>,
        command: &str,
        state: ExecutionState,
        assistant: Option<&str>,
        code: Option<&str>,
    ) -> Result<()> {
        let (prior,revision,fingerprint,prior_assistant):(String,i64,String,Option<String>)=tx.query_row("SELECT state,revision,fingerprint,assistant_id FROM zuse_task_execution_progress WHERE command_id=?1",[command],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        if matches!(
            prior.as_str(),
            "completed" | "failed" | "cancelled" | "outcome-unknown"
        ) {
            return Ok(());
        }
        let encoded = serde_json::to_value(state)?;
        let state_str = encoded
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("task_progress_state_invalid"))?;
        if prior == state_str && prior_assistant.as_deref() == assistant {
            return Ok(());
        }
        let revision = revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("task_progress_revision_exhausted"))?;
        let report = Progress {
            command_id: command.into(),
            // The local queue issuer retains organization actor metadata for
            // quarantine, while the server authenticates its original HMAC.
            fingerprint: fingerprint
                .split_once(":actor-v1:")
                .map_or(fingerprint.as_str(), |(base, _)| base)
                .to_owned(),
            storage_incarnation_id: self.disk.clone(),
            revision,
            state,
            turn_id: assistant.map(str::to_owned),
            error_code: code.map(str::to_owned),
        };
        tx.execute(
            "INSERT INTO zuse_task_execution_outbox(command_id,revision,body) VALUES(?1,?2,?3)",
            params![command, revision, serde_json::to_string(&report)?],
        )?;
        tx.execute("UPDATE zuse_task_execution_progress SET state=?2,revision=?3,assistant_id=?4 WHERE command_id=?1",params![command,state_str,revision,assistant])?;
        Ok(())
    }
    fn next(&self) -> Result<Option<Progress>> {
        let db = self.connect()?;
        let raw:Option<String>=db.query_row("SELECT o.body FROM zuse_task_execution_outbox o JOIN zuse_task_execution_progress p USING(command_id) WHERE p.workspace_id=?1 AND p.generation=?2 AND p.disk=?3 ORDER BY o.ordinal LIMIT 1",params![self.workspace,self.generation,self.disk],|r|r.get(0)).optional()?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
            .transpose()
    }
    fn acknowledged(&self, report: &Progress, response: &Value) -> Result<()> {
        ensure!(
            response["commandId"] == report.command_id,
            "task_progress_response_invalid"
        );
        let revision = response["revision"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("task_progress_response_invalid"))?;
        ensure!(
            revision == report.revision || response["superseded"] == true,
            "task_progress_response_invalid"
        );
        let mut db = self.connect()?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if response["superseded"] == true && response["terminal"] == true {
            let state: ExecutionState = serde_json::from_value(response["state"].clone())?;
            ensure!(state.terminal(), "task_progress_response_invalid");
            tx.execute(
                "DELETE FROM zuse_task_execution_outbox WHERE command_id=?1",
                [&report.command_id],
            )?;
            tx.execute(
                "UPDATE zuse_task_execution_progress SET state=?2 WHERE command_id=?1",
                params![report.command_id, response["state"].as_str()],
            )?;
        } else {
            tx.execute("DELETE FROM zuse_task_execution_outbox WHERE command_id=?1 AND revision=?2 AND body=?3",params![report.command_id,report.revision,serde_json::to_string(report)?])?;
        }
        tx.commit()?;
        Ok(())
    }
    pub async fn run(
        &self,
        authority: Authority,
        updates: watch::Receiver<CloudRuntimeBootstrap>,
        cancel: CancellationToken,
    ) -> Result<()> {
        loop {
            ensure!(
                !self.fatal.load(Ordering::Acquire),
                "runtime_execution_progress_unavailable"
            );
            if cancel.is_cancelled() {
                return Ok(());
            }
            let changed = self.changed.notified();
            let Some(report) = self.next()? else {
                tokio::select! {_=cancel.cancelled()=>return Ok(()),_=changed=>{}};
                continue;
            };
            let boot = updates.borrow().clone();
            ensure!(
                boot.workspace_id == self.workspace && boot.runtime_generation == self.generation,
                "task_progress_runtime_fenced"
            );
            let result = authority
                .post(
                    &self.workspace,
                    &["runtime", "commands", "progress"],
                    &boot.runtime_credential,
                    &serde_json::to_value(&report)?,
                )
                .await;
            match result {
                Ok(response) => self.acknowledged(&report, &response)?,
                Err(error) if crate::supervisor::retryable(&error) => {
                    tokio::select! {_=cancel.cancelled()=>return Ok(()),_=tokio::time::sleep(Duration::from_secs(1))=>{}}
                }
                Err(error) => return Err(error),
            }
        }
    }
    pub async fn flush(&self, authority: &Authority, boot: &CloudRuntimeBootstrap) -> Result<()> {
        while let Some(report) = self.next()? {
            let response = authority
                .post(
                    &self.workspace,
                    &["runtime", "commands", "progress"],
                    &boot.runtime_credential,
                    &serde_json::to_value(&report)?,
                )
                .await?;
            self.acknowledged(&report, &response)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zuse_engine::{EngineProfile, HarnessRegistry};
    use zuse_protocol::{AgentEvent, DoneStatus, HarnessId, methods};
    use zuse_rpc::RpcService;
    fn boot() -> CloudRuntimeBootstrap {
        serde_json::from_value(json!({"workspaceId":"task","zuseAccountId":"owner","chatId":"session","initialSessionId":"session","runtimeCredential":"synthetic","runtimeGeneration":1,"gatewayEpoch":1,"runtimeCredentialExpiresAt":9007199254740000i64,"gatewayUrl":"wss://fixture.invalid","gatewayProtocol":"fixture","sealedTranscriptKey":"synthetic","runtimeStorage":{"format":"zuse-rust-engine-v1","storageIncarnationId":"disk"},"commandAdmissionVersion":2})).unwrap()
    }
    fn payload(root: &Path) -> CloudSessionCommand {
        serde_json::from_value(json!({"commandId":"command","sessionId":"session","runtimeBinding":{"version":2,"policyRevision":1,"cwdRelative":"."},"payload":{"kind":"run","messageId":"message","request":{"prompt":"execute once","cwd":root,"harness":"mock","model":"fixture","sandbox":"workspace-write"}}})).unwrap()
    }
    async fn setup(root: &Path) -> Arc<EngineCore> {
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(zuse_providers::mock::MockHarness {
            script: vec![
                AgentEvent::TextDelta {
                    text: "one response".into(),
                },
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ],
        }));
        let core = Arc::new(
            EngineCore::assemble_with_profile(
                EngineProfile::synced(root, "task", "owner")
                    .with_durable_command_commits()
                    .with_managed_recovery()
                    .with_preparation_pending(),
                registry,
                HarnessId::Mock,
                None,
            )
            .unwrap(),
        );
        core.rpc_service().handle(methods::MUTATE,json!({"op":"createChat","chatId":"session","deviceId":core.device_id,"cwd":root,"config":{"harness":"mock","model":"fixture","sandbox":"workspace-write"}})).await.unwrap();
        core
    }
    #[tokio::test]
    async fn factual_done_is_durable_and_admission_does_not_complete_execution() {
        let dir = tempfile::tempdir().unwrap();
        let core = setup(dir.path()).await;
        let database = EngineProfile::synced(dir.path(), "task", "owner")
            .store_root()
            .join("docs.sqlite3");
        let progress = Publisher::open(&database, &boot(), &core).unwrap();
        let payload = payload(dir.path());
        let execution_identity = "hmac-sha256:fixture:actor-v1:eyJzdWJqZWN0IjoibWVtYmVyIn0";
        progress.register(&payload, execution_identity).unwrap();
        let entry:zuse_doc::SessionCommandEntry=serde_json::from_value(json!({"id":"command","payload":payload.payload,"issuedBy":format!("cloud-mailbox:{execution_identity}"),"issuedAt":crate::now_ms(),"expiresAt":9007199254740000i64,"status":"pending"})).unwrap();
        core.doc_host
            .admit_mailbox_command("session", entry)
            .await
            .unwrap();
        let first = progress.next().unwrap().unwrap();
        assert_eq!(first.fingerprint, "hmac-sha256:fixture");
        assert!(matches!(first.state, ExecutionState::Starting));
        // A lost HTTP response leaves precisely the same immutable report pending.
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(progress.next().unwrap().unwrap()).unwrap()
        );
        progress
            .acknowledged(
                &first,
                &json!({"commandId":"command","revision":first.revision}),
            )
            .unwrap();
        assert!(progress.next().unwrap().is_none());
        assert!(core.sessions.watch_sessions().borrow().is_empty());
        core.sessions
            .complete_preparation(core.sessions.preparation_epoch())
            .unwrap();
        core.doc_host.kick_drains();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let db = progress.connect().unwrap();
                let state: String = db
                    .query_row(
                        "SELECT state FROM zuse_task_execution_progress WHERE command_id='command'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                if state == "completed" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let db = progress.connect().unwrap();
        let terminal_before: i64 = db
            .query_row(
                "SELECT revision FROM zuse_task_execution_progress WHERE command_id='command'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        progress
            .record(&Fact {
                chat_id: "session".into(),
                message_id: "message".into(),
                command_id: None,
                assistant_message_id: None,
                state: State::Running,
                at: crate::now_ms(),
            })
            .unwrap();
        let terminal_after: i64 = db
            .query_row(
                "SELECT revision FROM zuse_task_execution_progress WHERE command_id='command'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            terminal_before, terminal_after,
            "late events cannot reopen a completed turn"
        );
        core.shutdown().await;
    }
    #[tokio::test]
    async fn lost_authority_response_retries_only_the_same_progress_receipt() {
        use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
        type Seen = Arc<std::sync::Mutex<Vec<Value>>>;
        async fn report(
            State(seen): State<Seen>,
            Json(body): Json<Value>,
        ) -> (StatusCode, Json<Value>) {
            let mut seen = seen.lock().unwrap();
            seen.push(body.clone());
            if seen.len() == 1 {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":{"code":"temporary_failure"}})),
                )
            } else {
                (
                    StatusCode::OK,
                    Json(json!({"commandId":body["commandId"],"revision":body["revision"]})),
                )
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let core = setup(dir.path()).await;
        let database = EngineProfile::synced(dir.path(), "task", "owner")
            .store_root()
            .join("docs.sqlite3");
        let progress = Publisher::open(&database, &boot(), &core).unwrap();
        progress
            .register(&payload(dir.path()), "hmac-sha256:fixture")
            .unwrap();
        let seen: Seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/v1/cloud/workspaces/task/runtime/commands/progress",
                post(report),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let authority =
            Authority::new(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        assert!(progress.flush(&authority, &boot()).await.is_err());
        assert!(progress.next().unwrap().is_some());
        progress.flush(&authority, &boot()).await.unwrap();
        assert!(progress.next().unwrap().is_none());
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], seen[1]);
        drop(seen);
        assert!(
            core.sessions.watch_sessions().borrow().is_empty(),
            "report retry must never dispatch a model"
        );
        server.abort();
        core.shutdown().await;
    }
    #[tokio::test]
    async fn cold_admission_is_quarantined_before_preparation_gate_can_open() {
        let dir = tempfile::tempdir().unwrap();
        let core = setup(dir.path()).await;
        let database = EngineProfile::synced(dir.path(), "task", "owner")
            .store_root()
            .join("docs.sqlite3");
        let progress = Publisher::open(&database, &boot(), &core).unwrap();
        let payload = payload(dir.path());
        progress.register(&payload, "hmac-sha256:fixture").unwrap();
        let entry:zuse_doc::SessionCommandEntry=serde_json::from_value(json!({"id":"command","payload":payload.payload,"issuedBy":"cloud-mailbox:hmac-sha256:fixture","issuedAt":crate::now_ms(),"expiresAt":9007199254740000i64,"status":"pending"})).unwrap();
        core.doc_host
            .admit_mailbox_command("session", entry)
            .await
            .unwrap();
        // A fixture nonce stands for a prior process; the real persisted queue
        // and Engine are reopened, with preparation still closed throughout.
        progress
            .connect()
            .unwrap()
            .execute(
                "UPDATE zuse_task_execution_progress SET process_nonce='retired-process'",
                [],
            )
            .unwrap();
        core.shutdown().await;
        drop(core);
        drop(progress);
        let reopened = setup(dir.path()).await;
        let progress = Publisher::open(&database, &boot(), &reopened).unwrap();
        let state: String = progress
            .connect()
            .unwrap()
            .query_row("SELECT state FROM zuse_task_execution_progress", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(state, "outcome-unknown");
        reopened
            .sessions
            .complete_preparation(reopened.sessions.preparation_epoch())
            .unwrap();
        let handle = reopened.doc_host.open_local("session").unwrap();
        reopened.doc_host.drain_commands(&handle).await;
        assert!(reopened.sessions.watch_sessions().borrow().is_empty());
        assert!(handle.doc().read_entries().unwrap().is_empty());
        assert!(
            handle
                .doc()
                .read_commands()
                .unwrap()
                .iter()
                .all(|row| row.status != zuse_doc::SessionCommandStatus::Pending)
        );
        assert!(
            matches!(
                progress.next().unwrap().unwrap().state,
                ExecutionState::Starting
            ),
            "older immutable report remains ordered before uncertainty"
        );
        reopened.shutdown().await;
    }
    #[tokio::test]
    async fn expired_and_superseded_commands_have_factual_terminal_reports_without_model_calls() {
        let dir = tempfile::tempdir().unwrap();
        let core = setup(dir.path()).await;
        let database = EngineProfile::synced(dir.path(), "task", "owner")
            .store_root()
            .join("docs.sqlite3");
        let progress = Publisher::open(&database, &boot(), &core).unwrap();
        let handle = core.doc_host.open_local("session").unwrap();
        let now = crate::now_ms();
        for (id, issued, expiry) in [
            ("expired", now, 0),
            ("old-stop", now, now + 60000),
            ("new-stop", now + 1, now + 60000),
        ] {
            let payload:CloudSessionCommand=serde_json::from_value(json!({"commandId":id,"sessionId":"session","runtimeBinding":{"version":2,"policyRevision":1,"cwdRelative":"."},"payload":{"kind":"interrupt"}})).unwrap();
            progress.register(&payload, "hmac-sha256:fixture").unwrap();
            let entry:zuse_doc::SessionCommandEntry=serde_json::from_value(json!({"id":id,"payload":payload.payload,"issuedBy":"cloud-mailbox:hmac-sha256:fixture","issuedAt":issued,"expiresAt":expiry,"status":"pending"})).unwrap();
            handle.doc().queue_command(&entry).unwrap();
        }
        core.doc_host.drain_commands(&handle).await;
        let db = progress.connect().unwrap();
        for (id, expected) in [
            ("expired", "cancelled"),
            ("old-stop", "cancelled"),
            ("new-stop", "completed"),
        ] {
            let actual: String = db
                .query_row(
                    "SELECT state FROM zuse_task_execution_progress WHERE command_id=?1",
                    [id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(actual, expected);
        }
        assert!(core.sessions.watch_sessions().borrow().is_empty());
        core.shutdown().await;
    }
    #[tokio::test]
    async fn durable_stop_tombstone_prevents_delayed_run_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let original = setup(dir.path()).await;
        original.doc_host.require_canonical_cloud_commands();
        original
            .doc_host
            .bind_cloud_interrupt("session", "stop", Some(("command", "message")))
            .unwrap();
        assert!(
            original
                .doc_host
                .bind_cloud_interrupt("session", "stop", Some(("other-command", "other-message")))
                .is_err()
        );
        assert!(
            original
                .doc_host
                .bind_cloud_interrupt("other-session", "stop", Some(("command", "message")))
                .is_err()
        );
        original.shutdown().await;
        drop(original);
        let core = setup(dir.path()).await;
        core.doc_host.require_canonical_cloud_commands();
        let database = EngineProfile::synced(dir.path(), "task", "owner")
            .store_root()
            .join("docs.sqlite3");
        let progress = Publisher::open(&database, &boot(), &core).unwrap();
        let payload = payload(dir.path());
        progress.register(&payload, "hmac-sha256:fixture").unwrap();
        let entry:zuse_doc::SessionCommandEntry=serde_json::from_value(json!({"id":"command","payload":payload.payload,"issuedBy":"cloud-mailbox:hmac-sha256:fixture","issuedAt":crate::now_ms(),"expiresAt":9007199254740000i64,"status":"pending"})).unwrap();
        core.doc_host
            .admit_mailbox_command("session", entry)
            .await
            .unwrap();
        core.sessions
            .complete_preparation(core.sessions.preparation_epoch())
            .unwrap();
        let handle = core.doc_host.open_local("session").unwrap();
        core.doc_host.drain_commands(&handle).await;
        let state: String = progress
            .connect()
            .unwrap()
            .query_row(
                "SELECT state FROM zuse_task_execution_progress WHERE command_id='command'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "cancelled");
        assert!(handle.doc().read_entries().unwrap().is_empty());
        assert!(core.sessions.watch_sessions().borrow().is_empty());
        core.shutdown().await;
    }
}

#[cfg(test)]
mod executor_tests;
