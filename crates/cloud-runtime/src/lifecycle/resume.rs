//! A retained graph remains parked until a fresh operation query authorizes it.
use super::*;
use crate::{Identity, parking::Wake};
use tokio_util::sync::CancellationToken;

async fn renew(authority: &Authority, identity: &mut Identity, cancel: &CancellationToken) -> bool {
    for backoff in [1, 2, 4, 8, 8] {
        if cancel.is_cancelled() {
            return false;
        }
        match authority.renew(identity).await {
            Ok(()) => return true,
            Err(e) if crate::supervisor::retryable(&e) => {
                crate::supervisor::wait(cancel, backoff).await
            }
            Err(_) => return false,
        }
    }
    false
}

/// Retry only transient transport/authority failures for this explicit wake.
/// Exhaustion returns to the listener; the durable lifecycle delivery may
/// re-notify the same operation while its owned parked socket still exists.
pub(super) async fn retry_transient(
    error: &anyhow::Error,
    retries: &mut usize,
    expires_at: i64,
    cancel: &CancellationToken,
) -> bool {
    if !crate::supervisor::retryable(error)
        || *retries >= 2
        || cancel.is_cancelled()
        || crate::now_ms() >= expires_at
    {
        return false;
    }
    let delay = 1u64 << *retries;
    *retries += 1;
    crate::supervisor::wait(cancel, delay).await;
    !cancel.is_cancelled() && crate::now_ms() < expires_at
}

pub(super) async fn authorize(
    authority: &Authority,
    identity: &mut Identity,
    wake: &Wake,
    cancel: &CancellationToken,
) -> Result<Option<(CloudRuntimeBootstrap, Directive)>> {
    let boot = identity
        .bootstrap()
        .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
    if cancel.is_cancelled()
        || wake.workspace_id != boot.workspace_id
        || wake.generation != boot.runtime_generation
        || !boot
            .runtime_storage
            .as_ref()
            .is_some_and(|s| s.storage_incarnation_id == wake.storage_incarnation_id)
    {
        return Ok(None);
    }
    // Match normal enrollment's expiry margin. An unfinished renewal must finish
    // under its original durable request ID even when the old bearer is valid.
    let mut renewed = identity.has_pending_renewal()
        || boot.runtime_credential_expires_at <= crate::now_ms().saturating_add(60_000);
    if renewed && !renew(authority, identity, cancel).await {
        return Ok(None);
    }
    let mut transient_retries = 0;
    loop {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let current = identity
            .bootstrap()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        match authority.lifecycle(&current, RuntimeRequest::Query).await {
            Ok(Some(d))
                if d.action == Action::Resume
                    && d.operation_id == wake.operation_id
                    && d.generation == wake.generation
                    && d.storage_incarnation_id == wake.storage_incarnation_id
                    && matches!(d.phase.as_str(), "waking" | "launching" | "ready")
                    && current.runtime_credential_expires_at > crate::now_ms()
                    && !cancel.is_cancelled() =>
            {
                return Ok(Some((current, d)));
            }
            // A server-rejected bearer may rotate only through this enrolled
            // recipient's signed proof; the new bearer must pass Query again.
            Err(e)
                if !renewed
                    && e.downcast_ref::<crate::authority::Failure>()
                        .is_some_and(|e| {
                            matches!(
                                e.code,
                                "workspace_runtime_fenced" | "runtime_authority_fenced"
                            )
                        }) =>
            {
                if !renew(authority, identity, cancel).await {
                    return Ok(None);
                }
                renewed = true;
            }
            Err(e) if crate::supervisor::retryable(&e) => {
                if !retry_transient(
                    &e,
                    &mut transient_retries,
                    current.runtime_credential_expires_at,
                    cancel,
                )
                .await
                {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        }
    }
}

#[cfg(test)]
mod tests;
