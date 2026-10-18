//! Durable link ownership and verification of API-minted host grants.
use super::{AccountClient, Endpoint, Error, HostIdentity, LinkReceipt, Result};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, Zeroizing};
use zuse_serve::managed::{Authority, Authorization};
mod install;
mod orphans;
mod runtime;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Saved {
    version: u32,
    device: String,
    subject: String,
    api_url: String,
    enabled: bool,
    label: String,
    local_port: u16,
    environment_id: String,
    endpoint: Endpoint,
    api_issuer: String,
    environment_credential: String,
    mint_public_key: String,
    tunnel_hostname: String,
    connector_token: String,
}
impl Drop for Saved {
    fn drop(&mut self) {
        self.environment_credential.zeroize();
        self.connector_token.zeroize();
    }
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub configured: bool,
    pub enabled: bool,
    pub account_matches: bool,
    pub environment_id: Option<String>,
    pub label: Option<String>,
    pub endpoint: Option<Endpoint>,
    pub local_port: Option<u16>,
    // Link provisioning does not prove connector or host reachability.
    pub tunnel_running: bool,
    pub last_heartbeat: Option<i64>,
    pub error: Option<String>,
}
struct State {
    saved: Option<Saved>,
    key: Option<DecodingKey>,
    retired: CancellationToken,
    error: Option<String>,
    runtime_error: Option<String>,
    tunnel_running: bool,
    last_heartbeat: Option<i64>,
    listener: Option<(u16, CancellationToken)>,
    runtime: Option<tokio::task::JoinHandle<()>>,
    #[cfg(test)]
    fixture_binary: Option<PathBuf>,
}
struct Shared {
    root: PathBuf,
    device: String,
    client: AccountClient,
    gate: tokio::sync::Mutex<()>,
    state: Mutex<State>,
    closed: CancellationToken,
    changed: tokio::sync::watch::Sender<()>,
}
#[derive(Clone)]
pub struct Host(Arc<Shared>);
/// Only EngineCore holds the owner; RPC/server handles cannot outlive its authority.
pub struct Owner(Host);
impl Drop for Owner {
    fn drop(&mut self) {
        self.begin_close();
    }
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
fn decoding_key(saved: &Saved) -> Result<DecodingKey> {
    let value: serde_json::Value =
        serde_json::from_str(&saved.mint_public_key).map_err(|_| Error::Invalid)?;
    if value["kty"] != "OKP" || value["crv"] != "Ed25519" || !value["d"].is_null() {
        return Err(Error::Invalid);
    }
    let jwk = serde_json::from_value(value).map_err(|_| Error::Invalid)?;
    DecodingKey::from_jwk(&jwk).map_err(|_| Error::Invalid)
}
fn read_saved(root: &Path, device: &str, api_url: &str) -> Result<Option<Saved>> {
    let path = root.join("managed-access/link.json");
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Storage),
    };
    if !metadata.is_file() || metadata.len() > 65536 {
        return Err(Error::Invalid);
    }
    let file = std::fs::File::open(&path).map_err(|_| Error::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| Error::Storage)?;
    }
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| Error::Storage)?;
    if bytes.len() > 65536 {
        return Err(Error::Invalid);
    }
    let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    if saved.version != 1
        || saved.device != device
        || saved.environment_id != device
        || saved.api_url != api_url
        || saved.subject.is_empty()
        || saved.local_port == 0
        || saved.environment_credential.is_empty()
        || saved.connector_token.is_empty()
        || saved.environment_credential.len() > 8192
        || saved.connector_token.len() > 16384
        || saved.label.is_empty()
        || saved.label.len() > 128
    {
        return Err(Error::Invalid);
    }
    let origin = saved.endpoint.managed_origin()?;
    zuse_remote::normalized_origin(&saved.api_issuer).map_err(|_| Error::Invalid)?;
    if zuse_remote::normalized_origin(&format!("https://{}", saved.tunnel_hostname))
        .map_err(|_| Error::Invalid)?
        != origin
    {
        return Err(Error::Invalid);
    }
    decoding_key(&saved)?;
    Ok(Some(saved))
}
fn write_saved(root: &Path, saved: &Saved) -> Result<()> {
    let directory = root.join("managed-access");
    std::fs::create_dir_all(&directory).map_err(|_| Error::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Storage)?;
    }
    let bytes = Zeroizing::new(serde_json::to_vec(saved).map_err(|_| Error::Storage)?);
    if bytes.len() > 65536 {
        return Err(Error::Invalid);
    }
    let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|_| Error::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| Error::Storage)?;
    }
    file.write_all(&bytes)
        .and_then(|_| file.as_file().sync_all())
        .map_err(|_| Error::Storage)?;
    file.persist(directory.join("link.json"))
        .map_err(|_| Error::Storage)?;
    #[cfg(unix)]
    std::fs::File::open(&directory)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error::Storage)?;
    Ok(())
}
impl Owner {
    pub fn open(root: &Path, device: &str, client: AccountClient) -> Self {
        let root = std::fs::canonicalize(root)
            .or_else(|_| std::path::absolute(root))
            .unwrap_or_else(|_| root.to_path_buf());
        let root = root.as_path();
        let (saved, key, error) = match read_saved(root, device, client.account.api_url()) {
            Ok(saved) => {
                let key = saved.as_ref().and_then(|s| decoding_key(s).ok());
                (saved, key, None)
            },
            Err(_) => (None, None, Some("Saved managed access settings are invalid. Restore this profile before linking again.".into())),
        };
        Self(Host(Arc::new(Shared {
            root: root.into(),
            device: device.into(),
            client,
            gate: tokio::sync::Mutex::new(()),
            state: Mutex::new(State {
                saved,
                key,
                retired: CancellationToken::new(),
                error,
                runtime_error: None,
                tunnel_running: false,
                last_heartbeat: None,
                listener: None,
                runtime: None,
                #[cfg(test)]
                fixture_binary: None,
            }),
            closed: CancellationToken::new(),
            changed: tokio::sync::watch::channel(()).0,
        })))
    }
    pub fn control(&self) -> Host {
        self.0.clone()
    }
    pub fn begin_close(&self) {
        self.0.0.closed.cancel();
        lock(&self.0.0.state).retired.cancel();
    }
    pub async fn close(&self) {
        self.begin_close();
        let task = lock(&self.0.0.state).runtime.take();
        if let Some(mut task) = task {
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
    }
}
impl Host {
    /// Restore the browser transport only for a saved, enabled link. Foreground
    /// servers already own their listener and do not need this startup helper.
    pub async fn restore_listener(
        &self,
        browser: &crate::browser_access::BrowserAccess,
        service: Arc<dyn zuse_rpc::RpcService>,
    ) {
        let status = self.status();
        if !status.configured || !status.enabled || browser.status().await.running {
            return;
        }
        let options = crate::browser_access::Start {
            listen: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
            public_origin: None,
            tls_cert: None,
            tls_key: None,
            assets: None,
        };
        if browser.start(options, service).await.is_err() {
            lock(&self.0.state).runtime_error=Some("The saved managed connection needs browser app files and an available loopback listener. Check the installation before restarting access.".into());
        }
    }
    #[cfg(test)]
    pub(super) fn fixture_binary(&self, path: PathBuf) {
        lock(&self.0.state).fixture_binary = Some(path);
    }
    pub fn status(&self) -> Status {
        let state = lock(&self.0.state);
        let Some(saved) = &state.saved else {
            return Status {
                error: state.error.clone(),
                ..Default::default()
            };
        };
        let account_matches = self
            .0
            .client
            .account
            .status()
            .user
            .is_some_and(|u| u.id == saved.subject);
        Status {
            configured: true,
            enabled: saved.enabled && !self.0.closed.is_cancelled(),
            account_matches,
            environment_id: Some(saved.environment_id.clone()),
            label: Some(saved.label.clone()),
            endpoint: Some(saved.endpoint.clone()),
            local_port: Some(saved.local_port),
            tunnel_running: state.tunnel_running
                && account_matches
                && !self.0.closed.is_cancelled(),
            last_heartbeat: state.last_heartbeat,
            error: state.error.clone().or_else(|| state.runtime_error.clone()),
        }
    }
    pub async fn link(&self, label: &str, port: u16) -> Result<Status> {
        let _gate = self.0.gate.lock().await;
        if self.0.closed.is_cancelled() {
            return Err(Error::Changed);
        }
        {
            let state = lock(&self.0.state);
            if state.error.is_some() || state.saved.is_some() {
                return Err(Error::Rejected);
            }
        }
        let lease = self.0.client.lease().await?;
        let identity = HostIdentity::open(&self.0.root, &self.0.device)?;
        let receipt = self.0.client.link(&identity, label, port).await?;
        lease.check(&self.0.client.account)?;
        let LinkReceipt {
            environment_id,
            endpoint,
            api_issuer,
            environment_credential,
            mint_public_key,
            tunnel_hostname,
            connector_token,
        } = receipt;
        let saved = Saved {
            version: 1,
            device: self.0.device.clone(),
            subject: lease.subject.clone(),
            api_url: self.0.client.account.api_url().into(),
            enabled: true,
            label: label.trim().into(),
            local_port: port,
            environment_id,
            endpoint,
            api_issuer,
            environment_credential,
            mint_public_key,
            tunnel_hostname: tunnel_hostname.ok_or(Error::Invalid)?,
            connector_token: connector_token.ok_or(Error::Invalid)?,
        };
        let key = decoding_key(&saved)?;
        let mut state = lock(&self.0.state);
        if self.0.closed.is_cancelled() {
            return Err(Error::Changed);
        }
        lease.check(&self.0.client.account)?;
        write_saved(&self.0.root, &saved)?;
        state.retired.cancel();
        state.retired = CancellationToken::new();
        state.saved = Some(saved);
        state.key = Some(key);
        drop(state);
        self.0.changed.send_replace(());
        lease.check(&self.0.client.account)?;
        Ok(self.status())
    }
    /// Disable locally and persist before the uncertain remote write. A failed
    /// unlink keeps a disabled record for retry; restart can never re-enable it.
    pub async fn unlink(&self) -> Result<Status> {
        let _gate = self.0.gate.lock().await;
        let lease = self.0.client.lease().await?;
        let environment = {
            let mut state = lock(&self.0.state);
            let saved = state.saved.as_mut().ok_or(Error::Rejected)?;
            if saved.subject != lease.subject {
                return Err(Error::Rejected);
            }
            saved.enabled = false;
            let result = write_saved(&self.0.root, saved);
            let environment = saved.environment_id.clone();
            state.retired.cancel();
            self.0.changed.send_replace(());
            result?;
            environment
        };
        lease.check(&self.0.client.account)?;
        self.0.client.unlink(&environment).await?;
        lease.check(&self.0.client.account)?;
        let mut state = lock(&self.0.state);
        // Retain a durable tombstone until the remote unlink is confirmed.
        std::fs::remove_file(self.0.root.join("managed-access/link.json"))
            .map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        std::fs::File::open(self.0.root.join("managed-access"))
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Storage)?;
        state.saved = None;
        state.key = None;
        state.runtime_error = None;
        state.last_heartbeat = None;
        drop(state);
        self.0.changed.send_replace(());
        Ok(self.status())
    }
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claims {
    sub: String,
    environment_id: String,
    exp: i64,
}
impl Authority for Host {
    fn listener_opened(
        &self,
        address: std::net::SocketAddr,
        local_http: bool,
        closed: CancellationToken,
    ) {
        if !local_http
            || address.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            || self.0.closed.is_cancelled()
        {
            return;
        }
        let mut state = lock(&self.0.state);
        if state
            .listener
            .as_ref()
            .is_some_and(|(port, stop)| !stop.is_cancelled() && *port != address.port())
        {
            return;
        }
        state.listener = Some((address.port(), closed));
        if state.runtime.is_none() {
            let host = self.clone();
            state.runtime = Some(tokio::spawn(async move {
                runtime::supervise(host).await;
            }));
        }
        self.0.changed.send_replace(());
    }
    fn valid_origin(&self, origin: &str) -> bool {
        if self.0.closed.is_cancelled() {
            return false;
        }
        let Some(user) = self.0.client.account.status().user else {
            return false;
        };
        let state = lock(&self.0.state);
        state.saved.as_ref().is_some_and(|saved| {
            saved.enabled
                && saved.subject == user.id
                && !state.retired.is_cancelled()
                && origin == saved.endpoint.http_base_url.trim_end_matches('/')
        })
    }
    fn authorize(&self, token: &str) -> Option<Authorization> {
        if token.is_empty() || token.len() > 16384 || self.0.closed.is_cancelled() {
            return None;
        }
        let account_closed = self.0.client.account.session_cancellation();
        if account_closed.is_cancelled() {
            return None;
        }
        let account = self.0.client.account.status().user?;
        let state = lock(&self.0.state);
        let saved = state.saved.as_ref()?;
        if !saved.enabled || saved.subject != account.id || state.retired.is_cancelled() {
            return None;
        }
        let header = jsonwebtoken::decode_header(token).ok()?;
        if header.alg != Algorithm::EdDSA || header.typ.as_deref() != Some("connect+jwt") {
            return None;
        }
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.leeway = 0;
        validation.validate_nbf = true;
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.set_issuer(&[saved.api_issuer.as_str()]);
        validation.set_audience(&[format!("zuse-env:{}", saved.environment_id)]);
        let claims = jsonwebtoken::decode::<Claims>(token, state.key.as_ref()?, &validation)
            .ok()?
            .claims;
        let millis = claims
            .exp
            .checked_mul(1000)?
            .checked_sub(chrono::Utc::now().timestamp_millis())?;
        if claims.environment_id != saved.environment_id
            || claims.sub.is_empty()
            || claims.sub.len() > 256
            || millis <= 0
            || millis > 86400000
        {
            return None;
        }
        let grant = Authorization {
            device_id: self.0.device.clone(),
            subject: claims.sub,
            expires_at: claims.exp,
            deadline: Instant::now() + Duration::from_millis(millis as u64),
            account_closed,
            host_closed: state.retired.clone(),
        };
        grant.is_live().then_some(grant)
    }
}
