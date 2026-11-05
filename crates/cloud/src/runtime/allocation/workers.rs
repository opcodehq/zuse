//! One queue reader dispatches bounded independent allocations through durable claims.
use super::*;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// No speculative allocations: only committed, due workspace jobs can enter a lane.
/// Provider effects finish before shutdown; uncertain outcomes retain their receipts.
pub async fn run(
    runtime: Arc<RuntimeControl>,
    registry: Arc<crate::providers::registry::Registry>,
    concurrency: usize,
    poll: std::time::Duration,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        (1..=16).contains(&concurrency),
        "invalid_allocation_concurrency"
    );
    let mut lanes = tokio::task::JoinSet::new();
    let mut signal = super::super::lifecycle::wakeup::Listener::allocation_from_env()
        .unwrap_or_else(|_| super::super::lifecycle::wakeup::Listener::disabled());
    loop {
        if cancel.is_cancelled() {
            break;
        }
        while lanes.len() < concurrency && !cancel.is_cancelled() {
            let now = clock_ms();
            let job = match jobs::claim(&runtime.pool, JOB_KIND, "workspace", now, 120_000).await {
                Ok(Some(job)) => job,
                Ok(None) => break,
                Err(_) => {
                    tracing::warn!("Allocation queue read deferred");
                    break;
                }
            };
            let (runtime, registry) = (runtime.clone(), registry.clone());
            lanes.spawn(async move {
                let provider = runtime.provider_for_job(&registry, &job).await?;
                runtime.allocation_claimed(&provider, &job, now).await
            });
        }
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = signal.receive() => {
                if result.is_err() { signal = super::super::lifecycle::wakeup::Listener::disabled(); }
                let _ = signal.drain_ready();
            },
            result = lanes.join_next(), if !lanes.is_empty() => {
                match result {
                    Some(Ok(Ok(()))) => crate::events::hint::send(),
                    Some(Ok(Err(error))) => tracing::warn!(code=error.code, "Allocation stage deferred"),
                    Some(Err(_)) => {
                        // Join the other effects before reporting dispatcher failure.
                        while lanes.join_next().await.is_some() {}
                        anyhow::bail!("allocation_lane_failed");
                    },
                    None => {},
                }
            },
            _ = tokio::time::sleep(poll) => {},
        }
    }
    let mut failed = false;
    while let Some(result) = lanes.join_next().await {
        match result {
            Err(_) => failed = true,
            Ok(Err(error)) => {
                tracing::warn!(code = error.code, "Allocation shutdown stage deferred")
            }
            Ok(Ok(())) => {}
        }
    }
    anyhow::ensure!(!failed, "allocation_shutdown_requires_reconciliation");
    Ok(())
}
