use super::*;
use zuse_protocol::git_workspace::{GitAction, GitTarget};
impl EngineRpc {
    pub(super) async fn git_workspace(
        &self,
        method: &str,
        mut params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        if let Some(object) = params.as_object_mut() {
            object.remove("targetDeviceId");
        }
        let target: GitTarget = if method == methods::GIT_WORKSPACE_ACTION {
            let request: GitAction = parse_params(params.clone())?;
            GitTarget {
                chat_id: request.chat_id,
                cwd: request.cwd,
            }
        } else {
            parse_params(params.clone())?
        };
        let this = self.clone();
        let mutation = method == methods::GIT_WORKSPACE_ACTION;
        // An admitted Git action completes even if its caller loses the receipt.
        let task = tokio::spawn(async move {
            let _admission = this.sessions.filesystem_admission().await;
            let chat = this
                .workspace
                .chat(&target.chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?
                .ok_or_else(|| RpcError::Failed("Thread not found.".into()))?;
            if chat.device_id != this.doc_host.device_id() || chat.archived {
                return Err(RpcError::Failed(
                    "Unarchive this thread on its host before using Git controls.".into(),
                ));
            }
            let actual = this
                .repos
                .checkout_identity(std::path::Path::new(chat.cwd.as_deref().unwrap_or("")))
                .await
                .map_err(|e| RpcError::Failed(e.to_string()))?;
            let requested = this
                .repos
                .checkout_identity(std::path::Path::new(&target.cwd))
                .await
                .map_err(|e| RpcError::Failed(e.to_string()))?;
            if actual.id != requested.id {
                return Err(RpcError::BadParams(
                    "The thread checkout changed. Refresh before using Git controls.".into(),
                ));
            }
            let gate = this.repos.git_workspace_gate(&actual.id).await;
            let _guard = gate.lock().await;
            let latest = this
                .workspace
                .chat(&target.chat_id)
                .map_err(|e| RpcError::Failed(e.to_string()))?;
            if latest.is_none_or(|latest| {
                latest.archived || latest.cwd != chat.cwd || latest.device_id != chat.device_id
            }) {
                return Err(RpcError::Failed(
                    "The thread checkout changed. Refresh Git before continuing.".into(),
                ));
            }
            let current =
                crate::git_workspace::snapshot(&this.repos, &actual.root, &target.chat_id)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
            if mutation {
                let request: GitAction = parse_params(params)?;
                if request.checkout_id != current.checkout_id
                    || request.revision != current.revision
                {
                    return Err(RpcError::Failed(
                        "Git state changed. Refresh before trying this action.".into(),
                    ));
                }
                let result =
                    crate::git_workspace::apply(&actual.root, &current, &request.operation).await;
                this.diff_sync.sync_all();
                result.map_err(|e| RpcError::Failed(e.to_string()))?;
                let updated =
                    crate::git_workspace::snapshot(&this.repos, &actual.root, &target.chat_id)
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                this.workspace
                    .reconcile_git_branch(&chat, &updated.checkout_id, &updated.branch)
                    .map_err(|e| RpcError::Failed(e.to_string()))?;
                RpcReply::value(&updated)
            } else {
                RpcReply::value(&current)
            }
        });
        task.await.map_err(|_| {
            RpcError::Failed("Git operation stopped unexpectedly. Refresh before retrying.".into())
        })?
    }
}
