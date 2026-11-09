//! Lazy profile-owned MCP pool. Scheduling, permission and recovery stay in the kernel.
//!
//! Unix only, like the native harness that is its only user.
#![cfg(unix)]
mod http;
use async_trait::async_trait;
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleClient,
    model::{
        CallToolRequestParams, CallToolResponse, ClientCapabilities, ClientConfig, Implementation,
        PaginatedRequestParams, ProtocolVersion,
    },
    service::{Peer, RunningService},
    transport::{
        StreamableHttpClientTransport, async_rw::AsyncRwTransport,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use zuse_harness::{Error, ToolContext, ToolOutput, native_tools::Extensions};
fn error(message: &str) -> Error {
    Error::Tool(message.into())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Transport {
    Stdio {
        executable: PathBuf,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env_refs: BTreeMap<String, String>,
    },
    StreamableHttp {
        url: String,
        #[serde(default)]
        token_env: Option<String>,
    },
}
fn protocol() -> String {
    "2025-06-18".into()
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Server {
    pub trusted: bool,
    #[serde(default = "protocol")]
    pub protocol: String,
    #[serde(default)]
    pub read_only_tools: Vec<String>,
    #[serde(rename = "connection")]
    pub transport: Transport,
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    servers: BTreeMap<String, Server>,
}
struct OwnedChild(Option<tokio::process::Child>);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            #[cfg(unix)]
            if let Some(id) = child.id() {
                unsafe {
                    libc::kill(-(id as i32), libc::SIGKILL);
                }
            }
            let _ = child.start_kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }
    }
}
struct Connection {
    peer: Peer<RoleClient>,
    _owner: RunningService<RoleClient, ClientConfig>,
    _child: Option<OwnedChild>,
    admission: tokio::sync::Semaphore,
}
struct Slot {
    connection: Mutex<Option<Arc<Connection>>>,
    last_used: StdMutex<Instant>,
    failed_at: StdMutex<Option<Instant>>,
}
impl Slot {
    fn new() -> Self {
        Self {
            connection: Mutex::new(None),
            last_used: StdMutex::new(Instant::now()),
            failed_at: StdMutex::new(None),
        }
    }
}
struct Lease(Arc<Slot>);
impl Drop for Lease {
    fn drop(&mut self) {
        *self.0.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }
}
pub struct Registry {
    servers: BTreeMap<String, Server>,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
    client: OnceLock<Result<reqwest::Client, ()>>,
    stop: CancellationToken,
}
impl Drop for Registry {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
struct Scope {
    registry: Arc<Registry>,
    checkout: PathBuf,
    policy: Arc<dyn Fn() -> String + Send + Sync>,
}
impl Registry {
    pub fn load(profile: &Path) -> Result<Arc<Self>, Error> {
        let path = profile.join("harness/extensions.json");
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.custom_flags(libc::O_NOFOLLOW);
        }
        let config = match opts.open(&path) {
            Ok(file) => {
                if !file
                    .metadata()
                    .map_err(|_| error("Cannot inspect extension configuration"))?
                    .is_file()
                {
                    return Err(error("Extension configuration must be a regular file"));
                }
                let mut data = vec![];
                file.take(128 * 1024 + 1)
                    .read_to_end(&mut data)
                    .map_err(|_| error("Cannot read extension configuration"))?;
                if data.len() > 128 * 1024 {
                    return Err(error("Extension configuration exceeds 128 KiB"));
                }
                serde_json::from_slice::<Configuration>(&data)
                    .map_err(|_| error("Invalid extension configuration"))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Configuration::default(),
            Err(_) => return Err(error("Cannot open trusted extension configuration")),
        };
        if config.servers.len() > 32 {
            return Err(error("At most 32 external servers may be configured"));
        }
        for (name, server) in &config.servers {
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            {
                return Err(error("Invalid external server ID"));
            }
            if !matches!(
                server.protocol.as_str(),
                "2025-06-18" | "2025-11-25" | "2026-07-28"
            ) {
                return Err(error("Unsupported MCP protocol version"));
            }
            if let Transport::StreamableHttp { url, .. } = &server.transport {
                let url =
                    reqwest::Url::parse(url).map_err(|_| error("Invalid external server URL"))?;
                if !url.username().is_empty()
                    || url.password().is_some()
                    || url.fragment().is_some()
                    || !(url.scheme() == "https"
                        || (url.scheme() == "http"
                            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))))
                {
                    return Err(error(
                        "External server needs HTTPS or loopback HTTP without URL credentials",
                    ));
                }
            }
        }
        let registry = Arc::new(Self {
            servers: config.servers,
            slots: Mutex::new(HashMap::new()),
            client: OnceLock::new(),
            stop: CancellationToken::new(),
        });
        // One janitor per configured backend, never per root. No server starts here.
        if !registry.servers.is_empty() {
            let weak = Arc::downgrade(&registry);
            let stop = registry.stop.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {_=stop.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(30))=>{}}
                    let Some(registry) = weak.upgrade() else {
                        break;
                    };
                    registry.slots.lock().await.retain(|_, slot| {
                        Arc::strong_count(slot) > 1
                            || slot
                                .last_used
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .elapsed()
                                < Duration::from_secs(60)
                    });
                }
            });
        }
        Ok(registry)
    }
    pub fn scope(
        self: &Arc<Self>,
        checkout: PathBuf,
        policy: Arc<dyn Fn() -> String + Send + Sync>,
    ) -> Option<Arc<dyn Extensions>> {
        (!self.servers.is_empty()).then(|| {
            Arc::new(Scope {
                registry: self.clone(),
                checkout,
                policy,
            }) as Arc<dyn Extensions>
        })
    }
    async fn connection(
        &self,
        server: &str,
        checkout: &Path,
        policy: &str,
        context: &ToolContext,
    ) -> Result<(Lease, Arc<Connection>), Error> {
        let config = self
            .servers
            .get(server)
            .filter(|s| s.trusted)
            .ok_or_else(|| error("External server is absent or not trusted"))?;
        let mut environment = BTreeMap::new();
        for name in ["PATH", "HOME", "LANG", "TMPDIR", "SYSTEMROOT"] {
            if let Ok(value) = std::env::var(name) {
                environment.insert(name.to_owned(), value);
            }
        }
        let mut auth = None;
        match &config.transport {
            Transport::Stdio { env_refs, .. } => {
                for (key, reference) in env_refs {
                    if !env_name(key) || !env_name(reference) {
                        return Err(error("Invalid MCP environment reference"));
                    }
                    environment.insert(
                        key.clone(),
                        std::env::var(reference)
                            .map_err(|_| error("MCP environment credential unavailable"))?,
                    );
                }
            }
            Transport::StreamableHttp { token_env, .. } => {
                if let Some(name) = token_env {
                    if !env_name(name) {
                        return Err(error("Invalid MCP credential reference"));
                    }
                    auth = Some(
                        std::env::var(name).map_err(|_| error("MCP authentication required"))?,
                    );
                }
            }
        }
        let key = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    server,
                    config,
                    checkout,
                    policy,
                    context.permission_mode,
                    context.read_only,
                    &environment,
                    &auth
                ))
                .map_err(|_| error("Invalid MCP configuration"))?
            )
        );
        let slot = {
            let mut slots = self.slots.lock().await;
            if !slots.contains_key(&key) && slots.len() >= 8 {
                let idle = slots
                    .iter()
                    .find(|(_, v)| Arc::strong_count(v) == 1)
                    .map(|(k, _)| k.clone());
                if let Some(idle) = idle {
                    slots.remove(&idle);
                } else {
                    return Err(error("External server pool is busy"));
                }
            }
            slots
                .entry(key)
                .or_insert_with(|| Arc::new(Slot::new()))
                .clone()
        };
        let mut current = tokio::select! {biased;_=context.cancel.cancelled()=>return Err(Error::Interrupted),lock=slot.connection.lock()=>lock};
        let connection = match current.as_ref() {
            Some(c) => c.clone(),
            None => {
                if slot
                    .failed_at
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_some_and(|at| at.elapsed() < Duration::from_secs(30))
                {
                    return Err(error("External startup is cooling down after a failure"));
                }
                let started = tokio::select! {biased;_=context.cancel.cancelled()=>return Err(Error::Interrupted),result=tokio::time::timeout(Duration::from_secs(12),self.connect(config,checkout,environment,auth))=>result};
                match started {
                    Ok(Ok(c)) => {
                        let c = Arc::new(c);
                        *current = Some(c.clone());
                        c
                    }
                    _ => {
                        *slot.failed_at.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(Instant::now());
                        return Err(error(
                            "External server initialization failed; check its trusted configuration",
                        ));
                    }
                }
            }
        };
        drop(current);
        *slot.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        Ok((Lease(slot), connection))
    }
    async fn connect(
        &self,
        server: &Server,
        checkout: &Path,
        env: BTreeMap<String, String>,
        auth: Option<String>,
    ) -> Result<Connection, Error> {
        let version = match server.protocol.as_str() {
            "2025-06-18" => ProtocolVersion::V_2025_06_18,
            "2025-11-25" => ProtocolVersion::V_2025_11_25,
            _ => ProtocolVersion::V_2026_07_28,
        };
        let lifecycle = if server.protocol == "2026-07-28" {
            ClientLifecycleMode::Discover {
                preferred_versions: vec![version.clone()],
            }
        } else {
            ClientLifecycleMode::Initialize
        };
        let info = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("zuse", env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(version);
        let (owner, child) = match &server.transport {
            Transport::Stdio {
                executable, args, ..
            } => {
                if args.len() > 128 || args.iter().any(|s| s.len() > 16384) {
                    return Err(error("MCP command exceeds configured bounds"));
                }
                let mut command = tokio::process::Command::new(executable);
                command
                    .args(args)
                    .current_dir(checkout)
                    .env_clear()
                    .envs(env)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::null())
                    .kill_on_drop(true);
                #[cfg(unix)]
                command.process_group(0);
                let mut child = OwnedChild(Some(
                    command
                        .spawn()
                        .map_err(|_| error("Could not spawn external server"))?,
                ));
                let process = child.0.as_mut().unwrap();
                let output = process.stdout.take().unwrap();
                let input = process.stdin.take().unwrap();
                let transport = AsyncRwTransport::<RoleClient, _, _>::new(output, input)
                    .with_max_line_length(1024 * 1024);
                let owner = info
                    .serve_with_lifecycle(transport, lifecycle)
                    .await
                    .map_err(|_| error("External initialization rejected"))?;
                (owner, Some(child))
            }
            Transport::StreamableHttp { url, .. } => {
                let mut configuration = StreamableHttpClientTransportConfig::with_uri(url.as_str())
                    .max_concurrent_requests(4)
                    .max_sse_event_size(1024 * 1024)
                    .reinit_on_expired_session(false);
                configuration.channel_buffer_capacity = 8;
                configuration.retry_config =
                    Arc::new(rmcp::transport::common::client_side_sse::NeverRetry::default());
                if let Some(auth) = auth {
                    configuration = configuration.auth_header(auth);
                }
                let backend = http::Http {
                    client: self
                        .client
                        .get_or_init(|| {
                            reqwest::Client::builder()
                                .redirect(reqwest::redirect::Policy::none())
                                .connect_timeout(Duration::from_secs(10))
                                .timeout(Duration::from_secs(30))
                                .pool_max_idle_per_host(2)
                                .build()
                                .map_err(|_| ())
                        })
                        .as_ref()
                        .map_err(|_| error("External HTTP initialization failed"))?
                        .clone(),
                    uri: url.as_str().into(),
                };
                let transport = StreamableHttpClientTransport::with_client(backend, configuration);
                (
                    info.serve_with_lifecycle(transport, lifecycle)
                        .await
                        .map_err(|_| error("External initialization rejected"))?,
                    None,
                )
            }
        };
        let peer = owner.peer().clone();
        peer.set_response_cache_config(rmcp::service::ClientCacheConfig::disabled())
            .await;
        Ok(Connection {
            peer,
            _owner: owner,
            _child: child,
            admission: tokio::sync::Semaphore::new(4),
        })
    }
}
fn env_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
#[async_trait]
impl Extensions for Scope {
    fn servers(&self) -> Value {
        json!({"servers":self.registry.servers.iter().map(|(id,s)|json!({"id":id,"trusted":s.trusted,"protocol":s.protocol,"connected_on_demand":true})).collect::<Vec<_>>()})
    }
    fn read_only(&self, server: &str, tool: &str) -> bool {
        self.registry
            .servers
            .get(server)
            .is_some_and(|s| s.trusted && s.read_only_tools.iter().any(|t| t == tool))
    }
    async fn tools(
        &self,
        server: &str,
        cursor: Option<String>,
        context: ToolContext,
    ) -> Result<Value, Error> {
        let (_slot, c) = self
            .registry
            .connection(server, &self.checkout, &(self.policy)(), &context)
            .await?;
        let _permit = tokio::select! {biased;_=context.cancel.cancelled()=>return Err(Error::Interrupted),permit=c.admission.acquire()=>permit.map_err(|_|Error::Interrupted)?};
        let page = tokio::select! {biased;_=context.cancel.cancelled()=>return Err(Error::Interrupted),page=tokio::time::timeout(Duration::from_secs(30),c.peer.list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor))))=>page.map_err(|_|error("External discovery timed out"))?.map_err(|_|error("External discovery failed"))?};
        if page.tools.len() > 256 {
            return Err(error("External discovery page exceeds 256 tools"));
        }
        let mut names = std::collections::HashSet::new();
        if page.tools.iter().any(|t| !names.insert(t.name.clone())) {
            return Err(error("External catalog has duplicate names"));
        }
        let encoded = serde_json::to_vec(&page).map_err(|_| error("Invalid external catalog"))?;
        if encoded.len() > 4 * 1024 * 1024 {
            return Err(error("External catalog exceeds 4 MiB"));
        }
        serde_json::from_slice(&encoded).map_err(|_| error("Invalid external catalog"))
    }
    async fn call(
        &self,
        server: &str,
        tool: &str,
        arguments: Value,
        context: ToolContext,
    ) -> Result<ToolOutput, Error> {
        let args = arguments
            .as_object()
            .ok_or_else(|| error("External arguments must be an object"))?
            .clone();
        if serde_json::to_vec(&args)
            .map_err(|_| error("Invalid external arguments"))?
            .len()
            > 1024 * 1024
        {
            return Err(error("External arguments exceed 1 MiB"));
        }
        let (slot, c) = self
            .registry
            .connection(server, &self.checkout, &(self.policy)(), &context)
            .await?;
        let _permit = tokio::select! {biased;_=context.cancel.cancelled()=>return Err(Error::Interrupted),permit=c.admission.acquire()=>permit.map_err(|_|Error::Interrupted)?};
        let result = tokio::select! {biased;_=context.cancel.cancelled()=>return Err(Error::Interrupted),result=tokio::time::timeout(Duration::from_secs(30),c.peer.call_tool_once(CallToolRequestParams::new(tool.to_owned()).with_arguments(args)))=>result};
        let result = match result {
            Ok(Ok(CallToolResponse::Complete(result))) => result,
            _ => {
                // Retire the failed channel without resending the uncertain call.
                // A later, explicitly reviewed retry may establish a new connection.
                let mut current = slot.0.connection.lock().await;
                if current
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, &c))
                {
                    *current = None;
                }
                return Err(Error::paused(
                    "External tool outcome is uncertain. Inspect it before authorizing retry.",
                ));
            }
        };
        let encoded = serde_json::to_string(&result).map_err(|_| {
            Error::paused("External result could not be encoded; inspect before retry.")
        })?;
        if encoded.len() > 4 * 1024 * 1024 {
            return Err(Error::paused(
                "External result exceeds 4 MiB; inspect before retry.",
            ));
        }
        if result.is_error != Some(true) {
            let value = serde_json::to_value(&result).map_err(|_| {
                Error::paused("External result could not be normalized; inspect before retry.")
            })?;
            if value["content"]
                .as_array()
                .is_some_and(|p| p.iter().any(|p| p["type"] == "image"))
            {
                let mut parts = vec![];
                for part in value["content"].as_array().unwrap() {
                    parts.push(match part["type"].as_str(){
                        Some("text")=>json!({"type":"text","text":part["text"]}),
                        Some("image")=>json!({"type":"file","mediaType":part["mimeType"],"data":{"type":"data","data":part["data"]}}),
                        _=>json!({"type":"text","text":part.to_string()}),
                    });
                }
                if let Some(content) = value.get("structuredContent") {
                    parts.push(json!({"type":"text","text":json!({"structuredContent":content}).to_string()}));
                }
                let output = ToolOutput::Mixed {
                    r#type: "mixed".into(),
                    parts,
                };
                if !output.validate() {
                    return Err(Error::paused(
                        "External multimodal result is unsupported; inspect before retry.",
                    ));
                }
                return Ok(output);
            }
        }
        if result.is_error == Some(true) {
            Err(error(&format!(
                "External server reported an error: {encoded}"
            )))
        } else {
            Ok(encoded.into())
        }
    }
}
