//! New-chat source preparation does not create a thread or run an agent.
use super::*;
use zuse_protocol::creation_source as source;

#[derive(serde::Serialize, serde::Deserialize)]
struct Receipt {
    request: source::ResolveRequest,
    result: source::Resolution,
}

impl EngineRpc {
    async fn creation_source_root(&self, path: &str) -> Result<std::path::PathBuf, RpcError> {
        // Browser projects can be registered repositories before a Space exists.
        for repo in self.repos.list().await {
            if repo.path == path {
                return std::fs::canonicalize(path)
                    .map_err(|_| RpcError::Failed("Project root is unavailable".into()));
            }
        }
        self.change_request_root(path).await
    }

    pub(super) async fn creation_sources(
        &self,
        method: &str,
        mut params: serde_json::Value,
    ) -> Result<RpcReply, RpcError> {
        if let Some(object) = params.as_object_mut() {
            object.remove("targetDeviceId");
        }
        if method == source::LIST {
            let p: source::ListRequest = parse_params(params)?;
            let root = self.creation_source_root(&p.repo_path).await?;
            let rows = if p.tab == source::Tab::Branches {
                self.repos
                    .creation_branches(&root)
                    .await
                    .map_err(|e| RpcError::Failed(e.to_string()))?
            } else {
                let mut rows = self
                    .github
                    .creation_sources(&root, p.tab)
                    .await
                    .map_err(change_request_rpc_error)?;
                if p.tab == source::Tab::PullRequests {
                    let checkouts = self
                        .repos
                        .creation_worktrees(&root)
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                    for row in &mut rows {
                        row.in_use = row
                            .branch
                            .as_ref()
                            .is_some_and(|b| checkouts.contains_key(b));
                    }
                }
                rows
            };
            return RpcReply::value(&rows);
        }
        let mut p: source::ResolveRequest = parse_params(params)?;
        let id = uuid::Uuid::parse_str(&p.request_id)
            .map_err(|_| RpcError::BadParams("Invalid source request ID".into()))?;
        p.request_id = id.to_string();
        let this = self.clone();
        // Preserve an admitted checkout and its receipt if the window disconnects.
        tokio::spawn(async move {
            let root = this.creation_source_root(&p.repo_path).await?;
            let root = std::fs::canonicalize(root).map_err(|e| RpcError::Failed(e.to_string()))?;
            p.repo_path = root.to_string_lossy().into();
            if let Some(space_id) = p.space_id.as_deref() {
                let space = this.local_project_action_space(space_id)?;
                if std::fs::canonicalize(&space.path).ok().as_ref() != Some(&root) {
                    return Err(RpcError::BadParams("Source repository does not match the selected project".into()));
                }
            }
            let gate = this.repos.git_workspace_gate(&format!("creation:{}", root.display())).await;
            let _guard = gate.lock().await;
            let directory = this.repos.data_dir().join("creation-sources");
            let receipt = directory.join(format!("{id}.json"));
            match std::fs::read(&receipt) {
                Ok(bytes) => {
                    let saved: Receipt = serde_json::from_slice(&bytes).map_err(|_| RpcError::Failed("The source receipt could not be read".into()))?;
                    if saved.request != p { return Err(RpcError::BadParams("This source request ID was already used for another choice".into())); }
                    return RpcReply::value(&saved.result);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Err(error) => return Err(RpcError::Failed(error.to_string())),
            }
            let result = match &p.source {
                source::Source::Issue { number } if *number > 0 => {
                    let (title, markdown) = this.github.creation_issue(&root, *number).await.map_err(change_request_rpc_error)?;
                    let encoded = base64::engine::general_purpose::STANDARD.encode(markdown.as_bytes());
                    this.uploads.append(&p.request_id, &encoded, Some(0)).map_err(|e| RpcError::Failed(e.to_string()))?;
                    let attachment_path = this.uploads.commit(&p.request_id, &format!("issue-{number}.md")).map_err(|e| RpcError::Failed(e.to_string()))?;
                    source::Resolution::Issue { number: *number, title, attachment_path, markdown, upload_id: Some(p.request_id.clone()) }
                }
                source::Source::Issue { .. } => return Err(RpcError::BadParams("Invalid issue number".into())),
                source::Source::Branch { reference } if p.cloud => {
                    this.repos.creation_cloud_branch(&root, reference).await.map_err(|e| RpcError::Failed(e.to_string()))?
                }
                source::Source::Pr { number } if p.cloud && *number > 0 => {
                    this.github.creation_cloud_pr(&root, *number).await.map_err(change_request_rpc_error)?
                }
                source::Source::Pr { .. } if p.cloud => return Err(RpcError::BadParams("Invalid pull request number".into())),
                _ => {
                    let reply = this.handle(methods::CREATE_WORKTREE, serde_json::json!({"repoPath":p.repo_path,"branch":"HEAD","spaceId":p.space_id,"source":{"source":p.source,"requestId":p.request_id}})).await?;
                    let RpcReply::Value(value) = reply else { return Err(RpcError::Failed("Source checkout response was interrupted".into())); };
                    let outcome: CreateWorktreeOutcome = serde_json::from_value(value).map_err(|e| RpcError::Failed(e.to_string()))?;
                    let label = match &p.source {
                        source::Source::Pr { number } => format!("PR #{number} · {}", outcome.worktree.branch),
                        _ => outcome.worktree.branch.clone(),
                    };
                    source::Resolution::Checkout { label, outcome }
                }
            };
            std::fs::create_dir_all(&directory).map_err(|e| RpcError::Failed(e.to_string()))?;
            let bytes = serde_json::to_vec(&Receipt { request:p, result:result.clone() }).map_err(|e| RpcError::Failed(e.to_string()))?;
            let temporary = directory.join(format!("{id}.pending"));
            std::fs::write(&temporary, bytes).and_then(|_| std::fs::rename(&temporary, &receipt)).map_err(|e| RpcError::Failed(e.to_string()))?;
            RpcReply::value(&result)
        }).await.map_err(|e| RpcError::Failed(e.to_string()))?
    }
}
