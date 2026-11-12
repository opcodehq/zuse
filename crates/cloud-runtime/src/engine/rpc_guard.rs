//! Opaque gateway/local IPC cannot execute around the task's PG command lane.
use serde_json::Value;
use std::sync::Arc;
use zuse_rpc::{RpcError, RpcReply, RpcService, methods};
pub struct CanonicalTaskRpc {
    inner: Arc<dyn RpcService>,
}
impl CanonicalTaskRpc {
    pub fn new(inner: Arc<dyn RpcService>) -> Arc<dyn RpcService> {
        Arc::new(Self { inner })
    }
}
pub(super) fn blocked(method: &str, params: &Value) -> bool {
    if method == methods::MUTATE {
        return !matches!(
            params["op"].as_str(),
            Some("renameChat" | "markChatSeen" | "changeSidebarPin")
        );
    }
    matches!(
        method,
        methods::QUEUE_COMMAND
            | methods::RELAY_COMMAND
            | methods::RETRY_DELIVERY
            | methods::CREATE_THREAD_SESSION
            | methods::FORK_SIDE_CHAT
            | methods::QUEUE_MESSAGE
            | methods::UPDATE_QUEUED_MESSAGE
            | methods::BEGIN_QUEUED_MESSAGE_EDIT
            | methods::RENEW_QUEUED_MESSAGE_EDIT
            | methods::FINISH_QUEUED_MESSAGE_EDIT
            | methods::MOVE_QUEUED_MESSAGE
            | methods::REMOVE_QUEUED_MESSAGE
            | methods::SEND_QUEUED_MESSAGE_NOW
            | methods::STEER_QUEUED_MESSAGE_NOW
            | methods::WATCH_QUEUE
            | methods::IMPORT_LOCAL_WORKSPACE
            | methods::STOP_ENGINE
            | "ResumeSession"
            | "resumeSession"
            | "SetGoal"
            | "setGoal"
            | "RetryRun"
            | "RetrySession"
            | "Run"
            | "StartRun"
            | "Prompt"
    )
}
#[async_trait::async_trait]
impl RpcService for CanonicalTaskRpc {
    async fn handle(&self, method: &str, params: Value) -> std::result::Result<RpcReply, RpcError> {
        if blocked(method, &params) {
            return Err(RpcError::Failed(
                "cloud_server_command_admission_required".into(),
            ));
        }
        self.inner.handle(method, params).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Recorder(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl RpcService for Recorder {
        async fn handle(&self, _: &str, _: Value) -> std::result::Result<RpcReply, RpcError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            RpcReply::value(&serde_json::json!({"ok":true}))
        }
    }
    #[tokio::test]
    async fn opaque_execution_is_rejected_before_delegation_but_terminal_reads_survive() {
        let count = Arc::new(AtomicUsize::new(0));
        let rpc = CanonicalTaskRpc::new(Arc::new(Recorder(count.clone())));
        for method in [
            methods::QUEUE_COMMAND,
            methods::RELAY_COMMAND,
            methods::RETRY_DELIVERY,
            methods::QUEUE_MESSAGE,
            methods::SEND_QUEUED_MESSAGE_NOW,
            "resumeSession",
            "SetGoal",
        ] {
            assert!(rpc.handle(method, serde_json::json!({})).await.is_err());
        }
        for op in [
            "createChat",
            "setChatConfig",
            "setChatCwd",
            "setChatHost",
            "setChatArchived",
            "deleteChat",
            "forceDeleteChat",
            "unknownFutureAction",
        ] {
            assert!(
                rpc.handle(methods::MUTATE, serde_json::json!({"op":op}))
                    .await
                    .is_err()
            );
        }
        assert_eq!(count.load(Ordering::SeqCst), 0);
        rpc.handle(methods::WATCH_SESSIONS, serde_json::json!({}))
            .await
            .unwrap();
        rpc.handle(methods::SUBSCRIBE_TERMINAL, serde_json::json!({}))
            .await
            .unwrap();
        rpc.handle(methods::MUTATE, serde_json::json!({"op":"markChatSeen"}))
            .await
            .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 3);
    }
}
