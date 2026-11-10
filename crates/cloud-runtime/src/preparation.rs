//! Overlap independent access preparation without abandoning a checkout writer.
use std::{future::Future, sync::Arc};
use zuse_protocol::cloud_auth::CloudAuthProvider;

/// Only APIs that advertise support receive the new stage. Older authorities
/// still use the same admission barrier, without rejecting an unknown report.
pub fn credential_reports_supported(ingress: &serde_json::Value) -> bool {
    ingress.get("credentialPreparationVersion") == Some(&serde_json::json!(1))
}

/// Runtime publication and access APIs require the gateway's durable storage bind.
/// This is a local watch notification, not another authority round trip.
pub async fn wait_startup_binding(
    bootstrap: &zuse_protocol::cloud::CloudRuntimeBootstrap,
    mut updates: tokio::sync::watch::Receiver<zuse_protocol::cloud::CloudRuntimeBootstrap>,
    cancel: &tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    if bootstrap.extra.get("startupBindingVersion") != Some(&serde_json::json!(2)) {
        return Ok(());
    }
    loop {
        {
            let current = updates.borrow();
            anyhow::ensure!(
                current.workspace_id == bootstrap.workspace_id
                    && current.runtime_generation == bootstrap.runtime_generation
                    && current.gateway_epoch == bootstrap.gateway_epoch,
                "runtime_authority_fenced"
            );
            if current.extra.get("startupBindingCommitted") == Some(&serde_json::json!(true)) {
                return Ok(());
            }
        }
        tokio::select! {
            _ = cancel.cancelled() => anyhow::bail!("runtime_preparation_cancelled"),
            result = updates.changed() => { result.map_err(|_| anyhow::anyhow!("runtime_gateway_unavailable"))?; }
        }
    }
}

/// Reuse the selected broker's in-memory grant and recipient identity when
/// installing the harness; other agents remain lazy and obtain no grants here.
pub fn reuse_selected<T>(
    selected: Option<&(CloudAuthProvider, Arc<T>)>,
    provider: CloudAuthProvider,
    create: impl FnOnce() -> anyhow::Result<Arc<T>>,
) -> anyhow::Result<Arc<T>> {
    match selected.filter(|(selected, _)| *selected == provider) {
        Some((_, broker)) => Ok(broker.clone()),
        None => create(),
    }
}

