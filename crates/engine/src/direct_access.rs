//! Direct paired access over the local network and Tailscale, and the list of
//! routes this host offers its paired devices.
//!
//! The main listener serves only encrypted paired RPC: no application files,
//! no cookie sessions and nothing readable without the pairing secret. That is
//! why plain HTTP is acceptable on it, and it still refuses peers outside
//! private address space.
//!
//! Browsers need more. The app uses shared memory and WebGPU, which only exist
//! in a secure context, so a second listener serves it over HTTPS with a
//! certificate generated for this computer. Browsers warn about that
//! certificate once. Tailscale HTTPS reaches the app through a loopback
//! listener instead, with a certificate browsers already trust.
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, Notify, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use zuse_remote::{
    Route, RouteKind,
    routes::{is_private_host, is_tailnet_host},
};
use zuse_serve::{BrowserServer, PairedSessions, TlsConfig};

pub const DEFAULT_PORT: u16 = 27657;
const SERVE_HTTPS_PORT: u16 = 27658;
const LAN_HTTPS_PORT: u16 = 27659;
const FILE: &str = "direct-access.json";
const LAN_CERTIFICATE: &str = "lan-https.json";
const MAX_CERTIFICATE_BYTES: u64 = 64 * 1024;
const MAX_CERTIFICATE_ADDRESSES: usize = 32;
const REFRESH: Duration = Duration::from_secs(15);
const TAILSCALE_TIMEOUT: Duration = Duration::from_secs(3);
const CLOUD_METADATA: &str = "169.254.169.254:80";
const CLOUD_METADATA_TIMEOUT: Duration = Duration::from_millis(300);

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Start {
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub tailscale_serve: bool,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub running: bool,
    pub enabled: bool,
    pub port: Option<u16>,
    pub tailscale_available: bool,
    pub tailscale_serve: bool,
    pub routes: Vec<Route>,
    pub browser_origin: Option<String>,
    pub browser_self_signed: bool,
    pub error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved {
    version: u32,
    device_id: String,
    port: u16,
    tailscale_serve: bool,
    enabled: bool,
}
fn read_saved(profile: &Path, device: &str) -> anyhow::Result<Option<Saved>> {
    let bytes = match std::fs::read(profile.join(FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    anyhow::ensure!(
        bytes.len() <= 16_384,
        "Direct access settings are too large"
    );
    let saved: Saved = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        saved.version == 1 && saved.device_id == device,
        "Direct access settings belong to another profile"
    );
    Ok(Some(saved))
}
fn write_private(profile: &Path, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(profile)?;
    let mut file = tempfile::NamedTempFile::new_in(profile)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(profile.join(name))?;
    Ok(())
}
fn write_saved(profile: &Path, saved: &Saved) -> anyhow::Result<()> {
    write_private(profile, FILE, &serde_json::to_vec_pretty(saved)?)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LanCertificate {
    addresses: Vec<IpAddr>,
    certificate: String,
    key: String,
}
fn read_lan_certificate(profile: &Path) -> Option<LanCertificate> {
    let path = profile.join(LAN_CERTIFICATE);
    if std::fs::metadata(&path).ok()?.len() > MAX_CERTIFICATE_BYTES {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}
/// This computer's stable browser certificate. Earlier addresses stay on it,
/// so returning to a known network does not bring the browser warning back.
fn lan_certificate(profile: &Path, current: &[IpAddr]) -> anyhow::Result<TlsConfig> {
    let saved = read_lan_certificate(profile);
    if let Some(saved) = &saved
        && current.iter().all(|ip| saved.addresses.contains(ip))
        && let Ok(config) = TlsConfig::from_pem(saved.certificate.as_bytes(), saved.key.as_bytes())
    {
        return Ok(config);
    }
    let mut addresses = current.to_vec();
    for ip in saved.into_iter().flat_map(|saved| saved.addresses) {
        if !addresses.contains(&ip) {
            addresses.push(ip);
        }
    }
    addresses.truncate(MAX_CERTIFICATE_ADDRESSES);
    let names = addresses
        .iter()
        .map(IpAddr::to_string)
        .chain(["localhost".to_owned()])
        .collect::<Vec<_>>();
    let rcgen::CertifiedKey { cert, signing_key } = rcgen::generate_simple_self_signed(names)?;
    let generated = LanCertificate {
        addresses,
        certificate: cert.pem(),
        key: signing_key.serialize_pem(),
    };
    write_private(
        profile,
        LAN_CERTIFICATE,
        &serde_json::to_vec_pretty(&generated)?,
    )?;
    TlsConfig::from_pem(generated.certificate.as_bytes(), generated.key.as_bytes())
}
fn certificate_addresses(interfaces: &[(String, IpAddr)]) -> Vec<IpAddr> {
    let mut addresses: Vec<IpAddr> = interfaces
        .iter()
        .filter(|(name, ip)| {
            let text = ip.to_string();
            ip.is_ipv4()
                && !ip.is_loopback()
                && !is_virtual_interface(name)
                && (is_private_host(&text) || is_tailnet_host(&text))
        })
        .map(|(_, ip)| *ip)
        .collect();
    addresses.sort();
    addresses.dedup();
    addresses
}

/// Interface names of container and VM bridges, whose addresses no other
/// device can reach.
fn is_virtual_interface(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "docker", "br-", "veth", "virbr", "vmnet", "vboxnet", "podman", "cni", "flannel", "kube",
        "zt", "wg",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
}
fn origin_for(ip: IpAddr, port: u16) -> String {
    format!("http://{}", SocketAddr::new(ip, port))
}

/// Routes for the listener's port. Advertised addresses are only hints: every
/// client still proves a route by completing Noise through it.
fn interface_routes(
    interfaces: &[(String, IpAddr)],
    tailnet_addresses: &[IpAddr],
    port: u16,
    on_cloud_vm: bool,
) -> Vec<Route> {
    let tailscale_interfaces: Vec<&str> = interfaces
        .iter()
        .filter(|(name, ip)| {
            name.to_ascii_lowercase().starts_with("tailscale")
                || (ip.is_ipv6() && is_tailnet_host(&ip.to_string()))
        })
        .map(|(name, _)| name.as_str())
        .collect();
    let mut routes: Vec<Route> = interfaces
        .iter()
        .filter_map(|(name, ip)| {
            let IpAddr::V4(v4) = ip else { return None };
            if v4.is_loopback() || v4.is_link_local() {
                return None;
            }
            let text = v4.to_string();
            // The tailnet's range is shared with other carrier-grade NAT users.
            if is_tailnet_host(&text) {
                let confirmed =
                    tailscale_interfaces.contains(&name.as_str()) || tailnet_addresses.contains(ip);
                return confirmed.then(|| Route {
                    kind: RouteKind::Tailnet,
                    origin: origin_for(*ip, port),
                });
            }
            // A cloud VM's private network belongs to its provider.
            (!on_cloud_vm && is_private_host(&text) && !is_virtual_interface(name)).then(|| Route {
                kind: RouteKind::Lan,
                origin: origin_for(*ip, port),
            })
        })
        .collect();
    routes.sort_by(|a, b| (a.kind, &a.origin).cmp(&(b.kind, &b.origin)));
    routes.dedup();
    routes
}

fn local_interfaces() -> Vec<(String, IpAddr)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|interface| {
            let ip = interface.ip();
            (interface.name, ip)
        })
        .collect()
}

fn tailscale_binary() -> PathBuf {
    let bundled = Path::new("/Applications/Tailscale.app/Contents/MacOS/Tailscale");
    if cfg!(target_os = "macos") && bundled.is_file() {
        bundled.into()
    } else {
        "tailscale".into()
    }
}
async fn tailscale(arguments: &[&str]) -> Option<Vec<u8>> {
    let run = tokio::process::Command::new(tailscale_binary())
        .args(arguments)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(TAILSCALE_TIMEOUT, run)
        .await
        .ok()?
        .ok()?;
    output.status.success().then_some(output.stdout)
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TailscaleSelf {
    dns_name: Option<String>,
    addresses: Vec<IpAddr>,
}
fn parse_tailscale_status(json: &[u8]) -> Option<TailscaleSelf> {
    let status: serde_json::Value = serde_json::from_slice(json).ok()?;
    if status["BackendState"] != "Running" {
        return None;
    }
    let me = &status["Self"];
    Some(TailscaleSelf {
        dns_name: me["DNSName"]
            .as_str()
            .map(|name| name.trim_end_matches('.').to_owned())
            .filter(|name| !name.is_empty()),
        addresses: me["TailscaleIPs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|ip| ip.as_str()?.parse().ok())
            .collect(),
    })
}
async fn read_tailscale() -> Option<TailscaleSelf> {
    parse_tailscale_status(&tailscale(&["status", "--json"]).await?)
}
async fn enable_tailscale_serve(port: u16) -> bool {
    let https = format!("--https={SERVE_HTTPS_PORT}");
    let target = format!("http://127.0.0.1:{port}");
    tailscale(&["serve", "--bg", &https, &target])
        .await
        .is_some()
}
async fn disable_tailscale_serve() {
    let https = format!("--https={SERVE_HTTPS_PORT}");
    let _ = tailscale(&["serve", &https, "off"]).await;
}
fn on_cloud_vm() -> bool {
    use std::net::TcpStream;
    CLOUD_METADATA
        .parse::<SocketAddr>()
        .is_ok_and(|address| TcpStream::connect_timeout(&address, CLOUD_METADATA_TIMEOUT).is_ok())
}

struct Running {
    port: u16,
    stop: CancellationToken,
    task: JoinHandle<anyhow::Result<()>>,
    app: BrowserApp,
}
/// Listeners that carry the browser app. Missing web files or a failed
/// certificate only cost browser access; paired apps keep working without them.
#[derive(Default)]
struct BrowserApp {
    https_port: Option<u16>,
    tailscale_target: Option<u16>,
    tasks: Vec<JoinHandle<anyhow::Result<()>>>,
}
#[derive(Default)]
struct State {
    running: Option<Running>,
    saved: Option<Saved>,
    tailscale: Option<TailscaleSelf>,
    routes: Vec<Route>,
    error: Option<String>,
}
impl State {
    fn status(&self) -> Status {
        let browser = self.browser_origin();
        Status {
            running: self.running.is_some(),
            enabled: self.saved.as_ref().is_some_and(|saved| saved.enabled),
            port: self
                .running
                .as_ref()
                .map(|running| running.port)
                .or_else(|| self.saved.as_ref().map(|saved| saved.port)),
            tailscale_available: self.tailscale.is_some(),
            tailscale_serve: self
                .saved
                .as_ref()
                .is_some_and(|saved| saved.tailscale_serve),
            routes: self.routes.clone(),
            browser_origin: browser.as_ref().map(|(origin, _)| origin.clone()),
            browser_self_signed: browser.is_some_and(|(_, self_signed)| self_signed),
            error: self.error.clone(),
        }
    }
    /// Where a browser can open this computer, and whether it will see a
    /// warning first. A trusted tailnet certificate wins over a generated one.
    fn browser_origin(&self) -> Option<(String, bool)> {
        let app = &self.running.as_ref()?.app;
        if app.tailscale_target.is_some()
            && let Some(route) = self.routes.iter().find(|route| {
                route.kind == RouteKind::Tailnet && route.origin.starts_with("https:")
            })
        {
            return Some((route.origin.clone(), false));
        }
        let port = app.https_port?;
        let route = self
            .routes
            .iter()
            .filter(|route| matches!(route.kind, RouteKind::Lan | RouteKind::Tailnet))
            .filter(|route| route.origin.starts_with("http:"))
            .min_by_key(|route| route.kind)?;
        let (host, _) = route.origin.strip_prefix("http://")?.rsplit_once(':')?;
        Some((format!("https://{host}:{port}"), true))
    }
}
/// Other reachable addresses of this host, supplied by their own owners.
#[derive(Clone, Default)]
pub struct OtherRoutes {
    pub relay: Option<crate::relay_access::RelayAccess>,
    pub browser: Option<crate::browser_access::BrowserAccess>,
    pub managed: Option<crate::managed_access::host::Host>,
}
struct Shared {
    profile: PathBuf,
    device: String,
    pairings: zuse_remote::authority::Authority,
    slots: Arc<tokio::sync::Semaphore>,
    sessions: Arc<PairedSessions>,
    advertised: watch::Sender<Vec<Route>>,
    other: std::sync::Mutex<OtherRoutes>,
    closed: CancellationToken,
    refresh: Notify,
    cloud_vm: tokio::sync::OnceCell<bool>,
    /// Web files chosen by the host process; otherwise the installed ones.
    app_assets: std::sync::Mutex<Option<PathBuf>>,
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct DirectAccess(Arc<Shared>);
pub struct Owner(DirectAccess);
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
        browser: &crate::browser_access::BrowserAccess,
    ) -> Self {
        let state = match read_saved(&profile, &device) {
            Ok(saved) => State {
                saved,
                ..Default::default()
            },
            Err(_) => State {
                error: Some(
                    "Saved network settings could not be loaded. Turn access on again.".into(),
                ),
                ..Default::default()
            },
        };
        Self(DirectAccess(Arc::new(Shared {
            profile,
            device,
            pairings,
            slots: browser.pairing_slots(),
            sessions: browser.paired_sessions(),
            advertised: browser.advertised_routes(),
            other: Default::default(),
            closed: CancellationToken::new(),
            refresh: Notify::new(),
            cloud_vm: Default::default(),
            app_assets: Default::default(),
            state: Mutex::new(state),
        })))
    }
    pub fn control(&self) -> DirectAccess {
        self.0.clone()
    }
    pub fn begin_close(&self) {
        self.0.0.closed.cancel();
        self.0.0.sessions.close();
    }
    pub async fn close(&self) {
        self.begin_close();
        self.0.halt().await;
    }
}
impl DirectAccess {
    pub fn set_other_routes(&self, other: OtherRoutes) {
        *self.0.other.lock().unwrap_or_else(|e| e.into_inner()) = other;
        self.0.refresh.notify_one();
    }
    pub async fn status(&self) -> Status {
        if self.0.closed.is_cancelled() {
            return Status::default();
        }
        // The settings screen offers Tailscale before anything is running.
        if self.0.state.lock().await.running.is_none() {
            let found = read_tailscale().await;
            self.0.state.lock().await.tailscale = found;
        }
        self.0.state.lock().await.status()
    }
    /// Where a browser can open this computer directly, if anywhere.
    pub async fn browser_origin(&self) -> Option<(String, bool)> {
        self.0.state.lock().await.browser_origin()
    }
    fn paired_server(
        &self,
        service: Arc<dyn zuse_rpc::RpcService>,
        stop: &CancellationToken,
    ) -> anyhow::Result<BrowserServer> {
        Ok(
            BrowserServer::new(service, BrowserServer::random_token(), None)?
                .with_pairings(self.0.pairings.clone(), self.0.device.clone())
                .with_pairing_slots(self.0.slots.clone())
                .with_paired_sessions(self.0.sessions.clone())
                .with_advertised_routes(self.0.advertised.subscribe())
                .with_shutdown_token(stop.clone()),
        )
    }
    async fn start_browser_app(
        &self,
        service: &Arc<dyn zuse_rpc::RpcService>,
        stop: &CancellationToken,
        tailscale: bool,
    ) -> anyhow::Result<BrowserApp> {
        let mut app = BrowserApp::default();
        let chosen = self
            .0
            .app_assets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(assets) = chosen.or_else(crate::browser_access::discover_assets) else {
            return Ok(app);
        };
        let any = |port| SocketAddr::from(([0, 0, 0, 0], port));
        let tls = lan_certificate(&self.0.profile, &certificate_addresses(&local_interfaces()))?;
        let listener = match TcpListener::bind(any(LAN_HTTPS_PORT)).await {
            Ok(listener) => listener,
            Err(_) => TcpListener::bind(any(0)).await?,
        };
        app.https_port = Some(listener.local_addr()?.port());
        app.tasks.push(tokio::spawn(
            self.paired_server(service.clone(), stop)?.serve_paired_app(
                listener,
                assets.clone(),
                Some(tls),
                stop.clone().cancelled_owned(),
            ),
        ));
        if tailscale {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
            app.tailscale_target = Some(listener.local_addr()?.port());
            app.tasks.push(tokio::spawn(
                self.paired_server(service.clone(), stop)?.serve_paired_app(
                    listener,
                    assets,
                    None,
                    stop.clone().cancelled_owned(),
                ),
            ));
        }
        Ok(app)
    }
    /// The address to put in an invitation when nothing public is available.
    pub async fn invitation_origin(&self) -> Option<String> {
        let state = self.0.state.lock().await;
        state.running.as_ref()?;
        state
            .routes
            .iter()
            .filter(|route| matches!(route.kind, RouteKind::Lan | RouteKind::Tailnet))
            .min_by_key(|route| route.kind)
            .map(|route| route.origin.clone())
    }
    async fn activate(
        &self,
        saved: Saved,
        persist: bool,
        service: Arc<dyn zuse_rpc::RpcService>,
    ) -> anyhow::Result<Status> {
        let mut state = self.0.state.lock().await;
        anyhow::ensure!(!self.0.closed.is_cancelled(), "The engine is shutting down");
        anyhow::ensure!(state.running.is_none(), "Network access is already on");
        // A busy port should not cost the feature: clients learn the new one.
        let any = |port| SocketAddr::from(([0, 0, 0, 0], port));
        let listener = match TcpListener::bind(any(saved.port)).await {
            Ok(listener) => listener,
            Err(_) => TcpListener::bind(any(0)).await?,
        };
        let port = listener.local_addr()?.port();
        let stop = self.0.closed.child_token();
        let server = self.paired_server(service.clone(), &stop)?;
        if persist {
            write_saved(&self.0.profile, &saved)?;
        }
        let app = self
            .start_browser_app(&service, &stop, saved.tailscale_serve)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!("Browser access over the network is unavailable: {error:#}");
                BrowserApp::default()
            });
        let signal = stop.clone();
        let task = tokio::spawn(server.serve_paired_only(listener, signal.cancelled_owned()));
        let serve_failed = saved.tailscale_serve
            && !enable_tailscale_serve(app.tailscale_target.unwrap_or(port)).await;
        state.running = Some(Running {
            port,
            stop,
            task,
            app,
        });
        state.error = serve_failed.then(|| {
            "Tailscale HTTPS could not be turned on. Check that Tailscale is running and HTTPS is enabled for your tailnet.".to_owned()
        });
        state.saved = Some(saved);
        drop(state);
        self.refresh_routes().await;
        Ok(self.0.state.lock().await.status())
    }
    /// Serve these web files on the browser listeners, matching the host's
    /// own listener instead of whatever installation discovery finds.
    pub fn use_app_assets(&self, assets: PathBuf) {
        *self.0.app_assets.lock().unwrap_or_else(|e| e.into_inner()) = Some(assets);
    }
    pub async fn start(
        &self,
        options: Start,
        service: Arc<dyn zuse_rpc::RpcService>,
    ) -> anyhow::Result<Status> {
        self.halt().await;
        let saved = Saved {
            version: 1,
            device_id: self.0.device.clone(),
            port: options.port.unwrap_or(DEFAULT_PORT),
            tailscale_serve: options.tailscale_serve,
            enabled: true,
        };
        self.activate(saved, true, service).await
    }
    async fn halt(&self) {
        let (running, serve) = {
            let mut state = self.0.state.lock().await;
            let serve = state
                .saved
                .as_ref()
                .is_some_and(|saved| saved.tailscale_serve);
            (state.running.take(), serve)
        };
        let Some(running) = running else { return };
        running.stop.cancel();
        for mut task in std::iter::once(running.task).chain(running.app.tasks) {
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        if serve {
            disable_tailscale_serve().await;
        }
    }
    pub async fn stop(&self) -> Status {
        self.halt().await;
        {
            let mut state = self.0.state.lock().await;
            state.error = None;
            if let Some(mut saved) = state.saved.clone() {
                saved.enabled = false;
                if write_saved(&self.0.profile, &saved).is_ok() {
                    state.saved = Some(saved);
                }
            }
        }
        self.refresh_routes().await;
        self.status().await
    }
    /// Called once by runtime assembly, after the complete RPC service exists.
    pub async fn restore_on_startup(&self, service: Arc<dyn zuse_rpc::RpcService>) {
        zuse_sync::net_path::spawn_path_monitor();
        let saved = self.0.state.lock().await.saved.clone();
        if let Some(saved) = saved.filter(|saved| saved.enabled)
            && self.activate(saved, false, service).await.is_err()
        {
            self.0.state.lock().await.error =
                Some("Network access could not start. Turn it on again from Devices.".into());
        }
        let access = self.clone();
        tokio::spawn(async move { access.advertise().await });
    }
    async fn advertise(&self) {
        let mut network = zuse_sync::wake::subscribe_network_changed();
        loop {
            self.refresh_routes().await;
            tokio::select! {
                _ = self.0.closed.cancelled() => return,
                _ = self.0.refresh.notified() => {},
                _ = network.recv() => {},
                _ = tokio::time::sleep(REFRESH) => {},
            }
        }
    }
    async fn other_routes(&self) -> Vec<Route> {
        let other = self
            .0
            .other
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let mut routes = vec![];
        if let Some(relay) = &other.relay {
            let status = relay.status().await;
            if let Some(origin) = status.origin.filter(|_| status.running) {
                routes.push(Route {
                    kind: RouteKind::Relay,
                    origin,
                });
            }
        }
        if let Some(managed) = &other.managed {
            let status = managed.status();
            if status.enabled
                && status.account_matches
                && let Some(endpoint) = status.endpoint
            {
                routes.push(Route {
                    kind: RouteKind::Relay,
                    origin: endpoint.http_base_url.trim_end_matches('/').to_owned(),
                });
            }
        }
        if let Some(browser) = &other.browser
            && let Some(origin) = browser
                .status()
                .await
                .origin
                .filter(|o| o.starts_with("https:"))
        {
            routes.push(Route {
                kind: zuse_remote::routes::classify_origin(&origin),
                origin,
            });
        }
        routes
    }
    async fn refresh_routes(&self) {
        let serving = self.0.state.lock().await.running.is_some();
        let tailscale = if serving {
            read_tailscale().await
        } else {
            None
        };
        let cloud = serving
            && *self
                .0
                .cloud_vm
                .get_or_init(|| async {
                    tokio::task::spawn_blocking(on_cloud_vm)
                        .await
                        .unwrap_or(false)
                })
                .await;
        let mut routes = {
            let state = self.0.state.lock().await;
            match &state.running {
                Some(running) => {
                    let addresses = tailscale
                        .as_ref()
                        .map(|me| me.addresses.clone())
                        .unwrap_or_default();
                    let mut routes =
                        interface_routes(&local_interfaces(), &addresses, running.port, cloud);
                    let serve = state
                        .saved
                        .as_ref()
                        .is_some_and(|saved| saved.tailscale_serve);
                    if serve
                        && state.error.is_none()
                        && let Some(name) = tailscale.as_ref().and_then(|me| me.dns_name.as_ref())
                    {
                        routes.push(Route {
                            kind: RouteKind::Tailnet,
                            origin: format!("https://{name}:{SERVE_HTTPS_PORT}"),
                        });
                    }
                    routes
                }
                None => vec![],
            }
        };
        routes.extend(self.other_routes().await);
        let mut seen = std::collections::HashSet::new();
        routes.retain(|route| seen.insert(route.origin.clone()));
        {
            let mut state = self.0.state.lock().await;
            if serving {
                state.tailscale = tailscale;
            }
            state.routes = routes.clone();
        }
        self.0.advertised.send_if_modified(|current| {
            let changed = *current != routes;
            *current = routes;
            changed
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }
    fn interfaces(items: &[(&str, &str)]) -> Vec<(String, IpAddr)> {
        items.iter().map(|(n, a)| (n.to_string(), ip(a))).collect()
    }

    #[test]
    fn advertises_real_networks_and_skips_bridges() {
        let routes = interface_routes(
            &interfaces(&[
                ("lo", "127.0.0.1"),
                ("wlan0", "192.168.1.20"),
                ("docker0", "172.17.0.1"),
                ("br-a1", "172.18.0.1"),
                ("wlan0", "169.254.3.3"),
                ("eth1", "8.8.8.8"),
            ]),
            &[],
            27657,
            false,
        );
        assert_eq!(
            routes,
            vec![Route {
                kind: RouteKind::Lan,
                origin: "http://192.168.1.20:27657".into(),
            }]
        );
    }

    #[test]
    fn shared_address_space_needs_tailscale_evidence() {
        let other_vpn = interface_routes(&interfaces(&[("warp0", "100.96.0.2")]), &[], 1, false);
        assert!(other_vpn.is_empty());
        let by_name = interface_routes(&interfaces(&[("tailscale0", "100.96.0.2")]), &[], 1, false);
        assert_eq!(by_name[0].kind, RouteKind::Tailnet);
        let by_sibling = interface_routes(
            &interfaces(&[("utun4", "100.96.0.2"), ("utun4", "fd7a:115c:a1e0::1")]),
            &[],
            1,
            false,
        );
        assert_eq!(by_sibling[0].kind, RouteKind::Tailnet);
        let by_status = interface_routes(
            &interfaces(&[("utun4", "100.96.0.2")]),
            &[ip("100.96.0.2")],
            1,
            false,
        );
        assert_eq!(by_status[0].kind, RouteKind::Tailnet);
    }

    #[test]
    fn cloud_machines_only_offer_their_tailnet() {
        let routes = interface_routes(
            &interfaces(&[("eth0", "10.0.0.4"), ("tailscale0", "100.100.1.1")]),
            &[],
            1,
            true,
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].kind, RouteKind::Tailnet);
    }

    #[test]
    fn certificates_cover_real_networks_and_survive_moving_between_them() {
        let directory = tempfile::tempdir().unwrap();
        let addresses = certificate_addresses(&interfaces(&[
            ("lo", "127.0.0.1"),
            ("wlan0", "192.168.1.20"),
            ("docker0", "172.17.0.1"),
            ("eth1", "8.8.8.8"),
        ]));
        assert_eq!(addresses, vec![ip("192.168.1.20")]);
        lan_certificate(directory.path(), &addresses).unwrap();
        let first = read_lan_certificate(directory.path()).unwrap().certificate;
        lan_certificate(directory.path(), &addresses).unwrap();
        assert_eq!(
            read_lan_certificate(directory.path()).unwrap().certificate,
            first,
            "the same network must keep the certificate the browser accepted"
        );
        lan_certificate(directory.path(), &[ip("10.0.0.9")]).unwrap();
        let moved = read_lan_certificate(directory.path()).unwrap();
        assert_ne!(moved.certificate, first);
        assert_eq!(moved.addresses, vec![ip("10.0.0.9"), ip("192.168.1.20")]);
        lan_certificate(directory.path(), &addresses).unwrap();
        assert_eq!(
            read_lan_certificate(directory.path()).unwrap().certificate,
            moved.certificate,
            "returning to a known network must not bring the warning back"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(directory.path().join(LAN_CERTIFICATE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    fn running(app: BrowserApp) -> Running {
        Running {
            port: 27657,
            stop: CancellationToken::new(),
            task: tokio::spawn(async { Ok(()) }),
            app,
        }
    }

    #[tokio::test]
    async fn browsers_open_the_trusted_tailnet_before_the_generated_certificate() {
        let lan = Route {
            kind: RouteKind::Lan,
            origin: "http://192.168.1.20:27657".into(),
        };
        let tailnet_https = Route {
            kind: RouteKind::Tailnet,
            origin: "https://box.tail1.ts.net:27658".into(),
        };
        let mut state = State {
            routes: vec![lan.clone()],
            ..Default::default()
        };
        assert_eq!(state.browser_origin(), None, "nothing is listening");
        state.running = Some(running(BrowserApp::default()));
        assert_eq!(state.browser_origin(), None, "no web files were found");
        state.running = Some(running(BrowserApp {
            https_port: Some(27659),
            ..Default::default()
        }));
        assert_eq!(
            state.browser_origin(),
            Some(("https://192.168.1.20:27659".into(), true))
        );
        state.routes.push(tailnet_https);
        assert_eq!(
            state.browser_origin(),
            Some(("https://192.168.1.20:27659".into(), true)),
            "Tailscale HTTPS only carries the app when its listener exists"
        );
        state.running = Some(running(BrowserApp {
            https_port: Some(27659),
            tailscale_target: Some(40000),
            ..Default::default()
        }));
        assert_eq!(
            state.browser_origin(),
            Some(("https://box.tail1.ts.net:27658".into(), false))
        );
    }

    #[test]
    fn reads_tailscale_status() {
        let status = br#"{"BackendState":"Running","Self":{"DNSName":"box.tail1.ts.net.","TailscaleIPs":["100.100.1.1","fd7a:115c:a1e0::1"]}}"#;
        let me = parse_tailscale_status(status).unwrap();
        assert_eq!(me.dns_name.as_deref(), Some("box.tail1.ts.net"));
        assert_eq!(me.addresses.len(), 2);
        assert!(parse_tailscale_status(br#"{"BackendState":"Stopped"}"#).is_none());
        assert!(parse_tailscale_status(b"not json").is_none());
    }
}
