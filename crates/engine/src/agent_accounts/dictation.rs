//! The account dictation uploads with. Dictation transcribes phrase by
//! phrase, so every phrase needs the account; refreshing it launches Codex,
//! which is far too slow to repeat per phrase.
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::time::{Duration, Instant};

/// Access token and ChatGPT account id.
pub(crate) type DictationAuth = (String, Option<String>);

/// Access tokens outlive this by far; the reuse only spares Codex launches.
const REUSE: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub(super) struct DictationAccount {
    /// Held across a refresh, so concurrent requests share one.
    cached: tokio::sync::Mutex<Option<Cached>>,
    /// Bumped when the signed-in account changes; older entries are stale.
    generation: AtomicU64,
}

struct Cached {
    auth: DictationAuth,
    at: Instant,
    generation: u64,
}

impl DictationAccount {
    /// The cached account while it is fresh, else the result of `refresh`.
    /// `rejected` is a token the service refused: it is never returned
    /// again, but a different token another request already fetched is.
    pub(super) async fn get<E, F>(
        &self,
        rejected: Option<&str>,
        refresh: impl FnOnce() -> F,
    ) -> Result<DictationAuth, E>
    where
        F: Future<Output = Result<DictationAuth, E>>,
    {
        let mut cached = self.cached.lock().await;
        let generation = self.generation.load(Ordering::Acquire);
        if let Some(entry) = cached.as_ref()
            && entry.generation == generation
            && entry.at.elapsed() < REUSE
            && rejected != Some(entry.auth.0.as_str())
        {
            return Ok(entry.auth.clone());
        }
        // A failed refresh is not cached: the next request tries again.
        *cached = None;
        let auth = refresh().await?;
        *cached = Some(Cached {
            auth: auth.clone(),
            at: Instant::now(),
            generation,
        });
        Ok(auth)
    }

    /// Sign-in, sign-out and account switches make the cached account stale.
    pub(super) fn forget(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    fn refresher(
        calls: &Arc<AtomicUsize>,
    ) -> impl FnOnce() -> std::pin::Pin<Box<dyn Future<Output = Result<DictationAuth, ()>> + Send>>
    {
        let calls = calls.clone();
        move || {
            Box::pin(async move {
                let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok((format!("token-{n}"), Some("account".into())))
            })
        }
    }

    #[tokio::test(start_paused = true)]
    async fn reuses_the_account_for_five_minutes() {
        let account = DictationAccount::default();
        let calls = Arc::new(AtomicUsize::new(0));
        assert_eq!(
            account.get(None, refresher(&calls)).await.unwrap().0,
            "token-1"
        );
        tokio::time::advance(REUSE - Duration::from_secs(1)).await;
        assert_eq!(
            account.get(None, refresher(&calls)).await.unwrap().0,
            "token-1"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            account.get(None, refresher(&calls)).await.unwrap().0,
            "token-2"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn concurrent_requests_share_one_refresh() {
        let account = DictationAccount::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let (a, b, c) = tokio::join!(
            account.get(None, refresher(&calls)),
            account.get(None, refresher(&calls)),
            account.get(None, refresher(&calls)),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        for auth in [a, b, c] {
            assert_eq!(auth.unwrap().0, "token-1");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_rejected_token_is_replaced_once() {
        let account = DictationAccount::default();
        let calls = Arc::new(AtomicUsize::new(0));
        account.get(None, refresher(&calls)).await.unwrap();
        // Two phrases saw the same token rejected: only one refetches.
        let (a, b) = tokio::join!(
            account.get(Some("token-1"), refresher(&calls)),
            account.get(Some("token-1"), refresher(&calls)),
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(a.unwrap().0, "token-2");
        assert_eq!(b.unwrap().0, "token-2");
    }

    #[tokio::test(start_paused = true)]
    async fn failures_and_account_changes_are_not_reused() {
        let account = DictationAccount::default();
        let calls = Arc::new(AtomicUsize::new(0));
        assert!(
            account
                .get(None, || async { Err::<DictationAuth, _>(()) })
                .await
                .is_err()
        );
        assert_eq!(
            account.get(None, refresher(&calls)).await.unwrap().0,
            "token-1"
        );
        account.forget();
        assert_eq!(
            account.get(None, refresher(&calls)).await.unwrap().0,
            "token-2"
        );
    }
}