/// Access checks do not depend on a progress report reaching the server first.
/// The outer result fences a rejected report; the inner result lets callers
/// publish the access failure before returning it. Only cancellable access I/O
/// belongs here, never a blocking checkout or another filesystem writer.
pub async fn access_with_progress<T>(
    progress: impl Future<Output = anyhow::Result<()>>,
    access: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<anyhow::Result<T>> {
    let (_, access) = tokio::try_join!(progress, async { Ok::<_, anyhow::Error>(access.await) })?;
    Ok(access)
}

/// Do not use try_join: dropping spawn_blocking's handle does not stop Git's
/// disk writes. On either failure/cancellation, drain both futures before the
/// caller can park or release storage. A checkout failure keeps precedence.
pub async fn join_repository_and_credentials<T>(
    repository: impl Future<Output = anyhow::Result<T>>,
    credentials: impl Future<Output = anyhow::Result<()>>,
) -> anyhow::Result<T> {
    let (repository, credentials) = tokio::join!(repository, credentials);
    let repository = repository?;
    credentials?;
    Ok(repository)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        sync::oneshot,
        time::{Duration, timeout},
    };

    #[tokio::test]
    async fn access_starts_during_a_paused_progress_report_but_waits_for_its_fence() {
        let (started, observed) = oneshot::channel();
        let (release, paused) = oneshot::channel();
        let task = tokio::spawn(access_with_progress(
            async {
                paused.await.unwrap();
                Ok(())
            },
            async {
                started.send(()).unwrap();
                Err::<(), _>(anyhow::anyhow!("access_failed"))
            },
        ));
        timeout(Duration::from_secs(1), observed)
            .await
            .unwrap()
            .unwrap();
        assert!(!task.is_finished());
        release.send(()).unwrap();
        assert_eq!(
            task.await.unwrap().unwrap().unwrap_err().to_string(),
            "access_failed"
        );
        let rejected = access_with_progress(
            async { anyhow::bail!("progress_fenced") },
            std::future::pending::<anyhow::Result<()>>(),
        )
        .await;
        assert_eq!(rejected.unwrap_err().to_string(), "progress_fenced");
    }

    #[test]
    fn legacy_authority_and_selected_broker_reuse() {
        assert!(!credential_reports_supported(
            &serde_json::json!({"agentReady":false})
        ));
        assert!(!credential_reports_supported(
            &serde_json::json!({"credentialPreparationVersion":2})
        ));
        assert!(credential_reports_supported(
            &serde_json::json!({"credentialPreparationVersion":1})
        ));
        let broker = Arc::new(42);
        let selected = (CloudAuthProvider::Codex, broker.clone());
        let reused = reuse_selected(Some(&selected), CloudAuthProvider::Codex, || {
            panic!("selected broker must not be reconstructed")
        })
        .unwrap();
        assert!(Arc::ptr_eq(&broker, &reused));
        let lazy = reuse_selected(Some(&selected), CloudAuthProvider::Claude, || {
            Ok(Arc::new(7))
        })
        .unwrap();
        assert_eq!(*lazy, 7);
    }

    #[tokio::test]
    async fn both_stages_start_and_admission_waits_for_checkout_after_credentials_complete() {
        let (git_started, observed_git) = oneshot::channel();
        let (credentials_started, observed_credentials) = oneshot::channel();
        let (release_git, wait_git) = oneshot::channel();
        let task = tokio::spawn(join_repository_and_credentials(
            async move {
                git_started.send(()).unwrap();
                wait_git.await.unwrap();
                Ok(42)
            },
            async move {
                credentials_started.send(()).unwrap();
                Ok(())
            },
        ));
        timeout(Duration::from_secs(1), async {
            observed_git.await.unwrap();
            observed_credentials.await.unwrap();
        })
        .await
        .unwrap();
        assert!(
            !task.is_finished(),
            "no environment/Run before checkout barrier"
        );
        release_git.send(()).unwrap();
        assert_eq!(task.await.unwrap().unwrap(), 42);
    }

    #[tokio::test]
    async fn credential_failure_drains_durable_checkout_writer_and_preserves_git_error() {
        let journal = tempfile::tempdir().unwrap();
        let path = journal.path().join("staging-journal");
        let written = path.clone();
        let (release_git, wait_git) = oneshot::channel();
        let (credential_failed, observed_failure) = oneshot::channel();
        let task = tokio::spawn(join_repository_and_credentials(
            async move {
                // The blocking checkout remains owned even after access failed.
                tokio::task::spawn_blocking(move || {
                    wait_git.blocking_recv().unwrap();
                    std::fs::write(written, b"recoverable").unwrap();
                })
                .await
                .unwrap();
                Err::<(), _>(anyhow::anyhow!("repository_preparation_failed"))
            },
            async move {
                credential_failed.send(()).unwrap();
                anyhow::bail!("credential_preparation_failed")
            },
        ));
        observed_failure.await.unwrap();
        assert!(!task.is_finished());
        release_git.send(()).unwrap();
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), "repository_preparation_failed");
        assert_eq!(std::fs::read(path).unwrap(), b"recoverable");
    }

    #[tokio::test]
    async fn cancellation_drains_checkout_before_storage_can_be_parked() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let stopped = cancel.clone();
        let (release_git, wait_git) = oneshot::channel();
        let task = tokio::spawn(join_repository_and_credentials(
            async move {
                wait_git.await.unwrap();
                Ok(())
            },
            async move {
                stopped.cancelled().await;
                anyhow::bail!("runtime_preparation_cancelled")
            },
        ));
        cancel.cancel();
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        release_git.send(()).unwrap();
        assert_eq!(
            task.await.unwrap().unwrap_err().to_string(),
            "runtime_preparation_cancelled"
        );
    }
}
