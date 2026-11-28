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
    let mut outage = Outage::default();
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
                Ok(()) => {
                    outage.recovered(std::time::Instant::now());
                    backoff = backoff.min(RECONNECT_MAX_MS);
                }
                Err(error) if retryable(&error) => {
                    let cap = outage.failed(&error, std::time::Instant::now());
                    wait_ms(&cancel, jittered(backoff.min(cap))).await;
                    backoff = (backoff * 2).min(cap);
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
                    let cap = outage.failed(&error, std::time::Instant::now());
                    wait_ms(&cancel, jittered(backoff.min(cap))).await;
                    backoff = (backoff * 2).min(cap);
                    continue;
                }
                Err(error) => return Err(error),
                _ => {
                    outage.recovered(std::time::Instant::now());
                    backoff = backoff.min(RECONNECT_MAX_MS);
                }
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
                        // A socket that still carried the credential this
                        // runtime has since renewed reconnects with the new
                        // one; only a fence of the current credential ends it.
                        Ok(Err(zuse_rpc::RpcError::Capability(code))) if code=="cloud.runtime_fenced" && !renewed_since(identity, &bootstrap)=>bail!("runtime_authority_fenced"),
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
                            Ok(())=>{backoff=RECONNECT_START_MS;outage.recovered(std::time::Instant::now());},Err(error) if retryable(&error)=>{outage.failed(&error, std::time::Instant::now());},Err(error)=>return Err(error),
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
/// Whether the identity holds a different credential than the one `used`
/// carried: a refusal of `used` then says nothing about the current one.
fn renewed_since(identity: &Identity, used: &zuse_protocol::cloud::CloudRuntimeBootstrap) -> bool {
    identity
        .bootstrap()
        .is_some_and(|current| current.runtime_credential != used.runtime_credential)
}
pub fn retryable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Failure>().is_some_and(|e| e.retryable)
}
const RECONNECT_START_MS: u64 = 200;
const RECONNECT_MAX_MS: u64 = 5_000;
/// Once the authority has refused (5xx/429) for this long without a single
/// success, back off further so a struggling or misconfigured API is not
/// hammered every 5 s by every computer, forever.
const SUSTAINED_OUTAGE: Duration = Duration::from_secs(5 * 60);
const SUSTAINED_MAX_MS: u64 = 60_000;
/// A refusal that lasts this long is reported once (WARN with its code and
/// count), instead of retrying silently.
const OUTAGE_REPORT_AFTER: Duration = Duration::from_secs(2 * 60);

/// Consecutive retryable authority refusals: bounded backoff and one report
/// per outage, so a permanent refusal answered with 503 is visible in the
/// guest log with its code, and recovery is logged with its duration.
#[derive(Default)]
struct Outage {
    since: Option<std::time::Instant>,
    failures: u32,
    code: &'static str,
    reported: bool,
}
impl Outage {
    /// Record a retryable failure; returns the backoff cap to use now.
    fn failed(&mut self, error: &anyhow::Error, now: std::time::Instant) -> u64 {
        let since = *self.since.get_or_insert(now);
        self.failures = self.failures.saturating_add(1);
        self.code = error
            .downcast_ref::<Failure>()
            .map(|f| f.code)
            .unwrap_or("runtime_authority_unavailable");
        let lasted = now.saturating_duration_since(since);
        if !self.reported && lasted >= OUTAGE_REPORT_AFTER {
            self.reported = true;
            tracing::warn!(
                code = self.code,
                failures = self.failures,
                seconds = lasted.as_secs(),
                "Cloud authority keeps refusing this runtime; still retrying with backoff"
            );
        }
        if lasted >= SUSTAINED_OUTAGE {
            SUSTAINED_MAX_MS
        } else {
            RECONNECT_MAX_MS
        }
    }
    fn recovered(&mut self, now: std::time::Instant) {
        if let Some(since) = self.since.take() {
            if self.reported {
                tracing::info!(
                    code = self.code,
                    failures = self.failures,
                    seconds = now.saturating_duration_since(since).as_secs(),
                    "Cloud authority answered again after a sustained refusal"
                );
            }
        }
        *self = Self::default();
    }
}
/// 75%..125% of `ms`, from the clock's sub-second noise (no RNG needed).
fn jittered(ms: u64) -> u64 {
    let noise = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    ms * 3 / 4 + noise % (ms / 2 + 1)
}
async fn wait_ms(cancel: &CancellationToken, ms: u64) {
    tokio::select! {_=cancel.cancelled()=>{},_=tokio::time::sleep(Duration::from_millis(ms.min(SUSTAINED_MAX_MS * 5 / 4 + 1)))=>{}}
}
pub async fn wait(cancel: &CancellationToken, seconds: u64) {
    tokio::select! {_=cancel.cancelled()=>{},_=tokio::time::sleep(Duration::from_secs(seconds.min(10)))=>{}}
}

#[cfg(test)]
mod reconnect_tests {
    use super::*;
    fn refusal() -> anyhow::Error {
        Failure {
            code: "runtime_authority_unavailable",
            retryable: true,
        }
        .into()
    }
    #[test]
    fn a_sustained_refusal_backs_off_further_reports_once_and_resets_on_success() {
        let start = std::time::Instant::now();
        let mut outage = Outage::default();
        assert_eq!(outage.failed(&refusal(), start), RECONNECT_MAX_MS);
        assert!(!outage.reported);
        assert_eq!(
            outage.failed(&refusal(), start + OUTAGE_REPORT_AFTER),
            RECONNECT_MAX_MS
        );
        assert!(outage.reported, "a long refusal is reported");
        assert_eq!(
            outage.failed(&refusal(), start + SUSTAINED_OUTAGE),
            SUSTAINED_MAX_MS
        );
        assert_eq!(outage.failures, 3);
        outage.recovered(start + SUSTAINED_OUTAGE);
        assert!(outage.since.is_none() && !outage.reported && outage.failures == 0);
        // A new outage starts from the short cap again.
        assert_eq!(
            outage.failed(&refusal(), start + SUSTAINED_OUTAGE * 2),
            RECONNECT_MAX_MS
        );
    }
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
