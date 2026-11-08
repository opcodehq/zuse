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
        let watching = method == methods::WATCH_GIT_WORKSPACE;
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
            if watching {
                let common = crate::git_workspace::text(
                    &actual.root,
                    &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                )
                .await
                .map_err(|e| RpcError::Failed(e.to_string()))?;
                let worktree_prefix = format!("worktree:{common}:");
                let worktree_key = format!("{worktree_prefix}{}", actual.id);
                let root = actual.root.clone();
                let repos = this.repos.clone();
                let status = this
                    .source_control_refresh
                    .watch(worktree_key.clone(), move || {
                        let root = root.clone();
                        let repos = repos.clone();
                        async move {
                            let snapshot = crate::git_workspace::snapshot(&repos, &root, "")
                                .await
                                .map_err(|e| e.to_string())?;
                            Ok((
                                serde_json::to_value(snapshot).unwrap(),
                                Duration::from_secs(120),
                            ))
                        }
                    });
                // Fetch demand is repository-scoped, and never delays local status.
                let root = std::path::PathBuf::from(&common);
                let repos = this.repos.clone();
                let hub = this.source_control_refresh.clone();
                let sync = this.diff_sync.clone();
                let remote = hub.clone().watch(format!("remote:{common}"), move || {
                    let root = root.clone();
                    let repos = repos.clone();
                    let hub = hub.clone();
                    let sync = sync.clone();
                    let worktree_prefix = worktree_prefix.clone();
                    async move {
                        let remotes = crate::git_workspace::text(&root, &["remote"])
                            .await
                            .map_err(|e| e.to_string())?;
                        for remote in remotes.lines() {
                            repos
                                .fetch_remote(&root, remote, false)
                                .await
                                .map_err(|e| e.to_string())?;
                        }
                        hub.invalidate(&worktree_prefix);
                        sync.sync_all();
                        Ok((serde_json::json!({"ok":true}), Duration::from_secs(60)))
                    }
                });
                let workspace = this.workspace.clone();
                let chats = workspace.watch_chats();
                let chat_id = target.chat_id.clone();
                let files = this.diff_sync.watch_git_statuses();
                let expected_cwd = target.cwd.clone();
                let expected_device = this.doc_host.device_id().to_owned();
                let refresh = this.source_control_refresh.clone();
                let watches = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
                let attach = watches.clone();
                let identity = actual.clone();
                let events_hub = refresh.clone();
                let events_key = worktree_key.clone();
                let discovery = this.change_requests.clone();
                tokio::task::spawn_blocking(move || {
                    let mut targets: Vec<_> = crate::diff_sync::watch_targets(&identity)
                        .into_iter()
                        .map(|p| (p, notify::RecursiveMode::Recursive))
                        .collect();
                    targets.push((
                        std::path::PathBuf::from(&common),
                        notify::RecursiveMode::NonRecursive,
                    ));
                    targets.push((
                        std::path::PathBuf::from(&common).join("refs"),
                        notify::RecursiveMode::Recursive,
                    ));
                    let registrations = targets
                        .into_iter()
                        .map(|(path, mode)| {
                            let hub = events_hub.clone();
                            let key = events_key.clone();
                            let discovery = discovery.clone();
                            crate::fs_watch::FsWatchHub::global().watch(&path, mode, move |event| {
                                if !matches!(event.kind, notify::EventKind::Access(_)) {
                                    hub.touch(&key);
                                    if event.paths.iter().any(|path| {
                                        path.file_name().is_some_and(|name| {
                                            name == "HEAD"
                                                || name == "config"
                                                || name == "packed-refs"
                                        })
                                    }) {
                                        discovery.invalidate();
                                    }
                                }
                            })
                        })
                        .collect();
                    *attach.lock().unwrap() = registrations;
                });
                let stream = futures::stream::unfold(
                    (status, remote, chats, files, None::<String>, false),
                    move |(
                        mut status,
                        mut remote,
                        mut chats,
                        mut files,
                        mut file_revision,
                        emitted,
                    )| {
                        let workspace = workspace.clone();
                        let chat_id = chat_id.clone();
                        let expected_cwd = expected_cwd.clone();
                        let expected_device = expected_device.clone();
                        let refresh = refresh.clone();
                        let worktree_key = worktree_key.clone();
                        let checkout_id = actual.id.clone();
                        async move {
                            if emitted {
                                tokio::select! { result = status.changed() => { result.ok()?; }, result = remote.changed() => { result.ok()?; }, result = chats.changed() => { result.ok()?; }, result = files.changed() => {
                                    result.ok()?;
                                    let next = files.borrow_and_update().iter().find(|s| s.checkout_id == checkout_id).map(|s| s.revision.clone());
                                    if next != file_revision { file_revision = next; refresh.touch(&worktree_key); }
                                } }
                            }
                            let chat = workspace.chat(&chat_id).ok().flatten()?;
                            if chat.archived
                                || chat.device_id != expected_device
                                || chat.cwd.as_deref() != Some(expected_cwd.as_str())
                            {
                                return None;
                            }
                            let mut frame = status.borrow_and_update().clone();
                            if let Some(snapshot) = &mut frame.snapshot {
                                snapshot["chatId"] = serde_json::json!(chat_id);
                            }
                            let remote_frame = remote.borrow_and_update().clone();
                            frame.last_fetch_at = remote_frame.updated_at;
                            frame.fetch_error = remote_frame.error;
                            Some((
                                serde_json::to_value(frame).ok()?,
                                (status, remote, chats, files, file_revision, true),
                            ))
                        }
                    },
                );
                return Ok(RpcReply::Stream(
                    stream
                        .map(move |value| {
                            let _ = &watches;
                            value
                        })
                        .boxed(),
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
                let created =
                    if let zuse_protocol::git_workspace::GitOperation::CreatePullRequest {
                        title,
                        body,
                        base,
                        base_repository,
                        draft,
                    } = &request.operation
                    {
                        if current.upstream.is_none()
                            || current.ahead > 0
                            || current.head_sha.is_none()
                            || current.branch == "HEAD"
                        {
                            return Err(RpcError::Failed(
                                "Push this branch before creating its pull request.".into(),
                            ));
                        }
                        let resolver = crate::source_control::ChangeRequestResolver::new();
                        let source = resolver
                            .inspect_checkout(&actual.root)
                            .await
                            .map_err(change_request_rpc_error)?;
                        if source.branch.local_branch != current.branch {
                            return Err(RpcError::Failed(
                                "The branch changed. Refresh Git before creating a pull request."
                                    .into(),
                            ));
                        }
                        let repository = resolver
                            .repository_for_checkout(&actual.root)
                            .await
                            .ok_or_else(|| {
                                RpcError::Failed(
                                    "This checkout has no supported GitHub remote.".into(),
                                )
                            })?;
                        Some(
                            this.github
                                .create_pull_request(
                                    &repository,
                                    &source.branch.head_branch,
                                    current.head_sha.as_deref().unwrap(),
                                    title,
                                    body,
                                    base,
                                    base_repository.as_deref().filter(|s| !s.trim().is_empty()),
                                    *draft,
                                )
                                .await
                                .map_err(change_request_rpc_error)?,
                        )
                    } else {
                        let result = crate::git_workspace::apply(
                            &this.repos,
                            &actual.root,
                            &current,
                            &request.operation,
                        )
                        .await;
                        this.source_control_refresh.invalidate("worktree:");
                        this.github.invalidate_watches();
                        this.change_requests.invalidate();
                        this.diff_sync.sync_all();
                        result.map_err(|e| RpcError::Failed(e.to_string()))?;
                        None
                    };
                let mut updated =
                    crate::git_workspace::snapshot(&this.repos, &actual.root, &target.chat_id)
                        .await
                        .map_err(|e| RpcError::Failed(e.to_string()))?;
                updated.created_pull_request_url = created;
                this.change_requests.invalidate();
                this.github.invalidate_watches();
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
