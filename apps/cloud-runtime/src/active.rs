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
    runtime_updates: Option<&RuntimeUpdates>,
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
            // Ending the cycle stops the gateway and mailbox before a swap.
            let swap = RuntimeUpdates::now_ready(runtime_updates);
            tokio::pin!(swap);
            if let Some(mailbox) = mailbox {
                tokio::select! {result=&mut supervisor=>Cycle::Ended(result),result=&mut reporter=>Cycle::Ended(result),result=mailbox.run(authority.clone(),updates.clone(),cycle.clone())=>Cycle::Ended(result),()=&mut swap=>Cycle::SwapNow}
            } else {
                tokio::select! {result=&mut supervisor=>Cycle::Ended(result),result=&mut reporter=>Cycle::Ended(result),()=&mut swap=>Cycle::SwapNow}
            }
        };
        cycle.cancel();
        if let Some(mut projection) = projection {
            let _ = (&mut projection).await;
        }
        if cancel.is_cancelled() {
            return Ok(());
        }
        let result = match result {
            Cycle::Ended(result) => result,
            Cycle::SwapNow => {
                if let Some(updates) = runtime_updates {
                    if updates.swap_now(core, cancel).await? {
                        // `main` shuts the engine down, then execs the new binary.
                        return Ok(());
                    }
                }
                continue;
            }
        };
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
                    if RuntimeUpdates::staged_for_next_start(runtime_updates) {
                        // Swapping in a frozen process is not supported: take
                        // the non-memory pause so the next start swaps at cold
                        // boot. Costs one wake without memory per update.
                        tracing::info!("Runtime update staged; pausing without memory");
                        return result;
                    }
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
                        RuntimeUpdates::woke(runtime_updates);
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

enum Cycle {
    Ended(Result<()>),
    /// A `now` runtime update is staged and the engine is quiescent.
    SwapNow,
}

/// The runtime-update poller and what the swap boundaries need from it.
pub struct RuntimeUpdates {
    #[cfg(unix)]
    updater: Arc<zuse_cloud_runtime::updates::Updater>,
    #[cfg(unix)]
    activity: Arc<dyn zuse_cloud_runtime::updates::Activity>,
    #[cfg(unix)]
    _poller: AbortOnDropHandle<()>,
}
impl RuntimeUpdates {
    /// Start polling for the server-pinned release. `None` when this process
    /// cannot run the updater (it keeps serving without updates).
    pub async fn start(
        directory: &Path,
        core: &Arc<EngineCore>,
        database: &Path,
        authority: &Authority,
        boot: &watch::Receiver<CloudRuntimeBootstrap>,
        cancel: &CancellationToken,
    ) -> Option<Self> {
        #[cfg(unix)]
        {
            use zuse_cloud_runtime::updates::{Activity, Updater};
            let data = directory.to_path_buf();
            // Hashes the running binary: keep it off the async workers.
            let updater = match tokio::task::spawn_blocking(move || Updater::detect(&data)).await {
                Ok(Ok(updater)) => updater,
                Ok(Err(error)) => {
                    tracing::warn!(%error, "runtime_update_poller_unavailable");
                    return None;
                }
                Err(_) => return None,
            };
            let activity: Arc<dyn Activity> = Arc::new(EngineActivity {
                core: core.clone(),
                database: database.into(),
            });
            let poller = AbortOnDropHandle::new(tokio::spawn(updater.clone().run(
                authority.clone(),
                boot.clone(),
                activity.clone(),
                cancel.child_token(),
            )));
            Some(Self {
                updater,
                activity,
                _poller: poller,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (directory, core, database, authority, boot, cancel);
            None
        }
    }
    async fn now_ready(this: Option<&Self>) {
        #[cfg(unix)]
        if let Some(this) = this {
            this.updater.now_ready(this.activity.as_ref()).await;
            return;
        }
        let _ = this;
        std::future::pending::<()>().await
    }
    fn staged_for_next_start(this: Option<&Self>) -> bool {
        #[cfg(unix)]
        return this.is_some_and(|this| this.updater.staged_for_next_start());
        #[cfg(not(unix))]
        {
            let _ = this;
            false
        }
    }
    fn woke(this: Option<&Self>) {
        #[cfg(unix)]
        if let Some(this) = this {
            this.updater.woke();
        }
        let _ = this;
    }
    /// The gateway and mailbox are stopped. Park execution and install;
    /// `true` means the new binary is installed and this process must exec it.
    /// Otherwise execution reopens and serving resumes.
    async fn swap_now(&self, core: &EngineCore, cancel: &CancellationToken) -> Result<bool> {
        #[cfg(unix)]
        {
            // A command leased just before the mailbox stopped still blocks.
            if !self.activity.quiescent() {
                return Ok(false);
            }
            let parked = core.park_execution().await?;
            if !self.activity.mailbox_quiescent() {
                core.resume_execution(parked)?;
                return Ok(false);
            }
            match self.updater.install(cancel).await {
                Some(binary) => {
                    zuse_cloud_runtime::updates::request_exec(binary);
                    Ok(true)
                }
                None => {
                    core.resume_execution(parked)?;
                    Ok(false)
                }
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (core, cancel);
            Ok(false)
        }
    }
}

/// Swap gate backed by the engine's run statuses and the durable mailbox.
#[cfg(unix)]
struct EngineActivity {
    core: Arc<EngineCore>,
    database: std::path::PathBuf,
}
#[cfg(unix)]
impl zuse_cloud_runtime::updates::Activity for EngineActivity {
    fn turn_active(&self) -> bool {
        self.core.sessions.any_active()
    }
    fn mailbox_quiescent(&self) -> bool {
        zuse_cloud_runtime::updates::agent::mailbox_quiescent(&self.database)
    }
}
