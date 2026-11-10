//! A real committed hint prioritizes a bounded group of already-durable jobs.
//! Pending receipts keep their existing due_at, and every burst yields to the
//! ordinary worker roles before another one can run.
use crate::error::ApiError;
use std::{
    future::Future,
    time::{Duration, Instant},
};
pub const BURST_LIMIT: usize = 4;
pub struct Burst {
    pub claimed: usize,
    pub limit_reached: bool,
    pub error: Option<ApiError>,
    pub elapsed: Duration,
}
pub async fn ready_burst<F, Fut>(mut attempt: F) -> Burst
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<bool, ApiError>>,
{
    let start = Instant::now();
    let mut claimed = 0;
    for _ in 0..BURST_LIMIT {
        match attempt().await {
            Ok(true) => claimed += 1,
            Ok(false) => {
                return Burst {
                    claimed,
                    limit_reached: false,
                    error: None,
                    elapsed: start.elapsed(),
                };
            }
            Err(error) => {
                return Burst {
                    claimed,
                    limit_reached: false,
                    error: Some(error),
                    elapsed: start.elapsed(),
                };
            }
        }
    }
    Burst {
        claimed,
        limit_reached: true,
        error: None,
        elapsed: start.elapsed(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn lifecycle_burst_yields_at_budget_and_stops_on_empty_or_error() {
        let calls = AtomicUsize::new(0);
        let counter = &calls;
        let full = ready_burst(|| async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        })
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), BURST_LIMIT);
        assert_eq!(full.claimed, BURST_LIMIT);
        assert!(full.limit_reached);
        let empty = ready_burst(|| async move {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(false)
        })
        .await;
        assert_eq!(empty.claimed, 0);
        assert!(!empty.limit_reached);
        assert_eq!(calls.load(Ordering::SeqCst), BURST_LIMIT + 1);
        let failed = ready_burst(|| async { Err(ApiError::conflict("fixture_fenced")) }).await;
        assert_eq!(failed.claimed, 0);
        assert!(!failed.limit_reached);
        assert_eq!(failed.error.unwrap().code, "fixture_fenced");
    }
}
