//! Profile-owned outbound connector and durable startup preference.
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
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
    pub configured: bool,
    pub auto_start: bool,
    pub origin: Option<String>,
    pub sockets: usize,
    pub sessions: usize,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved {
    version: u32,
    device_id: String,
    origin: String,
    host_token_file: PathBuf,
    auto_start: bool,
}
fn read_saved(profile: &Path, device: &str) -> anyhow::Result<Option<Saved>> {
    let file = match std::fs::File::open(profile.join("relay-access.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(16_385).read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= 16_384,
        "Relay settings exceed their size limit"
    );
    let saved: Saved = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        saved.version == 1 && saved.device_id == device,
        "Relay settings belong to another profile"
    );
    anyhow::ensure!(
        saved.host_token_file.is_absolute(),
        "Relay credential path must be absolute"
    );
    zuse_serve::relay::validate_origin(&saved.origin)?;
    Ok(Some(saved))
}
fn write_saved(profile: &Path, saved: &Saved) -> anyhow::Result<()> {
    std::fs::create_dir_all(profile)?;
    let bytes = serde_json::to_vec_pretty(saved)?;
    anyhow::ensure!(
        bytes.len() < 16_384,
        "Relay settings exceed their size limit"
    );
    let mut file = tempfile::NamedTempFile::new_in(profile)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(profile.join("relay-access.json"))?;
    #[cfg(unix)]
    std::fs::File::open(profile)?.sync_all()?;
    Ok(())
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
    saved: Option<Saved>,
    error: Option<String>,
}
impl State {
    fn status(&self) -> Status {
        Status {
            running: self.running.is_some(),
            configured: self.saved.is_some(),
            auto_start: self.saved.as_ref().is_some_and(|saved| saved.auto_start),
            origin: self
                .running
                .as_ref()
                .map(|r| r.origin.clone())
                .or_else(|| self.saved.as_ref().map(|s| s.origin.clone())),
            sockets: self.running.as_ref().map_or(0, |r| r.metrics.sockets()),
            sessions: self.running.as_ref().map_or(0, |r| r.metrics.sessions()),
            error: self.error.clone(),
        }
    }
}
struct Shared {
    profile: PathBuf,
    device: String,
    pairings: zuse_remote::authority::Authority,
    slots: Arc<tokio::sync::Semaphore>,
    sessions: Arc<zuse_serve::PairedSessions>,
    advertised: tokio::sync::watch::Sender<Vec<zuse_remote::Route>>,
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
        let state = match read_saved(&profile, &device) {
            Ok(saved) => State { saved, ..Default::default() },
            Err(_) => State { error: Some("Saved relay settings could not be loaded. Enter the relay configuration again.".into()), ..Default::default() },
        };
        Self(RelayAccess(Arc::new(Shared {
            profile,
            device,
            pairings,
            slots,
            sessions: Arc::default(),
            advertised: tokio::sync::watch::Sender::new(vec![]),
            closed: CancellationToken::new(),
            state: Mutex::new(state),
        })))
    }
    /// Share one session table and route list with the other listeners.
    pub fn with_paired_state(
        mut self,
        sessions: Arc<zuse_serve::PairedSessions>,
        advertised: tokio::sync::watch::Sender<Vec<zuse_remote::Route>>,
    ) -> Self {
        let shared = Arc::get_mut(&mut self.0.0).expect("configure before sharing relay access");
        shared.sessions = sessions;
        shared.advertised = advertised;
        self
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
        state.status()
    }
    // All starts, saved restoration and preference writes share one admission
    // lock. Shutdown owns a separate cancellation token and always wins admission.
    fn activate(
        &self,
        state: &mut State,
        saved: Saved,
        persist: bool,
        service: Arc<dyn zuse_rpc::RpcService>,
    ) -> anyhow::Result<Status> {
        anyhow::ensure!(!self.0.closed.is_cancelled(), "The engine is shutting down");
        if state.running.as_ref().is_some_and(|r| r.task.is_finished()) {
            state.running.take();
        }
        anyhow::ensure!(
            state.running.is_none(),
            "Stop relay access before changing its configuration"
        );
        let token = HostToken::read_file(&saved.host_token_file)?;
        let stop = self.0.closed.child_token();
        let browser = BrowserServer::new(
            service,
            BrowserServer::load_or_create_token(&self.0.profile)?,
            None,
        )?
        .with_pairings(self.0.pairings.clone(), self.0.device.clone())
        .with_pairing_slots(self.0.slots.clone())
        .with_paired_sessions(self.0.sessions.clone())
        .with_advertised_routes(self.0.advertised.subscribe())
        .with_shutdown_token(stop.clone());
        if persist {
            write_saved(&self.0.profile, &saved)?;
        }
        let metrics = HostStatus::default();
        let task_origin = saved.origin.clone();
        let task_metrics = metrics.clone();
        let task = tokio::spawn(async move {
            zuse_serve::relay::serve_host_with_status(browser, &task_origin, token, task_metrics)
                .await
        });
        state.running = Some(Running {
            origin: saved.origin.clone(),
            metrics,
            stop,
            task,
        });
        state.saved = Some(saved);
        state.error = None;
        Ok(state.status())
    }
    pub async fn start(
        &self,
        options: Start,
        service: Arc<dyn zuse_rpc::RpcService>,
    ) -> anyhow::Result<Status> {
        let origin = zuse_serve::relay::validate_origin(&options.origin)?;
        let path = std::fs::canonicalize(options.host_token_file)?;
        let mut state = self.0.state.lock().await;
        let saved = Saved {
            version: 1,
            device_id: self.0.device.clone(),
            origin,
            host_token_file: path,
            auto_start: state.saved.as_ref().is_some_and(|s| s.auto_start),
        };
        self.activate(&mut state, saved, true, service)
    }
    pub async fn resume(&self, service: Arc<dyn zuse_rpc::RpcService>) -> anyhow::Result<Status> {
        let mut state = self.0.state.lock().await;
        let saved = state
            .saved
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Configure relay access first"))?;
        self.activate(&mut state, saved, false, service)
    }
    pub async fn set_auto_start(&self, enabled: bool) -> anyhow::Result<Status> {
        let mut state = self.0.state.lock().await;
        anyhow::ensure!(!self.0.closed.is_cancelled(), "The engine is shutting down");
        let mut saved = state
            .saved
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Configure relay access first"))?;
        if enabled {
            HostToken::read_file(&saved.host_token_file)?;
        }
        saved.auto_start = enabled;
        write_saved(&self.0.profile, &saved)?;
        state.saved = Some(saved);
        Ok(state.status())
    }
    /// Called once by runtime assembly, after the complete RPC service exists.
    /// A bad/missing credential disables this boot's connector, not the engine.
    pub async fn restore_on_startup(&self, service: Arc<dyn zuse_rpc::RpcService>) {
        let mut state = self.0.state.lock().await;
        let Some(saved) = state.saved.clone().filter(|s| s.auto_start) else {
            return;
        };
        if state.running.is_some() || self.0.closed.is_cancelled() {
            return;
        }
        if self.activate(&mut state, saved, false, service).is_err() {
            state.error = Some("Automatic relay startup failed. Check the saved relay URL and private credential file, then start again.".into());
        }
    }
    pub async fn stop(&self) -> Status {
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
        if self.0.closed.is_cancelled() {
            Status::default()
        } else {
            state.status()
        }
    }
}
