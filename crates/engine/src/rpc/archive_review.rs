use super::*;
use crate::archives::CheckoutArchive;
use std::path::Path;

impl EngineRpc {
    pub(super) fn archived_review(
        &self,
        chat_id: Option<&str>,
        cwd: &str,
    ) -> Result<Option<(zuse_protocol::Chat, CheckoutArchive)>, RpcError> {
        let Some(id) = chat_id else { return Ok(None) };
        let chat = self
            .workspace
            .chat(id)
            .map_err(|err| RpcError::Failed(err.to_string()))?
            .ok_or_else(|| RpcError::Failed("Thread not found.".into()))?;
        if chat.device_id != self.doc_host.device_id() {
            return Err(RpcError::Failed(
                "Review must run on the thread’s owning computer.".into(),
            ));
        }
        Ok(self
            .archives
            .review_record(&chat, Path::new(cwd))
            .map_err(|err| RpcError::Failed(err.to_string()))?
            .map(|record| (chat, record)))
    }

    /// One interested archive per viewer, with immutable snapshots cached for
    /// this subscription. No filesystem watchers or checkout rehydration.
    pub(super) fn watch_archive_review(&self, chat_id: String) -> RpcReply {
        let state = (
            self.clone(),
            self.workspace.watch_chats(),
            self.diff_sync.watch_diffs(),
            self.archives.watch_changes(),
            chat_id,
            false,
            None::<(String, String, zuse_protocol::CheckoutDiff)>,
        );
        RpcReply::Stream(
            futures::stream::unfold(
                state,
                |(this, mut chats, mut diffs, mut archives, id, emitted, mut cached)| async move {
                    if emitted {
                        tokio::select! {
                            changed = chats.changed() => { changed.ok()?; },
                            changed = diffs.changed() => { changed.ok()?; },
                            changed = archives.changed() => { changed.ok()?; },
                        }
                    }
                    archives.borrow_and_update();
                    let rows = chats.borrow_and_update().clone();
                    let mut values = diffs.borrow_and_update().clone();
                    let result = async {
                        let chat = rows.iter().find(|chat| chat.id == id).ok_or_else(|| {
                            crate::EngineError::Other("Thread no longer exists.".into())
                        })?;
                        if chat.device_id != this.doc_host.device_id() {
                            return Err(crate::EngineError::Other(
                                "Thread moved to another computer. Reopen its changes.".into(),
                            ));
                        }
                        if let Some(cwd) = chat.cwd.as_deref() {
                            if let Some(record) =
                                this.archives.review_record(chat, Path::new(cwd))?
                            {
                                let checkout = chat.checkout_id.as_deref().unwrap_or("");
                                if cached.as_ref().is_none_or(|(sha, identity, _)| {
                                    sha != &record.checkpoint || identity != checkout
                                }) {
                                    let diff = this
                                        .archives
                                        .review_diff(chat, &record, "workingTree", None, None, None)
                                        .await?;
                                    cached = Some((
                                        record.checkpoint.clone(),
                                        checkout.to_owned(),
                                        diff,
                                    ));
                                }
                                let saved = &cached.as_ref().unwrap().2;
                                values.retain(|diff| {
                                    diff.checkout_id != saved.checkout_id && diff.cwd != saved.cwd
                                });
                                values.push(saved.clone());
                            } else {
                                cached = None;
                            }
                        }
                        serde_json::to_value(&values)
                            .map_err(|err| crate::EngineError::Other(err.to_string()))
                    }
                    .await;
                    let value = match result {
                        Ok(value) => value,
                        Err(error) => serde_json::json!({"error":error.to_string()}),
                    };
                    Some((value, (this, chats, diffs, archives, id, true, cached)))
                },
            )
            .boxed(),
        )
    }
}
