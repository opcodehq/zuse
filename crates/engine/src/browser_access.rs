//! The engine owns listeners started by Settings; RPC clones only hold control.
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{net::TcpListener, sync::Mutex, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use zuse_serve::{BrowserServer, TlsConfig};

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start {
    pub listen: SocketAddr,
    #[serde(default)]
    pub public_origin: Option<String>,
    #[serde(default)]
    pub tls_cert: Option<PathBuf>,
    #[serde(default)]
    pub tls_key: Option<PathBuf>,
    #[serde(default)]
    pub assets: Option<PathBuf>,
}
#[derive(Clone, Default, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub managed: bool,
    pub origin: Option<String>,
    pub listen: Option<SocketAddr>,
    pub local_http: bool,
    pub error: Option<String>,
}
struct Running {
    status: Status,
    shutdown: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
}
#[derive(Default)]
struct State {
    running: Option<Running>,
    external: Option<Status>,
    last_error: Option<String>,
}
struct Shared {
    profile: PathBuf,
    device_id: String,
    pairings: zuse_remote::authority::Authority,
    slots: Arc<tokio::sync::Semaphore>,
    sessions: Arc<zuse_serve::PairedSessions>,
    advertised: tokio::sync::watch::Sender<Vec<zuse_remote::Route>>,
    closed: CancellationToken,
    managed: Option<Arc<dyn zuse_serve::managed::Authority>>,
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct BrowserAccess(Arc<Shared>);
/// Only EngineCore owns this guard. Listener-held RPC clones cannot extend it.
pub struct Owner(BrowserAccess);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.0.closed.cancel();
    }
}
impl Owner {
    pub fn new(
        profile: &Path,
        device_id: String,
        pairings: zuse_remote::authority::Authority,
    ) -> Self {
        Self(BrowserAccess(Arc::new(Shared {
            profile: profile.into(),
            device_id,
            pairings,
            slots: Arc::new(tokio::sync::Semaphore::new(64)),
            sessions: Arc::default(),
            advertised: tokio::sync::watch::Sender::new(vec![]),
            closed: CancellationToken::new(),
            managed: None,
            state: Mutex::new(State::default()),
        })))
    }
    pub fn control(&self) -> BrowserAccess {
        self.0.clone()
    }
    pub fn with_managed_authority(
        mut self,
        authority: Arc<dyn zuse_serve::managed::Authority>,
    ) -> Self {
        Arc::get_mut(&mut self.0.0)
            .expect("configure before sharing browser access")
            .managed = Some(authority);
        self
    }
    pub fn begin_close(&self) {
        self.0.0.closed.cancel();
    }
    pub async fn close(&self) {
        self.0.0.closed.cancel();
        self.0.stop().await;
    }
}
impl BrowserAccess {
    pub fn pairing_slots(&self) -> Arc<tokio::sync::Semaphore> {
        self.0.slots.clone()
    }
    /// Sessions continue across this profile's listeners and connectors.
    pub fn paired_sessions(&self) -> Arc<zuse_serve::PairedSessions> {
        self.0.sessions.clone()
    }
    pub fn advertised_routes(&self) -> tokio::sync::watch::Sender<Vec<zuse_remote::Route>> {
        self.0.advertised.clone()
    }
    /// The foreground server owns its listener and shutdown; Settings can still
    /// issue invitations using its actual origin without starting a second host.
    pub async fn register_external(&self, listen: SocketAddr, origin: String, local_http: bool) {
        let mut state = self.0.state.lock().await;
        if !self.0.closed.is_cancelled() {
            state.external = Some(Status {
                running: true,
                managed: false,
                origin: Some(origin),
                listen: Some(listen),
                local_http,
                error: None,
            });
        }
    }
    pub async fn status(&self) -> Status {
        let mut state = self.0.state.lock().await;
        if self.0.closed.is_cancelled() {
            return Status::default();
        }
        if state
            .running
            .as_ref()
            .is_some_and(|running| running.task.is_finished())
        {
            let running = state.running.take().unwrap();
            state.last_error = match running.task.await {
                Ok(Ok(())) => Some("Browser access stopped".into()),
                _ => Some("Browser access stopped unexpectedly. Start it again after checking its settings.".into()),
            };
        }
        state
            .running
            .as_ref()
            .map(|running| running.status.clone())
            .or_else(|| state.external.clone())
            .unwrap_or_else(|| Status {
                error: state.last_error.clone(),
                ..Default::default()
            })
    }
    pub async fn start(
        &self,
        options: Start,
        service: Arc<dyn zuse_rpc::RpcService>,
    ) -> anyhow::Result<Status> {
        let mut state = self.0.state.lock().await;
        anyhow::ensure!(!self.0.closed.is_cancelled(), "The engine is shutting down");
        if state
            .running
            .as_ref()
            .is_some_and(|running| running.task.is_finished())
        {
            let running = state.running.take().unwrap();
            let _ = running.task.await;
        }
        anyhow::ensure!(
            state.running.is_none() && state.external.is_none(),
            "Browser access is already running"
        );
        anyhow::ensure!(
            options.listen.ip().is_loopback() || options.public_origin.is_some(),
            "Enter a public HTTPS URL before allowing other devices to connect"
        );
        let tls = match (&options.tls_cert, &options.tls_key) {
            (Some(cert), Some(key)) => Some(TlsConfig::from_pem_files(cert, key)?),
            (None, None) => None,
            _ => anyhow::bail!("Choose both a certificate and its private key"),
        };
        // A non-loopback HTTP listener is allowed only behind the explicitly
        // configured HTTPS reverse proxy, matching the foreground server.
        let assets = options.assets.or_else(discover_assets)
            .ok_or_else(||anyhow::anyhow!("Browser app files are missing. Install the full desktop package or build the browser app first."))?;
        anyhow::ensure!(
            assets.join("index.html").is_file(),
            "The browser app folder does not contain index.html"
        );
        let token = BrowserServer::load_or_create_token(&self.0.profile)?;
        let shutdown = self.0.closed.child_token();
        let mut browser = BrowserServer::new(service, token, options.public_origin.clone())?
            .with_pairings(self.0.pairings.clone(), self.0.device_id.clone())
            .with_pairing_slots(self.0.slots.clone())
            .with_paired_sessions(self.0.sessions.clone())
            .with_advertised_routes(self.0.advertised.subscribe())
            .with_shutdown_token(shutdown.clone());
        if let Some(authority) = &self.0.managed {
            browser = browser.with_managed_authority(authority.clone());
        }
        let listener = tokio::select! { biased;
            _ = self.0.closed.cancelled() => anyhow::bail!("The engine is shutting down"),
            listener = TcpListener::bind(options.listen) => listener?,
        };
        let address = listener.local_addr()?;
        let origin = options.public_origin.unwrap_or_else(|| {
            format!(
                "{}://{address}",
                if tls.is_some() { "https" } else { "http" }
            )
        });
        let status = Status {
            running: true,
            managed: true,
            origin: Some(origin),
            listen: Some(address),
            local_http: tls.is_none(),
            error: None,
        };
        let signal = shutdown.clone();
        let owner = self.0.closed.clone();
        let task = tokio::spawn(
            browser.serve_with_shutdown(listener, assets, tls, async move {
                tokio::select! { _=signal.cancelled()=>{}, _=owner.cancelled()=>{} }
            }),
        );
        state.running = Some(Running {
            status: status.clone(),
            shutdown,
            task,
        });
        state.last_error = None;
        Ok(status)
    }
    pub async fn stop(&self) -> Status {
        let running = self.0.state.lock().await.running.take();
        if let Some(running) = running {
            running.shutdown.cancel();
            let mut task = running.task;
            if tokio::time::timeout(std::time::Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        self.status().await
    }
}
/// Packaged web files: beside the executable (Linux libexec, Windows), in the
/// macOS bundle's Resources, or in share/zuse of the standalone CLI archive.
pub fn discover_assets() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ZUSE_WEB_ASSETS").map(PathBuf::from) {
        return Some(path);
    }
    let mut paths = vec![];
    if let Ok(binary) = std::env::current_exe()
        && let Some(directory) = binary.parent()
    {
        paths.push(directory.join("web"));
        paths.push(directory.join("../Resources/web"));
        paths.push(directory.join("../share/zuse/web"));
    }
    if let Ok(directory) = std::env::current_dir() {
        paths.push(directory.join("dist/web"));
    }
    paths
        .into_iter()
        .find(|path| path.join("index.html").is_file())
}
