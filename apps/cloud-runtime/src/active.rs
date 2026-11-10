//! Reconnect/sleep cycles share one engine graph and its lifetime IPC listener.
use anyhow::Result;
use std::{path::Path, sync::Arc};
use tokio::sync::watch;
use tokio_util::{sync::CancellationToken, task::AbortOnDropHandle};
use zuse_cloud_runtime::{Authority, Identity, supervisor};
use zuse_engine::EngineCore;
use zuse_protocol::cloud::CloudRuntimeBootstrap;
use zuse_rpc::RpcService;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    core: &Arc<EngineCore>,
    authority: &Authority,
    identity: &mut Identity,
    rpc: Arc<dyn RpcService>,
    publisher: Option<&Arc<zuse_cloud_runtime::projection::Publisher>>,
    directory: &Path,
    database: &Path,
    checkout: &Path,
    allow_test: bool,
    cancel: &CancellationToken,
    credentials: &watch::Sender<CloudRuntimeBootstrap>,
    updates: &watch::Receiver<CloudRuntimeBootstrap>,
    broker: Option<&zuse_cloud_runtime::grants::ProviderBrokers>,
    progress: Option<&Arc<zuse_cloud_runtime::task_progress::Publisher>>,
) -> Result<()> {
    loop {
        let boot = identity
            .bootstrap()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        let cycle = cancel.child_token();
        let mailbox = mailbox(
            core, authority, identity, &boot, database, checkout, allow_test, updates, progress,
        )?;
        let projection = publisher.map(|publisher| {
            let publisher = publisher.clone();
            let core = core.clone();
            let authority = authority.clone();
            let updates = updates.clone();
            let cancel = cycle.clone();
            AbortOnDropHandle::new(tokio::spawn(async move {
                publisher.run(core, authority, updates, cancel).await
            }))
        });
        let result = {
            let supervisor = supervisor::serve(
                authority,
                identity,
                rpc.clone(),
                cycle.clone(),
                credentials.clone(),
            );
            tokio::pin!(supervisor);
            let reporter = async {
                match progress {
                    Some(progress) => {
                        progress
                            .run(authority.clone(), updates.clone(), cycle.clone())
                            .await
                    }
                    None => std::future::pending::<Result<()>>().await,
                }
            };
            tokio::pin!(reporter);
            if let Some(mailbox) = mailbox {
                tokio::select! {result=&mut supervisor=>result,result=&mut reporter=>result,result=mailbox.run(authority.clone(),updates.clone(),cycle.clone())=>result}
            } else {
                tokio::select! {result=&mut supervisor=>result,result=&mut reporter=>result}
            }
        };
        cycle.cancel();
        if let Some(mut projection) = projection {
            let _ = (&mut projection).await;
        }
        if cancel.is_cancelled() {
            return Ok(());
        }
        let current = identity
            .bootstrap()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("runtime_not_enrolled"))?;
        #[cfg(unix)]
        if current.extra.get("rustMemorySleepVersion") == Some(&serde_json::json!(1)) {
            if let Some(d) = authority
                .lifecycle(
                    &current,
                    zuse_protocol::cloud_lifecycle::RuntimeRequest::Query,
                )
                .await?
            {
                if d.action == zuse_protocol::cloud_lifecycle::Action::Pause
                    && !d.full_shutdown
                    && d.phase == "quiescing"
                {
                    if zuse_cloud_runtime::lifecycle::memory_sleep(
                        core,
                        authority,
                        identity,
                        directory,
                        d,
                        publisher.map(Arc::as_ref),
                        credentials,
                        broker,
                        cancel,
                        progress.map(Arc::as_ref),
                    )
                    .await?
                    {
                        continue;
                    }
                    return Ok(());
                }
            }
        }
        return result;
    }
}

#[allow(clippy::too_many_arguments)]
pub fn mailbox(
    core: &Arc<EngineCore>,
    authority: &Authority,
    identity: &Identity,
    boot: &CloudRuntimeBootstrap,
    database: &Path,
    checkout: &Path,
    allow_test: bool,
    updates: &watch::Receiver<CloudRuntimeBootstrap>,
    progress: Option<&Arc<zuse_cloud_runtime::task_progress::Publisher>>,
) -> Result<Option<zuse_cloud_runtime::mailbox::Mailbox>> {
    Ok(
        if boot.extra.get("commandMailboxEnabled") == Some(&serde_json::json!(true)) {
            let key = identity.open_transcript_key(&boot.sealed_transcript_key)?;
            Some(zuse_cloud_runtime::mailbox::Mailbox::new(
                database,
                &boot,
                zuse_cloud_runtime::mailbox::CommandKey::parse(&key)?,
                Arc::new(zuse_cloud_runtime::engine::EngineExecutor {
                    core: core.clone(),
                    thread: boot.chat_id.clone(),
                    checkout: checkout.into(),
                    allow_test,
                    progress: progress.cloned(),
                    task_context: if boot.extra.get("commandAdmissionVersion")
                        == Some(&serde_json::json!(2))
                    {
                        Some(zuse_cloud_runtime::engine::TaskContext::new(
                            authority.clone(),
                            updates.clone(),
                            checkout,
                        )?)
                    } else {
                        None
                    },
                }),
            )?)
        } else {
            None
        },
    )
}
