//! Durable, opt-in usage-limit continuations. Only provider-supplied reset
//! times are scheduled; an unknown reset always requires a manual resume.
use super::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Pending {
    harness: HarnessId,
    request: RunRequest,
    message_id: Option<String>,
    resets_at: Option<i64>,
    enabled: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    enabled: bool,
    pending: HashMap<String, Pending>,
}

pub(super) struct Store {
    path: PathBuf,
    saved: Saved,
    pub(super) stopped_runs: std::collections::HashSet<String>,
}
impl Store {
    pub(super) fn open(path: PathBuf) -> Self {
        let saved = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                tracing::warn!(%error, "invalid usage resume store; auto-resume disabled");
                Saved::default()
            }),
            Err(_) => Saved::default(),
        };
        Self {
            path,
            saved,
            stopped_runs: Default::default(),
        }
    }
    fn clear_pending(&mut self, chat: &str) -> Result<(), EngineError> {
        if self.saved.pending.contains_key(chat) {
            self.update(|saved| {
                saved.pending.remove(chat);
            })?;
        }
        Ok(())
    }

    fn cancel(&mut self, chat: &str, run_id: Option<&str>) -> Result<(), EngineError> {
        if let Some(run_id) = run_id {
            self.stopped_runs.insert(run_id.to_owned());
        }
        self.clear_pending(chat)
    }

    fn record(&mut self, chat: &str, run_id: &str, pending: Pending) -> Result<(), EngineError> {
        if self.stopped_runs.contains(run_id) {
            return Ok(());
        }
        self.update(|saved| {
            saved.pending.insert(chat.to_owned(), pending);
        })
    }

    fn update(&mut self, change: impl FnOnce(&mut Saved)) -> Result<(), EngineError> {
        let mut next = self.saved.clone();
        change(&mut next);
        let temporary = self.path.with_extension("tmp");
        let bytes = serde_json::to_vec(&next).map_err(|e| EngineError::Other(e.to_string()))?;
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(temporary, &self.path)?;
        self.saved = next;
        Ok(())
    }
}

pub(super) fn is_usage_limit(message: &str) -> bool {
    let text = message.to_lowercase();
    [
        "usage limit",
        "rate limit",
        "rate_limit",
        "ratelimit",
        "quota exceeded",
        "limit reached",
        "hit your limit",
        "too many requests",
    ]
    .iter()
    .any(|s| text.contains(s))
}

impl SessionsEngine {
    pub fn usage_resume_status(&self, chat: Option<&str>) -> serde_json::Value {
        let store = lock(&self.inner.usage_resume);
        let pending = chat.and_then(|id| store.saved.pending.get(id));
        serde_json::json!({
            "enabled": store.saved.enabled,
            "pending": pending.map(|p| serde_json::json!({
                "resetsAt": p.resets_at, "enabled": p.enabled,
            })),
        })
    }

    pub fn set_usage_resume(&self, chat: Option<&str>, enabled: bool) -> Result<(), EngineError> {
        lock(&self.inner.usage_resume).update(|saved| {
            if let Some(chat) = chat {
                if let Some(pending) = saved.pending.get_mut(chat) {
                    pending.enabled = enabled;
                }
            } else {
                saved.enabled = enabled;
                // Turning the preference off also cancels existing schedules.
                for pending in saved.pending.values_mut() {
                    pending.enabled = enabled;
                }
            }
        })?;
        self.start_usage_scheduler();
        Ok(())
    }

    pub(super) fn clear_usage_resume(&self, chat: &str) -> Result<(), EngineError> {
        let mut store = lock(&self.inner.usage_resume);
        store.clear_pending(chat)
    }

    pub(super) fn cancel_usage_resume(&self, chat: &str) -> Result<(), EngineError> {
        // Lock order is runs → usage_resume, also used by remove_run. Holding
        // both prevents a late recording from recreating a stopped schedule.
        let runs = lock(&self.inner.runs);
        let mut store = lock(&self.inner.usage_resume);
        store.cancel(chat, runs.get(chat).map(|run| run.run_id.as_str()))
    }

    pub(super) fn record_usage_limit(
        &self,
        chat: &str,
        run_id: &str,
        harness: HarnessId,
        request: RunRequest,
        message_id: Option<String>,
        resets_at: Option<i64>,
    ) -> Result<(), EngineError> {
        let mut store = lock(&self.inner.usage_resume);
        let enabled = store.saved.enabled;
        store.record(
            chat,
            run_id,
            Pending {
                harness,
                request,
                message_id,
                // A stale timestamp must not cause an immediate retry loop.
                resets_at: resets_at.filter(|time| *time > now_ms()),
                enabled,
            },
        )?;
        drop(store);
        self.start_usage_scheduler();
        Ok(())
    }

    pub async fn resume_usage_limit(&self, chat: &str) -> Result<(), EngineError> {
        self.resume_usage_limit_inner(chat, false).await
    }

