//! Engine-owned demand, freshness and invalidation for source-control reads.
use futures::{FutureExt, future::BoxFuture};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, watch};
use zuse_protocol::git_workspace::SourceControlFrame;

type Loader = Arc<dyn Fn() -> BoxFuture<'static, Result<(Value, Duration), String>> + Send + Sync>;
const CAPACITY: usize = 256;
const LINGER: Duration = Duration::from_secs(30);

struct Entry {
    tx: watch::Sender<SourceControlFrame>,
    kick: Notify,
    epoch: AtomicU64,
    running: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    async fn settled(rx: &mut watch::Receiver<SourceControlFrame>) -> SourceControlFrame {
        loop {
            let frame = rx.borrow_and_update().clone();
            if !frame.refreshing && (frame.snapshot.is_some() || frame.error.is_some()) {
                return frame;
            }
            rx.changed().await.unwrap();
        }
    }

    #[tokio::test(start_paused = true)]
    async fn subscribers_share_reads_and_release_demand() {
        let hub = RefreshHub::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let mut a = hub.watch("worktree:a".into(), move || {
            counter.fetch_add(1, Ordering::SeqCst);
            async {
                Ok((
                    serde_json::json!({"branch":"main"}),
                    Duration::from_secs(60),
                ))
            }
        });
        let mut b = hub.watch("worktree:a".into(), || async { panic!("duplicate loader") });
        assert_eq!(
            settled(&mut a).await.snapshot,
            settled(&mut b).await.snapshot
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        drop(a);
        drop(b);
        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(600)).await;
        tokio::task::yield_now().await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn failure_keeps_success_and_invalidation_retries() {
        let hub = RefreshHub::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let mut rx = hub.watch("pr:a".into(), move || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            async move {
                if call == 1 {
                    Err("offline".into())
                } else {
                    Ok((serde_json::json!(call), Duration::from_secs(60)))
                }
            }
        });
        let first = settled(&mut rx).await;
        hub.invalidate("pr:a");
        rx.changed().await.unwrap();
        let failed = settled(&mut rx).await;
        assert_eq!(failed.snapshot, first.snapshot);
        assert_eq!(failed.updated_at, first.updated_at);
        assert_eq!(failed.error.as_deref(), Some("offline"));
        hub.invalidate("pr:a");
        rx.changed().await.unwrap();
        assert_eq!(settled(&mut rx).await.snapshot, Some(serde_json::json!(2)));
    }

