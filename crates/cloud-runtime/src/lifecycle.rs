//! Shutdown receipts follow engine retirement and checked durable snapshots.
use crate::Authority;
use anyhow::{Result, ensure};
use std::sync::Arc;
use zuse_engine::{EngineCore, doc_host::ChatDocHandle};
use zuse_protocol::{
    cloud::CloudRuntimeBootstrap,
    cloud_lifecycle::{Action, Directive, RuntimeRequest},
};

pub(crate) fn family(core: &EngineCore, boot: &CloudRuntimeBootstrap) -> Result<Vec<String>> {
    let chats = core.workspace.read_chats()?;
    ensure!(
        chats
            .iter()
            .any(|c| c.id == boot.chat_id && c.device_id == core.device_id),
        "runtime_retained_history_missing"
    );
    let mut ids = std::collections::HashSet::from([boot.chat_id.clone()]);
    loop {
        let before = ids.len();
        for c in &chats {
            if c.device_id == core.device_id
                && (c.parent_chat_id.as_ref().is_some_and(|p| ids.contains(p))
                    || c.thread_id.as_ref().is_some_and(|p| ids.contains(p)))
            {
                ids.insert(c.id.clone());
            }
        }
        if ids.len() == before {
            break;
        }
    }
    Ok(ids.into_iter().collect())
}
pub struct Shutdown {
    directive: Directive,
    handles: Vec<Arc<ChatDocHandle>>,
}
/// Must run before engine shutdown; held handles survive retirement for the final checked flush.
pub fn hold(
    core: &EngineCore,
    boot: &CloudRuntimeBootstrap,
    directive: Directive,
) -> Result<Shutdown> {
    ensure!(
        matches!(directive.action, Action::Pause | Action::Archive)
            && directive.phase == "quiescing",
        "runtime_lifecycle_directive_invalid"
    );
    core.doc_host.pause_all_queues();
    let mut handles = Vec::new();
    for id in family(core, boot)? {
        core.doc_host.pause_chat_queue(&id)?;
        handles.push(core.doc_host.open_local(&id)?);
        if directive.action == Action::Archive {
            core.workspace.set_chat_archived(&id, true)?;
        }
    }
    Ok(Shutdown { directive, handles })
}
impl Shutdown {
    /// Call only after EngineCore::shutdown has awaited agents, PTYs and doc workers.
    pub async fn acknowledge(
        self,
        core: &EngineCore,
        authority: &Authority,
        boot: &CloudRuntimeBootstrap,
        publisher: Option<&crate::projection::Publisher>,
    ) -> Result<()> {
        for handle in &self.handles {
            core.doc_host.flush_chat_checked(handle)?;
        }
        core.workspace.flush_checked()?;
        if let Some(publisher) = publisher {
            loop {
                match publisher.finish(core, authority, boot, &self.handles).await {
                    Ok(()) => break,
                    Err(error)
                        if crate::supervisor::retryable(&error)
                            && crate::now_ms() < boot.runtime_credential_expires_at =>
                    {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        loop {
            match authority
                .lifecycle(
                    boot,
                    RuntimeRequest::Quiesced {
                        operation_id: self.directive.operation_id.clone(),
                    },
                )
                .await
            {
                Ok(_) => break,
                Err(error)
                    if crate::supervisor::retryable(&error)
                        && crate::now_ms() < boot.runtime_credential_expires_at =>
                {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}
/// Unarchive the original family on the retained disk before any interactive admission.
pub fn activate(core: &EngineCore, boot: &CloudRuntimeBootstrap, d: &Directive) -> Result<()> {
    ensure!(
        d.action == Action::Resume && matches!(d.phase.as_str(), "waking" | "launching" | "ready"),
        "runtime_lifecycle_directive_invalid"
    );
    for id in family(core, boot)? {
        core.workspace.set_chat_archived(&id, false)?;
    }
    core.workspace.flush_checked()?;
    Ok(())
}

/// Park in place and await explicit worker wake delivery. The notification alone
/// cannot reopen execution: verify the current server-owned operation first.
#[cfg(unix)]
pub async fn memory_sleep(
    core: &EngineCore,
    authority: &Authority,
    identity: &mut crate::Identity,
    directory: &std::path::Path,
    directive: Directive,
    publisher: Option<&crate::projection::Publisher>,
    credentials: &tokio::sync::watch::Sender<CloudRuntimeBootstrap>,
    broker: Option<&crate::grants::ProviderBrokers>,
    cancel: &tokio_util::sync::CancellationToken,
    progress: Option<&crate::task_progress::Publisher>,
) -> Result<bool> {
    let boot = identity
        .bootstrap()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
    ensure!(
        boot.extra.get("rustMemorySleepVersion") == Some(&serde_json::json!(1))
            && directive.action == Action::Pause
            && !directive.full_shutdown
            && directive.phase == "quiescing"
            && directive.generation == boot.runtime_generation
            && boot
                .runtime_storage
                .as_ref()
                .is_some_and(|s| s.storage_incarnation_id == directive.storage_incarnation_id),
        "runtime_memory_sleep_not_admitted"
    );
    let parked = core.park_execution().await?;
    let listener = crate::parking::WakeListener::bind(
        directory,
        &boot.workspace_id,
        boot.runtime_generation,
        &directive.storage_incarnation_id,
    )?;
    let handles = family(core, &boot)?
        .iter()
        .map(|id| core.doc_host.open_local(id))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if let Some(publisher) = publisher {
        loop {
            match publisher.finish(core, authority, &boot, &handles).await {
                Ok(()) => break,
                Err(e)
                    if crate::supervisor::retryable(&e)
                        && crate::now_ms() < boot.runtime_credential_expires_at =>
                {
                    crate::supervisor::wait(cancel, 1).await
                }
                Err(e) => return Err(e),
            }
            if cancel.is_cancelled() {
                return Ok(false);
            }
        }
    }
    if let Some(progress) = progress {
        // park_execution has joined model writers; flush their final facts before
        // Parked can allow the provider to freeze this process.
        tokio::time::timeout(
            std::time::Duration::from_secs(8),
            progress.flush(authority, &boot),
        )
        .await
        .map_err(|_| anyhow::anyhow!("task_progress_flush_timeout"))??;
    }
    // Preserving native Codex sockets across sleep is an optimisation. When it
    // cannot be prepared, park the ordinary way: Codex reconnects on wake.
    let mut native_transport = match crate::memory_transport::PreparedTransport::prepare(
        &boot,
        broker.and_then(|brokers| brokers.codex()),
    )
    .await
    {
        Ok(transport) => transport,
        Err(_) => {
            tracing::warn!("runtime_native_transport_not_preserved; parking without it");
            None
        }
    };
    let acknowledge = async {
        loop {
            match authority
                .lifecycle(
                    &boot,
                    RuntimeRequest::Parked {
                        operation_id: directive.operation_id.clone(),
                    },
                )
                .await
            {
                Ok(_) => return Ok::<_, anyhow::Error>(()),
                Err(e)
                    if crate::supervisor::retryable(&e)
                        && crate::now_ms() < boot.runtime_credential_expires_at =>
                {
                    crate::supervisor::wait(cancel, 1).await
                }
                Err(e) => return Err(e),
            }
            if cancel.is_cancelled() {
                return Ok(());
            }
        }
    };
    // The provider may freeze immediately after the server commits Parked,
    // before its HTTP response reaches this process. On wake, a valid Resume
    // supersedes that old acknowledgement; never tear the retained core down
    // merely because its response was lost or its bearer expired during sleep.
    let mut first_wake = tokio::select! {biased;
        wake=listener.wait(cancel)=>wake?,
        result=acknowledge=>{
            if result.is_err(){tracing::warn!("runtime_memory_park_ack_unconfirmed");}
            None
        }
    };
    'wake: loop {
        let wake = match first_wake.take() {
            Some(wake) => Some(wake),
            None => listener.wait(cancel).await?,
        };
        let Some(wake) = wake else {
            return Ok(false);
        };
        let mut timing = wake_trace::Trace::start(
            &directory.join(".runtime-memory"),
            &wake.workspace_id,
            &wake.operation_id,
            wake.generation,
            &wake.storage_incarnation_id,
        );
        let mut grant_retries = 0;
        let (current, resumed) = loop {
            wake_trace::record(&mut timing, "authorize-start");
            let Some((current, resumed)) =
                resume::authorize(authority, identity, &wake, cancel).await?
            else {
                wake_trace::record(&mut timing, "authorization-not-admitted");
                continue 'wake;
            };
            wake_trace::record(&mut timing, "authorized");
            credentials.send_replace(current.clone());
            if let Some(broker) = broker {
                wake_trace::record(&mut timing, "grant-start");
                if let Err(error) = broker.resume_retained(core, &current).await {
                    if resume::retry_transient(
                        &error,
                        &mut grant_retries,
                        current.runtime_credential_expires_at,
                        cancel,
                    )
                    .await
                    {
                        // Revalidate the exact operation and current authority
                        // before rechecking access; no prompt/tool is replayed.
                        continue;
                    }
                    wake_trace::record(&mut timing, "connection-not-admitted");
                    tracing::warn!("runtime_memory_resume_connection_unavailable");
                    continue 'wake;
                }
                wake_trace::record(&mut timing, "granted");
            } else {
                wake_trace::record(&mut timing, "grant-not-configured");
            }
            break (current, resumed);
        };
        if let Some(transport) = native_transport.take() {
            wake_trace::record(&mut timing, "transport-reset-start");
            if transport.after_wake(&current).await.is_err() {
                // A long sleep can outlive the preserved native sessions (the
                // process exits or its sockets go stale). Never strand the chat
                // waiting for a manual repair: resume normally, and the engine
                // starts or reconnects the agent for the next turn.
                wake_trace::record(&mut timing, "transport-not-preserved");
                tracing::warn!("runtime_native_transport_lost_during_sleep; resuming without it");
            } else {
                wake_trace::record(&mut timing, "transport-reset-complete");
            }
        } else {
            wake_trace::record(&mut timing, "transport-not-required");
        }
        activate(core, &current, &resumed)?;
        wake_trace::record(&mut timing, "resumed-ack-start");
        // A lost acknowledgement can be retried under the same operation. No
        // prompt is replayed and the engine remains fenced until acceptance.
        loop {
            match authority
                .lifecycle(
                    &current,
                    RuntimeRequest::Resumed {
                        operation_id: resumed.operation_id.clone(),
                        wake: Some(zuse_protocol::cloud_lifecycle::Wake { from_memory: true }),
                    },
                )
                .await
            {
                Ok(_) => break,
                Err(e)
                    if crate::supervisor::retryable(&e)
                        && crate::now_ms() < current.runtime_credential_expires_at =>
                {
                    crate::supervisor::wait(cancel, 1).await
                }
                Err(e) => return Err(e),
            }
            if cancel.is_cancelled() {
                return Ok(false);
            }
        }
        wake_trace::record(&mut timing, "resumed-acknowledged");
        core.resume_execution(parked)?;
        wake_trace::record(&mut timing, "core-reopened");
        return Ok(true);
    }
}

#[cfg(unix)]
mod resume;

#[cfg(unix)]
mod wake_trace;
