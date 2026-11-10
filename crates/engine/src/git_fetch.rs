//! One bounded remote writer per common Git directory, shared by worktrees.
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Weak},
    time::Duration,
};
use tokio::sync::{Mutex, Semaphore};
use tokio::time::Instant;
type Receipt = Result<(), String>;
type Pending = Shared<BoxFuture<'static, Receipt>>;
#[derive(Default)]
struct Entry {
    pending: Option<Pending>,
    finished: Option<(Instant, Receipt, u32)>,
}
struct Inner {
    entries: Mutex<HashMap<String, Entry>>,
    writers: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    slots: Semaphore,
    #[cfg(test)]
    attempts: std::sync::atomic::AtomicUsize,
}
#[derive(Clone)]
pub(crate) struct Fetches(Arc<Inner>);
impl Default for Fetches {
    fn default() -> Self {
        Self(Arc::new(Inner {
            entries: Mutex::new(HashMap::new()),
            writers: Mutex::new(HashMap::new()),
            slots: Semaphore::new(4),
            #[cfg(test)]
            attempts: Default::default(),
        }))
    }
}
impl Fetches {
    pub async fn fetch(&self, root: &Path, remote: &str, explicit: bool) -> Receipt {
        let common = crate::git_workspace::text(
            root,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .await
        .map_err(|e| e.to_string())?;
        let url = crate::git_workspace::text(root, &["remote", "get-url", remote])
            .await
            .map_err(|e| e.to_string())?;
        let key = format!("{common}\0{remote}\0{url}");
        let mut entries = self.0.entries.lock().await;
        if entries.len() >= 256 {
            entries.retain(|_, entry| entry.pending.is_some());
            if entries.len() >= 256 && !entries.contains_key(&key) {
                return Err("Git fetch queue is full. Try again shortly.".into());
            }
        }
        let entry = entries.entry(key.clone()).or_default();
        if let Some(pending) = &entry.pending {
            let pending = pending.clone();
            drop(entries);
            return pending.await;
        }
        if !explicit && let Some((at, result, failures)) = &entry.finished {
            let ttl = if result.is_ok() {
                60
            } else {
                30u64.saturating_mul(1 << (*failures).min(5)).min(900)
            };
            if at.elapsed() < Duration::from_secs(ttl) {
                return result.clone();
            }
        }
        let failures = entry
            .finished
            .as_ref()
            .map_or(0, |(_, result, n)| if result.is_err() { *n } else { 0 });
        let writer = {
            let mut writers = self.0.writers.lock().await;
            writers.retain(|_, writer| writer.strong_count() > 0);
            if let Some(writer) = writers.get(&common).and_then(Weak::upgrade) {
                writer
            } else {
                let writer = Arc::new(Mutex::new(()));
                writers.insert(common.clone(), Arc::downgrade(&writer));
                writer
            }
        };
        let inner = self.0.clone();
        let remote = remote.to_owned();
        let task = tokio::spawn(async move {
            let started = Instant::now();
            let result = std::panic::AssertUnwindSafe(async {
                let _writer = writer.lock().await;
                let _slot = inner.slots.acquire().await.map_err(|e| e.to_string())?;
                // No worktree dependency, tags, pruning or automatic maintenance.
                // A disconnected viewer does not cancel this admitted ref writer.
                #[cfg(test)]
                inner
                    .attempts
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut args = vec!["--git-dir", &common, "fetch", "--quiet", "--no-auto-gc"];
                if !explicit {
                    // Git writes FETCH_HEAD unlocked; a background fetch must
                    // not disturb a pull running beside it.
                    args.extend(["--no-tags", "--no-write-fetch-head"]);
                }
                args.extend(["--", &remote]);
                crate::git_workspace::git(Path::new(&common), &args, &[])
                    .await
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|_| Err("Git fetch stopped unexpectedly.".into()));
            tracing::debug!(
                elapsed_ms = started.elapsed().as_millis() as u64,
                success = result.is_ok(),
                "repository fetch completed"
            );
            if let Some(entry) = inner.entries.lock().await.get_mut(&key) {
                entry.pending = None;
                entry.finished = Some((
                    Instant::now(),
                    result.clone(),
                    if result.is_err() { failures + 1 } else { 0 },
                ));
            }
            result
        });
        let pending = async move {
            task.await
                .map_err(|_| "Git fetch stopped unexpectedly.".to_owned())?
        }
        .boxed()
        .shared();
        entry.pending = Some(pending.clone());
        drop(entries);
        pending.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@localhost")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@localhost")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[tokio::test]
    async fn linked_worktrees_share_fetches_and_explicit_refresh_bypasses_freshness() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        git(root, &["init", "--bare", "remote"]);
        git(root, &["clone", "remote", "checkout"]);
        let checkout = root.join("checkout");
        git(
            &checkout,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
        git(&checkout, &["push", "origin", "HEAD"]);
        git(&checkout, &["worktree", "add", "-b", "other", "../linked"]);
        let linked = root.join("linked");
        let fetches = Fetches::default();
        let (a, b, c) = tokio::join!(
            fetches.fetch(&checkout, "origin", false),
            fetches.fetch(&linked, "origin", false),
            fetches.fetch(&checkout, "origin", false)
        );
        a.unwrap();
        b.unwrap();
        c.unwrap();
        assert_eq!(
            fetches.0.attempts.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        fetches.fetch(&linked, "origin", true).await.unwrap();
        assert_eq!(
            fetches.0.attempts.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        git(&checkout, &["status", "--porcelain"]);
    }
}
