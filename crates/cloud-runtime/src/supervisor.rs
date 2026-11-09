//! One gateway/renewal supervisor. Engine ownership stays with the executable.
use crate::{Authority, Identity, authority::Failure, now_ms};
use anyhow::{Result, bail};
use std::{sync::Arc, time::Duration};
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};

pub async fn serve(
    authority: &Authority,
    identity: &mut Identity,
    service: Arc<dyn zuse_rpc::RpcService>,
    cancel: CancellationToken,
    credentials: tokio::sync::watch::Sender<zuse_protocol::cloud::CloudRuntimeBootstrap>,
) -> Result<()> {
    // Milliseconds. A dropped gateway reconnects almost at once; repeated
    // failures back off to at most 5s, with jitter so a restarted API is not
    // hit by every runtime in the same instant.
    let mut backoff = RECONNECT_START_MS;
    let mut initial_connection = !identity.acknowledged()
        && identity.bootstrap().is_some_and(|boot| {
            boot.launch_intent.is_some()
                && boot.extra.get("bootstrapAckMode") == Some(&serde_json::json!("gateway-v1"))
        });
    loop {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let bootstrap = identity
            .bootstrap()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        if identity.has_pending_renewal()
            || bootstrap.runtime_credential_expires_at <= now_ms() + 60000
        {
            let renewed = authority.renew(identity).await;
            publish(identity, &credentials);
            match renewed {
                Ok(()) => {}
                Err(error) if retryable(&error) => {
                    wait_ms(&cancel, jittered(backoff)).await;
                    backoff = (backoff * 2).min(RECONNECT_MAX_MS);
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        let bootstrap = identity
            .bootstrap()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        // Archive can win before the first gateway attaches, or between reconnects.
        // A denied socket has no live lease for the authority to fence, so inspect
        // durable lifecycle intent before retrying the connection.
        if !std::mem::replace(&mut initial_connection, false) {
            match authority
                .lifecycle(
                    &bootstrap,
                    zuse_protocol::cloud_lifecycle::RuntimeRequest::Query,
                )
                .await
            {
                Ok(Some(d))
                    if matches!(
                        d.action,
                        zuse_protocol::cloud_lifecycle::Action::Pause
                            | zuse_protocol::cloud_lifecycle::Action::Archive
                    ) && d.phase == "quiescing" =>
                {
                    bail!("runtime_quiescence_requested")
                }
                Err(error) if retryable(&error) => {
                    wait_ms(&cancel, jittered(backoff)).await;
                    backoff = (backoff * 2).min(RECONNECT_MAX_MS);
                    continue;
                }
                Err(error) => return Err(error),
                _ => {}
            }
        }
        let connection_started = std::time::Instant::now();
        let mut grant = authority.gateway(&bootstrap)?;
        if identity.acknowledged() {
            grant.storage_binding = None;
        }
        let link_cancel = cancel.child_token();
        let (connected, mut connection) = tokio::sync::oneshot::channel();
        let mut connection_pending = true;
        let mut socket = AbortOnDropHandle::new(tokio::spawn({
            let service = service.clone();
            let workspace = bootstrap.workspace_id.clone();
            let owner = bootstrap.zuse_account_id.clone();
            let token = link_cancel.clone();
            async move {
                zuse_rpc::cloud::serve_guest_connected(
                    service,
                    grant,
                    &workspace,
                    &owner,
                    token,
                    Some(connected),
                )
                .await
            }
        }));
        let mut renewal = tokio::time::interval(Duration::from_secs(30));
        renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {biased;
                _=cancel.cancelled()=> {link_cancel.cancel();let _=(&mut socket).await;return Ok(());}
                result=&mut connection, if connection_pending => {
                    connection_pending = false;
                    tracing::info!(workspace=%bootstrap.workspace_id, phase="runtime-gateway-connect", duration_ms=connection_started.elapsed().as_millis() as u64, succeeded=result.is_ok(), "Cloud startup phase");
                    if result.is_ok() && bootstrap.extra.get("startupBindingVersion") == Some(&serde_json::json!(2)) {
                        let mut committed = bootstrap.clone();
                        committed.extra.insert("startupBindingCommitted".into(), serde_json::json!(true));
                        credentials.send_replace(committed);
                    }
                    if result.is_ok() && bootstrap.extra.get("bootstrapAckMode") == Some(&serde_json::json!("gateway-v1")) && !identity.acknowledged() {
                        identity.acknowledge()?;
                    }
                }
                result=&mut socket=> {
                    match result {
                        Ok(Err(zuse_rpc::RpcError::Capability(code))) if code=="cloud.runtime_fenced"=>bail!("runtime_authority_fenced"),
                        Err(_)=>bail!("runtime_gateway_worker_failed"),
                        _=>{},
                    }
                    break;
                }
                _=renewal.tick()=> {
                    let boot=identity.bootstrap().ok_or_else(||anyhow::anyhow!("runtime_not_enrolled"))?;
                    if identity.has_pending_renewal() || boot.runtime_credential_expires_at<=now_ms()+120000 {
                        let renewed = authority.renew(identity).await;
                        publish(identity, &credentials);
                        match renewed {
                            Ok(())=>backoff=RECONNECT_START_MS,Err(error) if retryable(&error)=>{},Err(error)=>return Err(error),
                        }
                    }
                    if identity.bootstrap().is_none_or(|b|b.runtime_credential_expires_at<=now_ms()) {link_cancel.cancel();let _=(&mut socket).await;break;}
                }
            }
        }
        if connection_started.elapsed() >= Duration::from_secs(30) {
            backoff = RECONNECT_START_MS;
        }
        wait_ms(&cancel, jittered(backoff)).await;
        backoff = (backoff * 2).min(RECONNECT_MAX_MS);
    }
}
/// Hand consumers whatever credential the identity now holds, whether or not
/// the renewal call reported success: a failure after the identity changed
/// must not leave them presenting a bearer the identity no longer has.
fn publish(
    identity: &Identity,
    credentials: &tokio::sync::watch::Sender<zuse_protocol::cloud::CloudRuntimeBootstrap>,
) {
    if let Some(boot) = identity.bootstrap() {
        credentials.send_replace(boot.clone());
    }
}
pub fn retryable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Failure>().is_some_and(|e| e.retryable)
}
const RECONNECT_START_MS: u64 = 200;
const RECONNECT_MAX_MS: u64 = 5_000;
/// 75%..125% of `ms`, from the clock's sub-second noise (no RNG needed).
fn jittered(ms: u64) -> u64 {
    let noise = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    ms * 3 / 4 + noise % (ms / 2 + 1)
}
async fn wait_ms(cancel: &CancellationToken, ms: u64) {
    tokio::select! {_=cancel.cancelled()=>{},_=tokio::time::sleep(Duration::from_millis(ms.min(10_000)))=>{}}
}
pub async fn wait(cancel: &CancellationToken, seconds: u64) {
    tokio::select! {_=cancel.cancelled()=>{},_=tokio::time::sleep(Duration::from_secs(seconds.min(10)))=>{}}
}

#[cfg(test)]
mod reconnect_tests {
    #[test]
    fn reconnect_jitter_stays_within_a_quarter_of_the_backoff() {
        for ms in [super::RECONNECT_START_MS, 1_000, super::RECONNECT_MAX_MS] {
            for _ in 0..64 {
                let value = super::jittered(ms);
                assert!(
                    value >= ms * 3 / 4 && value <= ms * 5 / 4 + 1,
                    "{ms} -> {value}"
                );
            }
        }
    }
}
