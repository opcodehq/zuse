//! A preparing computer serves diagnostics while agent execution stays gated.
use serde_json::Value;
use std::{path::PathBuf, sync::Arc};
use zuse_engine::SessionsEngine;
use zuse_rpc::{RpcError, RpcReply, RpcService, methods};

pub struct PreparationRpc {
    inner: Arc<dyn RpcService>,
    sessions: SessionsEngine,
    diagnostic_cwd: PathBuf,
}
impl PreparationRpc {
    pub fn new(
        inner: Arc<dyn RpcService>,
        sessions: SessionsEngine,
        diagnostic_cwd: PathBuf,
    ) -> Arc<dyn RpcService> {
        Arc::new(Self {
            inner,
            sessions,
            diagnostic_cwd,
        })
    }
}
#[async_trait::async_trait]
impl RpcService for PreparationRpc {
    async fn handle(
        &self,
        method: &str,
        mut params: Value,
    ) -> std::result::Result<RpcReply, RpcError> {
        if !self.sessions.agent_preparation_ready() {
            if super::rpc_guard::blocked(method, &params) {
                return Err(RpcError::Failed("runtime_preparation_pending".into()));
            }
            if method == methods::OPEN_TERMINAL {
                let object = params
                    .as_object_mut()
                    .ok_or_else(|| RpcError::Failed("invalid_terminal_request".into()))?;
                // Checkout preparation atomically replaces its directory. A
                // diagnostic PTY must not retain the replaced empty inode.
                object.insert(
                    "cwd".into(),
                    serde_json::to_value(&self.diagnostic_cwd).map_err(|_| {
                        RpcError::Failed("runtime_diagnostic_directory_invalid".into())
                    })?,
                );
            }
        }
        self.inner.handle(method, params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Recorder(Arc<Mutex<Vec<(String, Value)>>>);
    #[async_trait::async_trait]
    impl RpcService for Recorder {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> std::result::Result<RpcReply, RpcError> {
            self.0.lock().unwrap().push((method.into(), params));
            RpcReply::value(&serde_json::json!({"ok":true}))
        }
    }
    #[tokio::test]
    async fn preparing_rpc_keeps_diagnostic_pty_outside_replaced_checkout_and_rejects_agent_bypass()
    {
        let dir = tempfile::tempdir().unwrap();
        let sessions = SessionsEngine::new(
            "preparing-device".into(),
            Arc::new(zuse_engine::RunJournal::open(dir.path()).unwrap()),
            Arc::new(zuse_engine::HarnessRegistry::new()),
        );
        let epoch = sessions.require_preparation();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let rpc = PreparationRpc::new(
            Arc::new(Recorder(calls.clone())),
            sessions.clone(),
            dir.path().into(),
        );
        for method in [
            methods::QUEUE_COMMAND,
            methods::QUEUE_MESSAGE,
            methods::CREATE_THREAD_SESSION,
            "Run",
        ] {
            assert!(rpc.handle(method, serde_json::json!({})).await.is_err());
        }
        assert!(
            rpc.handle(methods::MUTATE, serde_json::json!({"op":"setChatConfig"}))
                .await
                .is_err()
        );
        assert!(calls.lock().unwrap().is_empty());
        rpc.handle(
            methods::OPEN_TERMINAL,
            serde_json::json!({"chatId":"task","cwd":"/will/be/atomically/replaced"}),
        )
        .await
        .unwrap();
        assert_eq!(
            calls.lock().unwrap()[0].1["cwd"],
            dir.path().to_string_lossy().as_ref()
        );
        rpc.handle(
            methods::WRITE_TERMINAL,
            serde_json::json!({"terminalId":"pty","data":"inspect setup"}),
        )
        .await
        .unwrap();
        sessions.complete_preparation(epoch).unwrap();
        rpc.handle(
            methods::QUEUE_COMMAND,
            serde_json::json!({"stableCommandId":"original"}),
        )
        .await
        .unwrap();
        assert_eq!(
            calls.lock().unwrap().last().unwrap().1["stableCommandId"],
            "original"
        );
    }
}

#[cfg(test)]
mod core_tests {
    use super::*;
    use serde_json::json;
    use zuse_engine::{EngineCore, EngineProfile, HarnessRegistry};
    use zuse_protocol::{AgentEvent, DoneStatus, HarnessId};
    #[tokio::test]
    async fn durable_initial_command_stays_pending_until_preparation_then_runs_once() {
        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(HarnessRegistry::new());
        registry.register(Arc::new(zuse_providers::mock::MockHarness {
            script: vec![
                AgentEvent::TextDelta {
                    text: "prepared reply".into(),
                },
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                },
            ],
        }));
        let core = EngineCore::assemble_with_profile(
            EngineProfile::synced(dir.path(), "task", "owner")
                .with_durable_command_commits()
                .with_managed_recovery()
                .with_preparation_pending(),
            registry,
            HarnessId::Mock,
            None,
        )
        .unwrap();
        core.rpc_service().handle(methods::MUTATE,json!({"op":"createChat","chatId":"session","deviceId":core.device_id,"cwd":dir.path(),"config":{"harness":"mock","model":"fixture","sandbox":"workspace-write"}})).await.unwrap();
        let entry:zuse_doc::SessionCommandEntry=serde_json::from_value(json!({"id":"launch:task","payload":{"kind":"run","messageId":"turn:task","request":{"prompt":"one saved first message","cwd":dir.path(),"harness":"mock","model":"fixture","sandbox":"workspace-write"}},"issuedBy":"cloud-mailbox:hmac-sha256:fixture","issuedAt":crate::now_ms(),"expiresAt":9_007_199_254_740_991i64,"status":"pending"})).unwrap();
        core.doc_host
            .admit_mailbox_command("session", entry.clone())
            .await
            .unwrap();
        let handle = core.doc_host.open_local("session").unwrap();
        core.doc_host.drain_commands(&handle).await;
        assert!(core.sessions.watch_sessions().borrow().is_empty());
        assert!(handle.doc().read_entries().unwrap().is_empty());
        // A real diagnostic PTY can start while the first command remains held.
        #[cfg(unix)]
        {
            let terminal = core
                .terminals
                .open_owned("session", dir.path().to_str().unwrap(), 80, 24)
                .unwrap();
            core.terminals
                .write(&terminal.id, "printf 'diagnostic-ready\\n'\n")
                .unwrap();
        }
        core.sessions
            .complete_preparation(core.sessions.preparation_epoch())
            .unwrap();
        core.doc_host.kick_drains();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if handle
                    .doc()
                    .read_entries()
                    .unwrap()
                    .iter()
                    .any(|e| e.id == "turn:task")
                    && !core.sessions.any_active()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        core.doc_host
            .admit_mailbox_command("session", entry)
            .await
            .unwrap();
        core.doc_host.drain_commands(&handle).await;
        assert_eq!(
            handle
                .doc()
                .read_entries()
                .unwrap()
                .iter()
                .filter(|e| e.id == "turn:task")
                .count(),
            1
        );
        core.shutdown().await;
    }
}
