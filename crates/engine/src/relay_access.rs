//! Profile-owned outbound connector; control handles cannot extend engine life.
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use zuse_serve::{
    BrowserServer,
    relay::{HostStatus, HostToken},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start {
    pub origin: String,
    pub host_token_file: PathBuf,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub origin: Option<String>,
    pub sockets: usize,
    pub sessions: usize,
    pub error: Option<String>,
}
struct Running {
    origin: String,
    metrics: HostStatus,
    stop: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}
#[derive(Default)]
struct State {
    running: Option<Running>,
    error: Option<String>,
}
struct Shared {
    profile: PathBuf,
    device: String,
    pairings: zuse_remote::authority::Authority,
    slots: Arc<tokio::sync::Semaphore>,
    closed: CancellationToken,
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct RelayAccess(Arc<Shared>);
pub struct Owner(RelayAccess);
impl Drop for Owner {
    fn drop(&mut self) {
        self.begin_close();
    }
}
impl Owner {
    pub fn new(
        profile: PathBuf,
        device: String,
        pairings: zuse_remote::authority::Authority,
        slots: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self(RelayAccess(Arc::new(Shared {
            profile,
            device,
            pairings,
            slots,
            closed: CancellationToken::new(),
            state: Mutex::new(State::default()),
        })))
    }
    pub fn control(&self) -> RelayAccess {
        self.0.clone()
    }
    pub fn begin_close(&self) {
        self.0.0.closed.cancel();
    }
    pub async fn close(&self) {
        self.begin_close();
        self.0.stop().await;
    }
}
impl RelayAccess {
    pub async fn status(&self) -> Status {
        if self.0.closed.is_cancelled() {
            return Status::default();
        }
        let mut state = self.0.state.lock().await;
        if state.running.as_ref().is_some_and(|r| r.task.is_finished()) {
            let running = state.running.take().unwrap();
            let _ = running.task.await;
            state.error =
                Some("Relay access stopped. Check its configuration and start it again.".into());
        }
        match &state.running {
            Some(r) => Status {
                running: true,
                origin: Some(r.origin.clone()),
                sockets: r.metrics.sockets(),
                sessions: r.metrics.sessions(),
                error: None,
            },
            None => Status {
                error: state.error.clone(),
                ..Status::default()
            },
        }
    }
    pub async fn start(
        &self,
        options: Start,
        service: Arc<dyn zuse_rpc::RpcService>,
    ) -> anyhow::Result<Status> {
        // Validate before taking ownership; never retain the credential path in status.
        let origin = zuse_serve::relay::validate_origin(&options.origin)?;
        let token = HostToken::read_file(&options.host_token_file)?;
        let mut state = self.0.state.lock().await;
        anyhow::ensure!(!self.0.closed.is_cancelled(), "The engine is shutting down");
        if state.running.as_ref().is_some_and(|r| r.task.is_finished()) {
            let old = state.running.take().unwrap();
            let _ = old.task.await;
        }
        anyhow::ensure!(
            state.running.is_none(),
            "Stop relay access before changing its configuration"
        );
        let stop = self.0.closed.child_token();
        let browser = BrowserServer::new(
            service,
            BrowserServer::load_or_create_token(&self.0.profile)?,
            None,
        )?
        .with_pairings(self.0.pairings.clone(), self.0.device.clone())
        .with_pairing_slots(self.0.slots.clone())
        .with_shutdown_token(stop.clone());
        let metrics = HostStatus::default();
        let task_origin = origin.clone();
        let task_metrics = metrics.clone();
        let task = tokio::spawn(async move {
            zuse_serve::relay::serve_host_with_status(browser, &task_origin, token, task_metrics)
                .await
        });
        state.running = Some(Running {
            origin: origin.clone(),
            metrics,
            stop,
            task,
        });
        state.error = None;
        Ok(Status {
            running: true,
            origin: Some(origin),
            ..Status::default()
        })
    }
    pub async fn stop(&self) -> Status {
        // Hold admission through draining so a replacement never overlaps workers.
        let mut state = self.0.state.lock().await;
        if let Some(running) = state.running.take() {
            running.stop.cancel();
            let mut task = running.task;
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        state.error = None;
        Status::default()
    }
}
