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
    let mut backoff = 1u64;
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
            match authority.renew(identity).await {
                Ok(()) => {
                    if let Some(boot) = identity.bootstrap() {
                        credentials.send_replace(boot.clone());
                    }
                }
                Err(error) if retryable(&error) => {
                    wait(&cancel, backoff).await;
                    backoff = (backoff * 2).min(10);
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
        let bootstrap = identity
            .bootstrap()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        let connection_started = std::time::Instant::now();
        let grant = authority.gateway(&bootstrap);
        let link_cancel = cancel.child_token();
        let mut socket = AbortOnDropHandle::new(tokio::spawn({
            let service = service.clone();
            let workspace = bootstrap.workspace_id.clone();
            let owner = bootstrap.zuse_account_id.clone();
            let token = link_cancel.clone();
            async move { zuse_rpc::cloud::serve_guest(service, grant, &workspace, &owner, token).await }
        }));
        let mut renewal = tokio::time::interval(Duration::from_secs(30));
        renewal.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {biased;
                _=cancel.cancelled()=> {link_cancel.cancel();let _=(&mut socket).await;return Ok(());}
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
                        match authority.renew(identity).await {
                            Ok(())=>{backoff=1;if let Some(boot)=identity.bootstrap(){credentials.send_replace(boot.clone());}},Err(error) if retryable(&error)=>{},Err(error)=>return Err(error),
                        }
                    }
                    if identity.bootstrap().is_none_or(|b|b.runtime_credential_expires_at<=now_ms()) {link_cancel.cancel();let _=(&mut socket).await;break;}
                }
            }
        }
        if connection_started.elapsed() >= Duration::from_secs(30) {
            backoff = 1;
        }
        wait(&cancel, backoff).await;
        backoff = (backoff * 2).min(10);
    }
}
pub fn retryable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Failure>().is_some_and(|e| e.retryable)
}
pub async fn wait(cancel: &CancellationToken, seconds: u64) {
    tokio::select! {_=cancel.cancelled()=>{},_=tokio::time::sleep(Duration::from_secs(seconds.min(10)))=>{}}
}