    async fn resume_usage_limit_inner(
        &self,
        chat: &str,
        scheduled: bool,
    ) -> Result<(), EngineError> {
        self.require_unarchived(chat)?;
        if self.has_run(chat) {
            return Err(EngineError::Other("This chat is already running.".into()));
        }
        let host = self
            .inner
            .doc_host()
            .ok_or_else(|| EngineError::Other("Chat host unavailable.".into()))?;
        let cwd = if let Some(workspace) = host.workspace() {
            let row = workspace
                .chat(chat)?
                .ok_or_else(|| EngineError::Other("Thread no longer exists.".into()))?;
            if row.device_id != self.inner.device_id {
                return Err(EngineError::Other(
                    "Thread moved to another computer.".into(),
                ));
            }
            row.cwd
        } else {
            None
        };
        let pending = {
            let mut store = lock(&self.inner.usage_resume);
            if scheduled
                && !store
                    .saved
                    .pending
                    .get(chat)
                    .is_some_and(|p| p.enabled && p.resets_at.is_some_and(|at| at <= now_ms()))
            {
                return Ok(());
            }
            let pending = store.saved.pending.get(chat).cloned().ok_or_else(|| {
                EngineError::Other("There is no usage-limit continuation to resume.".into())
            })?;
            if cwd.as_deref().is_some_and(|cwd| cwd != pending.request.cwd) {
                return Err(EngineError::Other(
                    "The chat's working directory changed. Send a new message to continue there."
                        .into(),
                ));
            }
            // Persist the claim before dispatch. Reconnects and multiple clients
            // cannot dispatch the same schedule twice.
            store.update(|saved| {
                saved.pending.remove(chat);
            })?;
            pending
        };
        let mut request = pending.request.clone();
        request.resume = None; // Resolve the provider session using normal dispatch.
        lock(&self.inner.continuing_turns).insert(chat.to_owned());
        if let Err(error) = host
            .dispatch_with_source_context(
                self,
                chat,
                pending.harness,
                request,
                pending.message_id.clone(),
            )
            .await
        {
            lock(&self.inner.continuing_turns).remove(chat);
            let mut pending = pending;
            pending.enabled = false; // A failed admission requires user action.
            lock(&self.inner.usage_resume).update(|saved| {
                saved.pending.entry(chat.to_owned()).or_insert(pending);
            })?;
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn start_usage_scheduler(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if self
            .inner
            .usage_scheduler_started
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                if inner
                    .shutting_down
                    .load(std::sync::atomic::Ordering::Acquire)
                {
                    break;
                }
                let engine = SessionsEngine { inner };
                if !engine.agent_preparation_ready() || engine.inner.doc_host().is_none() {
                    continue;
                }
                let due: Vec<String> = lock(&engine.inner.usage_resume)
                    .saved
                    .pending
                    .iter()
                    .filter(|(_, p)| p.enabled && p.resets_at.is_some_and(|at| at <= now_ms()))
                    .map(|(id, _)| id.clone())
                    .collect();
                for chat in due {
                    if engine.has_run(&chat) {
                        continue;
                    }
                    if let Err(error) = engine.resume_usage_limit_inner(&chat, true).await {
                        tracing::warn!(%chat, %error, "usage-limit auto-resume failed");
                        let _ = engine.set_usage_resume(Some(&chat), false);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_wins_over_recording_in_either_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path().join("resume.json"));
        let pending = || Pending {
            harness: HarnessId::Mock,
            request: serde_json::from_value(
                serde_json::json!({"prompt":"finish","cwd":"/tmp","sandbox":"workspace-write"}),
            )
            .unwrap(),
            message_id: Some("message".into()),
            resets_at: Some(now_ms() + 60_000),
            enabled: true,
        };
        // Stop can arrive before the errored turn persists its continuation.
        store.cancel("chat", Some("stopped-run")).unwrap();
        store.record("chat", "stopped-run", pending()).unwrap();
        assert!(!store.saved.pending.contains_key("chat"));
        // A different chat and a later run in the stopped chat still work.
        store.record("other", "other-run", pending()).unwrap();
        store.record("chat", "new-run", pending()).unwrap();
        assert!(store.saved.pending.contains_key("other"));
        assert!(store.saved.pending.contains_key("chat"));
        // Recording before Stop must also leave no scheduled continuation.
        store.cancel("chat", Some("new-run")).unwrap();
        assert!(!store.saved.pending.contains_key("chat"));
        assert!(
            Store::open(store.path.clone())
                .saved
                .pending
                .contains_key("other")
        );
        assert!(
            !Store::open(store.path.clone())
                .saved
                .pending
                .contains_key("chat")
        );
    }

    #[test]
    fn settings_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume.json");
        let mut store = Store::open(path.clone());
        assert!(!store.saved.enabled);
        store.update(|s| s.enabled = true).unwrap();
        assert!(Store::open(path).saved.enabled);
    }
    #[test]
    fn only_usage_errors_are_recoverable() {
        assert!(is_usage_limit("You've hit your limit"));
        assert!(is_usage_limit("Claude five-hour limit reached"));
        assert!(!is_usage_limit("Invalid API key"));
        assert!(!is_usage_limit("Connection lost"));
    }
}
