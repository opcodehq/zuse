//! User-only recovery controls for a stopped native conversation.
use crate::workspace_host::WorkspaceHost;
use sha2::{Digest, Sha256};
use std::path::Path;
use zuse_harness::{
    Checkpoint, Error, ReceiptStatus, RecoveryDecision, ToolOutput, native::RootStore,
};
use zuse_protocol::recovery::{RecoveryAction, RecoveryCall, RecoverySnapshot, ResolveRecovery};

fn fail(message: &str) -> Error {
    Error::paused(message)
}
fn session(workspace: &WorkspaceHost, chat_id: &str) -> Result<String, Error> {
    let chat = workspace
        .chat(chat_id)
        .map_err(|_| fail("Could not read the conversation."))?
        .ok_or_else(|| fail("Conversation not found."))?;
    if chat.device_id != workspace.device_id() {
        return Err(fail(
            "Review recovery on the device that owns this conversation.",
        ));
    }
    let id = chat
        .harness_session_id
        .ok_or_else(|| fail("This conversation has no saved Zuse session."))?;
    if !id
        .strip_prefix("zuse:")
        .is_some_and(|suffix| uuid::Uuid::parse_str(suffix).is_ok())
    {
        return Err(fail("Recovery review is available for Zuse sessions."));
    }
    Ok(id)
}
fn snapshot(state: &Checkpoint) -> Result<RecoverySnapshot, Error> {
    let mut calls = vec![];
    let mut input_bytes = 0usize;
    for agent in &state.agents {
        for call in &agent.pending_calls {
            let Some(receipt) = state.tools.get(&format!("{}:{}", agent.id, call.id)) else {
                continue;
            };
            if !matches!(receipt.status, ReceiptStatus::Started) {
                continue;
            }
            let bytes = serde_json::to_vec(&(&state.root_id, &agent.id, call, receipt))
                .map_err(|_| Error::Storage)?;
            // Preserve the complete input for inspection; never authorize from a truncated view.
            input_bytes = input_bytes.saturating_add(bytes.len());
            if input_bytes > 1024 * 1024 {
                return Err(fail(
                    "These calls exceed the recovery view limit. Inspect the local journal.",
                ));
            }
            calls.push(RecoveryCall {
                agent_id: agent.id.clone(),
                agent_name: agent.name.clone(),
                call_id: call.id.clone(),
                tool: call.name.clone(),
                input: call.input.clone(),
                revision: format!("{:x}", Sha256::digest(&bytes)),
            });
        }
    }
    Ok(RecoverySnapshot {
        session_id: state.root_id.clone(),
        calls,
        reviewed_count: state.reviews.len(),
    })
}
fn load(store: &RootStore) -> Result<Checkpoint, Error> {
    store.load()?.ok_or_else(|| {
        fail("The saved conversation journal is missing. Restore it before reviewing recovery.")
    })
}
pub(crate) fn inspect(
    profile: &Path,
    workspace: &WorkspaceHost,
    chat_id: &str,
) -> Result<RecoverySnapshot, Error> {
    let id = session(workspace, chat_id)?;
    let store = RootStore::acquire(profile, &id)?;
    snapshot(&load(&store)?)
}
pub(crate) fn resolve(
    profile: &Path,
    workspace: &WorkspaceHost,
    request: ResolveRecovery,
) -> Result<RecoverySnapshot, Error> {
    let id = session(workspace, &request.chat_id)?;
    if request.session_id != id {
        return Err(fail("The conversation changed. Inspect recovery again."));
    }
    let store = RootStore::acquire(profile, &id)?;
    let mut state = load(&store)?;
    let before = snapshot(&state)?;
    if !before.calls.iter().any(|call| {
        call.agent_id == request.agent_id
            && call.call_id == request.call_id
            && call.revision == request.revision
    }) {
        return Err(fail("The tool outcome changed. Inspect recovery again."));
    }
    let decision = match request.decision {
        RecoveryAction::RetryAfterInspection => RecoveryDecision::RetryAfterInspection,
        RecoveryAction::RecordOutcome { output } => {
            if output.trim().is_empty() || output.len() > 65536 {
                return Err(fail("Enter an inspected result of at most 64 KiB."));
            }
            RecoveryDecision::RecordOutcome {
                output: ToolOutput::Text(output),
            }
        }
    };
    // Both the receipt and its review audit are replaced atomically under the root lease.
    state.resolve_unknown(&request.agent_id, &request.call_id, decision)?;
    if session(workspace, &request.chat_id)? != id {
        return Err(fail("The conversation changed. Inspect recovery again."));
    }
    store.save(&state)?;
    snapshot(&state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::{Value, json};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use tokio::sync::Notify;
    use zuse_harness::{
        Call, Engine, Event, Host, Message, ModelInput, ModelOutput, Options, Role, ToolCategory,
        ToolContext, ToolDefinition, Tools, Usage,
    };
    use zuse_protocol::{HarnessId, methods};
    use zuse_rpc::{RpcReply, RpcService};
    struct Fixture {
        models: AtomicUsize,
        executions: AtomicUsize,
        block: AtomicBool,
        started: Notify,
    }
    #[async_trait]
    impl Host for Fixture {
        fn tools(&self) -> Tools {
            [(
                "edit".into(),
                ToolDefinition {
                    description: "Edit fixture".into(),
                    parameters: json!({"type":"object"}),
                    category: ToolCategory::Edit,
                },
            )]
            .into()
        }
        async fn load(&self) -> Result<Option<Checkpoint>, Error> {
            unreachable!("native storage owns journals")
        }
        async fn save(&self, _: &Checkpoint) -> Result<(), Error> {
            unreachable!("native storage owns journals")
        }
        async fn instructions(&self) -> Result<String, Error> {
            Ok("fixture".into())
        }
        async fn model(&self, _: ModelInput) -> Result<ModelOutput, Error> {
            let first = self.models.fetch_add(1, Ordering::AcqRel) == 0;
            Ok(ModelOutput {
                messages: vec![Message::text(Role::Assistant, "fixture")],
                calls: if first {
                    vec![Call {
                        id: "edit-1".into(),
                        name: "edit".into(),
                        input: json!({"path":"reviewed.txt"}),
                    }]
                } else {
                    vec![]
                },
                text: "fixture".into(),
                usage: Usage::default(),
            })
        }
        async fn execute(&self, _: &str, _: Value, _: ToolContext) -> Result<ToolOutput, Error> {
            self.executions.fetch_add(1, Ordering::AcqRel);
            self.started.notify_one();
            if self.block.load(Ordering::Acquire) {
                futures::future::pending::<()>().await;
            }
            Ok(ToolOutput::Text("changed".into()))
        }
        fn notify(&self, _: Event) {}
    }
    async fn review_case(retry: bool) {
        let temp = tempfile::tempdir().unwrap();
        let profile = crate::EngineProfile::local(temp.path()).unwrap();
        let directory = profile.device_root().to_path_buf();
        let core = crate::EngineCore::assemble_with_profile(
            profile,
            Arc::new(crate::HarnessRegistry::new()),
            HarnessId::Zuse,
            None,
        )
        .unwrap();
        let id = format!("zuse:{}", uuid::Uuid::new_v4());
        core.workspace
            .create_chat("chat", None, Some(&core.device_id), None, None)
            .unwrap();
        core.workspace.set_chat_harness_session("chat", &id, "/tmp");
        let fixture = Arc::new(Fixture {
            models: AtomicUsize::new(0),
            executions: AtomicUsize::new(0),
            block: AtomicBool::new(true),
            started: Notify::new(),
        });
        let engine = Engine::open_native(&directory, &id, Options::new("fixture"), fixture.clone())
            .await
            .unwrap();
        let running = {
            let engine = engine.clone();
            tokio::spawn(async move { engine.send("Edit".into(), vec![]).await })
        };
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            fixture.started.notified(),
        )
        .await
        .unwrap();
        let rpc = core.rpc_service();
        assert!(
            rpc.handle(methods::INSPECT_RECOVERY, json!({"chatId":"chat"}))
                .await
                .is_err(),
            "live ownership cannot be bypassed"
        );
        engine.interrupt(None).await.unwrap();
        running.await.unwrap().unwrap();
        engine.close().await.unwrap();
        let RpcReply::Value(value) = rpc
            .handle(
                methods::INSPECT_RECOVERY,
                json!({"chatId":"chat","targetDeviceId":core.device_id}),
            )
            .await
            .unwrap()
        else {
            panic!("unary review")
        };
        let inspection: RecoverySnapshot = serde_json::from_value(value).unwrap();
        assert_eq!(inspection.calls.len(), 1);
        let call = &inspection.calls[0];
        assert_eq!(call.tool, "edit");
        assert_eq!(call.input, json!({"path":"reviewed.txt"}));
        let mut request = ResolveRecovery {
            chat_id: "chat".into(),
            session_id: id.clone(),
            agent_id: call.agent_id.clone(),
            call_id: call.call_id.clone(),
            revision: "stale".into(),
            decision: RecoveryAction::RetryAfterInspection,
        };
        assert!(
            rpc.handle(
                methods::RESOLVE_RECOVERY,
                serde_json::to_value(&request).unwrap()
            )
            .await
            .is_err()
        );
        request.revision = call.revision.clone();
        request.session_id = format!("zuse:{}", uuid::Uuid::new_v4());
        assert!(
            rpc.handle(
                methods::RESOLVE_RECOVERY,
                serde_json::to_value(&request).unwrap()
            )
            .await
            .is_err()
        );
        request.session_id = id.clone();
        request.decision = RecoveryAction::RecordOutcome { output: " ".into() };
        assert!(
            rpc.handle(
                methods::RESOLVE_RECOVERY,
                serde_json::to_value(&request).unwrap()
            )
            .await
            .is_err()
        );
        assert_eq!(fixture.executions.load(Ordering::Acquire), 1);
        request.decision = if retry {
            RecoveryAction::RetryAfterInspection
        } else {
            RecoveryAction::RecordOutcome {
                output: "Inspected the edit: it completed.".into(),
            }
        };
        let RpcReply::Value(value) = rpc
            .handle(
                methods::RESOLVE_RECOVERY,
                serde_json::to_value(&request).unwrap(),
            )
            .await
            .unwrap()
        else {
            panic!("unary decision")
        };
        let after: RecoverySnapshot = serde_json::from_value(value).unwrap();
        assert!(after.calls.is_empty());
        assert_eq!(after.reviewed_count, 1);
        assert_eq!(
            fixture.executions.load(Ordering::Acquire),
            1,
            "saving a review cannot launch a tool"
        );
        assert_eq!(
            fixture.models.load(Ordering::Acquire),
            1,
            "saving a review cannot launch a model"
        );
        assert!(
            rpc.handle(
                methods::RESOLVE_RECOVERY,
                serde_json::to_value(&request).unwrap()
            )
            .await
            .is_err(),
            "stale clicks cannot authorize twice"
        );
        fixture.block.store(false, Ordering::Release);
        let resumed =
            Engine::open_native(&directory, &id, Options::new("fixture"), fixture.clone())
                .await
                .unwrap();
        resumed
            .send("Continue after inspection".into(), vec![])
            .await
            .unwrap();
        assert_eq!(
            fixture.executions.load(Ordering::Acquire),
            if retry { 2 } else { 1 }
        );
        let state = resumed.snapshot().await;
        assert_eq!(state.reviews.len(), 1);
        assert_eq!(
            state.reviews[0].action,
            if retry {
                "retryAfterInspection"
            } else {
                "recordOutcome"
            }
        );
        let history = serde_json::to_string(&state.agent(&id).unwrap().history).unwrap();
        assert!(
            history.contains(if retry {
                "changed"
            } else {
                "Inspected the edit: it completed."
            }),
            "the inspected or retried result must reach durable model history"
        );
        resumed.close().await.unwrap();
        core.workspace
            .create_chat("foreign", None, Some("another-device"), None, None)
            .unwrap();
        core.workspace
            .set_chat_harness_session("foreign", &id, "/tmp");
        assert!(
            rpc.handle(methods::INSPECT_RECOVERY, json!({"chatId":"foreign"}))
                .await
                .is_err()
        );
        assert!(
            rpc.handle(methods::INSPECT_RECOVERY, json!({"chatId":"missing"}))
                .await
                .is_err()
        );
        core.shutdown().await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recovery_rpc_records_an_inspected_outcome_without_replaying_the_tool() {
        review_case(false).await;
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn recovery_rpc_retries_only_after_review_and_explicit_continuation() {
        review_case(true).await;
    }
}
