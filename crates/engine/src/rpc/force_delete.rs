use super::*;
use std::{collections::HashSet, path::Path};

impl EngineRpc {
    pub(super) async fn force_delete_chat(&self, chat_id: &str) -> Result<(), RpcError> {
        // Once admitted, finish even if the renderer disconnects.
        let this = self.clone();
        let chat_id = chat_id.to_owned();
        tokio::spawn(async move { this.force_delete_chat_inner(&chat_id).await })
            .await
            .map_err(|error| RpcError::Failed(format!("Delete interrupted: {error}")))?
    }

    async fn force_delete_chat_inner(&self, chat_id: &str) -> Result<(), RpcError> {
        let failed = |error: crate::EngineError| RpcError::Failed(error.to_string());
        let _exclusive = self.sessions.archive_exclusive().await;
        let chats = self.workspace.read_chats().map_err(failed)?;
        let Some(chat) = chats.iter().find(|row| row.id == chat_id) else {
            return Ok(());
        };
        if chat.device_id != self.doc_host.device_id() {
            return Err(RpcError::Failed(
                "Delete must run on the session’s owning computer.".into(),
            ));
        }
        let mut family = HashSet::from([chat.id.clone()]);
        loop {
            let before = family.len();
            for row in &chats {
                if row.device_id == chat.device_id
                    && (row
                        .parent_chat_id
                        .as_ref()
                        .is_some_and(|id| family.contains(id))
                        || row.thread_id.as_ref().is_some_and(|id| family.contains(id)))
                {
                    family.insert(row.id.clone());
                }
            }
            if before == family.len() {
                break;
            }
        }
        for id in &family {
            self.sessions.end_for_archive(id).await.map_err(failed)?;
        }
        // Side chats and additional conversations never own the checkout.
        if chat.parent_chat_id.is_none()
            && chat.thread_id.is_none()
            && let Some(cwd) = chat
                .cwd
                .as_deref()
                .map(Path::new)
                .filter(|cwd| cwd.is_absolute() && cwd.exists())
            && let Some(space) = chat
                .space_id
                .as_deref()
                .map(|id| self.workspace.space(id))
                .transpose()
                .map_err(failed)?
                .flatten()
        {
            let repo = Path::new(&space.path);
            let root = self.repos.worktree_settings().effective_directory;
            if self
                .archives
                .managed_checkout(repo, cwd, Path::new(&root))
                .await
                .map_err(failed)?
            {
                let checkout =
                    std::fs::canonicalize(cwd).map_err(|err| RpcError::Failed(err.to_string()))?;
                let sharing = |row: &zuse_protocol::Chat| {
                    row.device_id == chat.device_id
                        && !family.contains(&row.id)
                        && row.cwd.as_deref().is_some_and(|path| {
                            Path::new(path) == checkout
                                || std::fs::canonicalize(path).is_ok_and(|path| path == checkout)
                        })
                };
                if !chats.iter().any(sharing) {
                    if self.terminals.has_live_checkout(&checkout) {
                        return Err(RpcError::Failed(
                            "Close this worktree’s terminals before deleting it.".into(),
                        ));
                    }
                    for id in &family {
                        self.sessions.end_for_archive(id).await.map_err(failed)?;
                    }
                    // Check again after asynchronous shutdown, before removing files.
                    let current = self.workspace.read_chats().map_err(failed)?;
                    if current.iter().any(sharing)
                        || current
                            .iter()
                            .find(|row| row.id == chat.id)
                            .is_none_or(|row| {
                                row.cwd != chat.cwd || row.device_id != chat.device_id
                            })
                    {
                        return Err(RpcError::Failed(
                            "The session or worktree changed. Refresh before deleting it.".into(),
                        ));
                    }
                    self.repos
                        .delete_worktree(repo, &checkout)
                        .await
                        .map_err(failed)?;
                    if checkout.exists() {
                        return Err(RpcError::Failed("Could not remove the worktree. The session was kept; check file permissions and retry.".into()));
                    }
                    self.archives
                        .discard(repo, &checkout)
                        .await
                        .map_err(failed)?;
                }
            }
        }
        // Already archived worktrees may be absent; their saved recovery data
        // still belongs to any other conversation sharing the checkout.
        if chat.parent_chat_id.is_none()
            && chat.thread_id.is_none()
            && let Some(cwd) = chat
                .cwd
                .as_deref()
                .map(Path::new)
                .filter(|cwd| !cwd.exists())
            && let Some(record) = self.archives.record(cwd).map_err(failed)?
        {
            let mut shared = false;
            for row in &chats {
                if row.device_id == chat.device_id
                    && !family.contains(&row.id)
                    && let Some(path) = row.cwd.as_deref().map(Path::new)
                {
                    shared |= path == record.checkout
                        || self
                            .archives
                            .record(path)
                            .map_err(failed)?
                            .is_some_and(|other| other.checkout == record.checkout);
                }
            }
            if !shared {
                self.archives
                    .discard(&record.repo, &record.checkout)
                    .await
                    .map_err(failed)?;
            }
        }
        for id in &family {
            self.sessions.end_for_archive(id).await.map_err(failed)?;
            self.voice.retire_chat(id);
            self.workspace.delete_chat(id).map_err(failed)?;
            self.simulators.forget_chat(id);
            self.doc_host.purge_chat(id);
        }
        self.archives.notify_change();
        Ok(())
    }
}