    #[tokio::test(start_paused = true)]
    async fn mutation_discards_an_older_in_flight_read() {
        let hub = RefreshHub::default();
        let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(Notify::new());
        let permit = release.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let mut rx = hub.watch("pr:a".into(), move || {
            let call = counter.fetch_add(1, Ordering::SeqCst);
            let started = started.clone();
            let permit = permit.clone();
            async move {
                started.send(call).unwrap();
                if call == 0 {
                    permit.notified().await;
                }
                Ok((serde_json::json!(call), Duration::from_secs(60)))
            }
        });
        assert_eq!(starts.recv().await, Some(0));
        hub.invalidate("pr:a");
        release.notify_one();
        assert_eq!(settled(&mut rx).await.snapshot, Some(serde_json::json!(1)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn continuous_changes_cannot_starve_capture() {
        let hub = RefreshHub::default();
        let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
        let progress = Arc::new(AtomicUsize::new(0));
        let counter = progress.clone();
        let mut rx = hub.watch("worktree:a".into(), move || {
            let started = started.clone();
            let counter = counter.clone();
            async move {
                started.send(counter.load(Ordering::SeqCst)).unwrap();
                Ok((serde_json::json!(true), Duration::from_secs(120)))
            }
        });
        settled(&mut rx).await;
        starts.recv().await.unwrap();
        let refresh = hub.clone();
        let updates = tokio::spawn(async move {
            for index in 1..=30 {
                progress.store(index, Ordering::SeqCst);
                refresh.invalidate("worktree:a");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        let captured_during_burst = starts.recv().await.unwrap();
        assert!(captured_during_burst > 0 && captured_during_burst < 30);
        updates.abort();
    }
}

#[derive(Clone, Default)]
pub(crate) struct RefreshHub {
    entries: Arc<Mutex<HashMap<String, Arc<Entry>>>>,
}

impl RefreshHub {
    pub fn watch<F, Fut>(&self, key: String, load: F) -> watch::Receiver<SourceControlFrame>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<(Value, Duration), String>> + Send + 'static,
    {
        let mut entries = self.entries.lock().unwrap();
        if entries.len() >= CAPACITY {
            entries.retain(|_, entry| entry.running || entry.tx.receiver_count() > 0);
            if entries.len() >= CAPACITY && !entries.contains_key(&key) {
                return watch::channel(SourceControlFrame {
                    error: Some(
                        "Too many active source control views. Close unused views and retry."
                            .into(),
                    ),
                    ..Default::default()
                })
                .1;
            }
        }
        let entry = entries.entry(key.clone()).or_insert_with(|| {
            Arc::new(Entry {
                tx: watch::channel(SourceControlFrame::default()).0,
                kick: Notify::new(),
                epoch: AtomicU64::new(0),
                running: false,
            })
        });
        let rx = entry.tx.subscribe();
        if !entry.running {
            // Only this registry lock changes the lifecycle; a late unsubscribe
            // cannot stop the replacement subscriber's worker.
            // Existing inactive entries can still have a worker Arc in teardown.
            let replacement = Entry {
                tx: entry.tx.clone(),
                kick: Notify::new(),
                epoch: AtomicU64::new(entry.epoch.load(Ordering::Acquire)),
                running: true,
            };
            *entry = Arc::new(replacement);
            let worker = entry.clone();
            let hub = self.clone();
            let loader: Loader = Arc::new(move || load().boxed());
            tokio::spawn(async move {
                hub.run(key, worker, loader).await;
            });
        }
        rx
    }

    pub fn invalidate(&self, prefix: &str) {
        for (key, entry) in self.entries.lock().unwrap().iter() {
            if key.starts_with(prefix) {
                entry.epoch.fetch_add(1, Ordering::AcqRel);
                entry.kick.notify_one();
            }
        }
    }

    async fn run(&self, key: String, entry: Arc<Entry>, load: Loader) {
        let mut failures = 0u32;
        let mut captured_epoch = None;
        loop {
            if entry.tx.receiver_count() == 0 {
                tokio::time::sleep(LINGER).await;
                let mut entries = self.entries.lock().unwrap();
                if entry.tx.receiver_count() == 0 {
                    // Keep the last successful snapshot for a quick reopen.
                    if entries
                        .get(&key)
                        .is_some_and(|current| Arc::ptr_eq(current, &entry))
                    {
                        let cached = Entry {
                            tx: entry.tx.clone(),
                            kick: Notify::new(),
                            epoch: AtomicU64::new(entry.epoch.load(Ordering::Acquire)),
                            running: false,
                        };
                        entries.insert(key, Arc::new(cached));
                    }
                    return;
                }
            }
            if captured_epoch.is_some_and(|epoch| epoch != entry.epoch.load(Ordering::Acquire)) {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                loop {
                    if tokio::time::Instant::now() >= deadline {
                        break;
                    }
                    let delay = Duration::from_millis(150)
                        .min(deadline.saturating_duration_since(tokio::time::Instant::now()));
                    if tokio::time::timeout(delay, entry.kick.notified())
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            let epoch = entry.epoch.load(Ordering::Acquire);
            captured_epoch = Some(epoch);
            // Consume the wake that caused this read; later invalidations
            // increment the epoch and require one new capture.
            let _ = entry.kick.notified().now_or_never();
            entry.tx.send_modify(|frame| frame.refreshing = true);
            // A client disappearing never kills an already running read.
            let started = tokio::time::Instant::now();
            let deadline = if key.starts_with("remote:") {
                Duration::from_secs(300)
            } else {
                Duration::from_secs(45)
            };
            let result = tokio::time::timeout(
                deadline,
                std::panic::AssertUnwindSafe(async { load().await }).catch_unwind(),
            )
            .await
            .map_err(|_| {
                "Source control refresh timed out. The last successful status is retained."
                    .to_owned()
            })
            .and_then(|result| {
                result
                    .unwrap_or_else(|_| Err("Source control refresh stopped unexpectedly.".into()))
            });
            tracing::debug!(
                resource = key.split(':').next().unwrap_or("source-control"),
                elapsed_ms = started.elapsed().as_millis() as u64,
                subscribers = entry.tx.receiver_count(),
                success = result.is_ok(),
                "source control refresh completed"
            );
            if epoch != entry.epoch.load(Ordering::Acquire) {
                continue;
            }
            let delay = match result {
                Ok((snapshot, delay)) => {
                    failures = 0;
                    entry.tx.send_modify(|frame| {
                        frame.reset = false;
                        frame.snapshot = Some(snapshot);
                        frame.updated_at = Some(crate::now_ms());
                        frame.error = None;
                        frame.refreshing = false;
                    });
                    delay
                }
                Err(error) => {
                    let account_changed = error.starts_with("Account changed.");
                    let authentication =
                        error.contains("not authenticated") || error.contains("not installed");
                    failures = failures.saturating_add(1);
                    entry.tx.send_modify(|frame| {
                        frame.reset = account_changed;
                        if account_changed {
                            frame.snapshot = None;
                            frame.updated_at = None;
                        }
                        frame.error = Some(error);
                        frame.refreshing = false;
                    });
                    if account_changed {
                        self.entries.lock().unwrap().remove(&key);
                        return;
                    }
                    if authentication {
                        failures = 8;
                    }
                    Duration::from_secs(5u64.saturating_mul(1 << failures.min(8)))
                        .min(Duration::from_secs(900))
                }
            };
            // Stable per-resource staggering avoids synchronized refresh bursts
            // after a window opens many repositories at once.
            use std::hash::{Hash, Hasher};
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            key.hash(&mut hash);
            let jitter =
                Duration::from_millis(hash.finish() % ((delay.as_millis() as u64 / 10).max(1)));
            tokio::select! {
                _ = tokio::time::sleep(delay + jitter) => {},
                _ = entry.kick.notified() => {},
                _ = entry.tx.closed() => {},
            }
        }
    }
}
