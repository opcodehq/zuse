//! Account-scoped stdio MCP servers that run on this computer. Commands and
//! environment values are stored locally and never reach the hosted plugin API.
//! Their tools join the same agent gateway as hosted plugins.

use crate::PluginError;
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, RoleClient,
    model::{
        CallToolRequestParams, CallToolResponse, ClientCapabilities, ClientConfig, Implementation,
        PaginatedRequestParams, ProtocolVersion, Tool,
    },
    service::{Peer, RunningService},
    transport::async_rw::AsyncRwTransport,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use zuse_protocol::{
    plugin_setup::{MAX_LOCAL_SERVERS, validate_local_server},
    plugins::{
        LOCAL_SERVER_PREFIX, PluginConnection, PluginConnectionState, PluginDefinition,
        PluginResponse, PluginToolRequest,
    },
};

const STARTUP: Duration = Duration::from_secs(30);
const REQUEST: Duration = Duration::from_secs(30);
const SEARCH: Duration = Duration::from_secs(15);
const DISCOVERY_CACHE: Duration = Duration::from_secs(30);
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(10);
const MAX_TOOLS: usize = 1000;
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024;

/// Gateway addresses mirror hosted ones so agents see one address space.
fn prefix(id: &str) -> String {
    format!("tools.{id}.user.{id}.")
}
pub fn owns_address(address: &str) -> bool {
    address.starts_with(&format!("tools.{LOCAL_SERVER_PREFIX}"))
}

/// Inherited variables for launched servers. Everything else is cleared.
pub fn base_environment() -> BTreeMap<String, String> {
    [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "TERM",
        "LANG",
        "TMPDIR",
        "SYSTEMROOT",
        "APPDATA",
        "LOCALAPPDATA",
        "USERPROFILE",
    ]
    .into_iter()
    .filter_map(|name| Some((name.to_owned(), std::env::var(name).ok()?)))
    .collect()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Row {
    id: String,
    request_id: String,
    name: String,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    enabled: bool,
    created_at: f64,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    servers: Vec<Row>,
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
    _child: OwnedChild,
    tools: StdMutex<Option<(Instant, Arc<Vec<Tool>>)>>,
}
#[derive(Default)]
struct Slot {
    connection: Mutex<Option<Arc<Connection>>>,
    failed_at: StdMutex<Option<Instant>>,
}

type Environment = Arc<dyn Fn() -> BTreeMap<String, String> + Send + Sync>;

/// One store per signed-in account. Dropping it stops every server it launched.
pub struct LocalServers {
    dir: PathBuf,
    path: PathBuf,
    environment: Environment,
    writes: Mutex<()>,
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

impl LocalServers {
    pub fn new(root: &Path, subject: &str, environment: Environment) -> Self {
        let dir = root.join("local-plugins");
        let path = dir.join(format!("{:x}.json", Sha256::digest(subject.as_bytes())));
        Self {
            dir,
            path,
            environment,
            writes: Mutex::new(()),
            slots: Mutex::new(HashMap::new()),
        }
    }

    /// A malformed file fails closed: nothing is listed, launched or overwritten.
    fn load(&self) -> Result<Vec<Row>, PluginError> {
        let unreadable = || PluginError::Local("Local MCP servers could not be read.");
        let file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err(unreadable()),
        };
        let metadata = file.metadata().map_err(|_| unreadable())?;
        if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
            return Err(unreadable());
        }
        let saved: Saved =
            serde_json::from_reader(std::io::BufReader::new(file)).map_err(|_| unreadable())?;
        if saved.servers.len() > MAX_LOCAL_SERVERS
            || saved.servers.iter().any(|row| {
                !row.id.starts_with(LOCAL_SERVER_PREFIX)
                    || validate_local_server(&row.name, &row.command, &row.args, &row.env).is_err()
            })
        {
            return Err(unreadable());
        }
        Ok(saved.servers)
    }

    fn save(&self, servers: &[Row]) -> Result<(), PluginError> {
        let failed = |_| PluginError::Local("Local MCP servers could not be saved.");
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            builder.mode(0o700);
            builder.create(&self.dir).map_err(failed)?;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))
                .map_err(failed)?;
        }
        #[cfg(not(unix))]
        builder.create(&self.dir).map_err(failed)?;
        let temp = self.dir.join(format!(".{}.tmp", uuid_like()));
        let write = || -> std::io::Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&serde_json::to_vec(&Saved {
                servers: servers.to_vec(),
            })?)?;
            file.sync_all()?;
            std::fs::rename(&temp, &self.path)
        };
        write().map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            failed(error)
        })
    }

    /// Catalog rows and connections for the personal snapshot. Env values never leave.
    pub async fn snapshot(
        &self,
    ) -> Result<(Vec<PluginDefinition>, Vec<PluginConnection>), PluginError> {
        let rows = self.load()?;
        let catalog = rows
            .iter()
            .map(|row| PluginDefinition {
                id: row.id.clone(),
                name: row.name.clone(),
                description: String::new(),
                domain: String::new(),
                category: None,
                featured: false,
                custom: true,
                url: None,
                api_key_header: None,
                command: Some(
                    std::iter::once(row.command.as_str())
                        .chain(row.args.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
            })
            .collect();
        let connections = rows
            .iter()
            .map(|row| PluginConnection {
                id: row.id.clone(),
                plugin_id: row.id.clone(),
                label: row.name.clone(),
                owner: "user".into(),
                state: PluginConnectionState::Connected,
                created_at: row.created_at,
                enabled: row.enabled,
            })
            .collect();
        Ok((catalog, connections))
    }

    /// Verify that the server starts and lists tools before saving it.
    pub async fn add(
        &self,
        request_id: &str,
        name: &str,
        command: &str,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    ) -> Result<PluginResponse, PluginError> {
        let (name, command) = (name.trim(), command.trim());
        validate_local_server(name, command, &args, &env).map_err(PluginError::Local)?;
        if request_id.is_empty() || request_id.len() > 128 {
            return Err(PluginError::Local("Retry adding the local server."));
        }
        let _write = self.writes.lock().await;
        let mut rows = self.load()?;
        if let Some(row) = rows.iter().find(|row| row.request_id == request_id) {
            return Ok(connected(&row.id));
        }
        if rows.len() >= MAX_LOCAL_SERVERS {
            return Err(PluginError::Local(
                "Remove a local server before adding another.",
            ));
        }
        let row = Row {
            id: format!("{LOCAL_SERVER_PREFIX}{}", uuid_like()),
            request_id: request_id.into(),
            name: name.into(),
            command: command.into(),
            args,
            env,
            enabled: true,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as f64)
                .unwrap_or_default(),
        };
        let slot = Arc::new(Slot::default());
        let connection = tokio::time::timeout(STARTUP, self.connect(&row))
            .await
            .map_err(|_| PluginError::Local("The local server did not start in time."))??;
        discover(&connection).await?;
        rows.push(row.clone());
        self.save(&rows)?;
        *slot.connection.lock().await = Some(connection);
        self.slots.lock().await.insert(row.id.clone(), slot);
        Ok(connected(&row.id))
    }

    pub async fn remove(&self, id: &str) -> Result<PluginResponse, PluginError> {
        let _write = self.writes.lock().await;
        let mut rows = self.load()?;
        let before = rows.len();
        rows.retain(|row| row.id != id);
        if rows.len() == before {
            return Err(PluginError::Local(
                "This local server is no longer available.",
            ));
        }
        self.save(&rows)?;
        self.stop(id).await;
        Ok(PluginResponse::Ok)
    }

    pub async fn set_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<PluginResponse, PluginError> {
        let _write = self.writes.lock().await;
        let mut rows = self.load()?;
        let row = rows
            .iter_mut()
            .find(|row| row.id == id)
            .ok_or(PluginError::Local(
                "This local server is no longer available.",
            ))?;
        row.enabled = enabled;
        self.save(&rows)?;
        if !enabled {
            self.stop(id).await;
        }
        Ok(PluginResponse::Ok)
    }

    async fn stop(&self, id: &str) {
        let slot = self.slots.lock().await.remove(id);
        if let Some(slot) = slot {
            slot.connection.lock().await.take();
        }
    }

    /// Atomic replacement lets reads proceed while a new server is being verified.
    async fn enabled(&self) -> Result<Vec<Row>, PluginError> {
        Ok(self.load()?.into_iter().filter(|row| row.enabled).collect())
    }

    /// Serve the shared gateway. List and Search return arrays merged with hosted results.
    pub async fn tools(&self, request: &PluginToolRequest) -> Result<Value, PluginError> {
        let rows = self.enabled().await?;
        match request {
            PluginToolRequest::List => Ok(Value::Array(
                rows.iter()
                    .map(|row| json!({"connectionId":row.id,"pluginId":row.id,"label":row.name}))
                    .collect(),
            )),
            PluginToolRequest::Search { query } => {
                let terms: Vec<String> = query
                    .chars()
                    .take(200)
                    .collect::<String>()
                    .to_lowercase()
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect();
                // Unavailable servers are omitted rather than failing hosted results.
                let searches = rows.iter().map(|row| async {
                    let tools = tokio::time::timeout(SEARCH, async {
                        let connection = self.connection(row).await?;
                        discover(&connection).await
                    })
                    .await
                    .ok()?
                    .ok()?;
                    Some(
                        tools
                            .iter()
                            .filter(|tool| {
                                let text = format!(
                                    "{} {} {}",
                                    row.name,
                                    tool.name,
                                    tool.description.as_deref().unwrap_or_default()
                                )
                                .to_lowercase();
                                terms.iter().all(|term| text.contains(term))
                            })
                            .map(|tool| json!({"address":format!("{}{}",prefix(&row.id),tool.name),"description":tool.description}))
                            .collect::<Vec<_>>(),
                    )
                });
                let found = futures::future::join_all(searches).await;
                Ok(Value::Array(
                    found.into_iter().flatten().flatten().take(100).collect(),
                ))
            }
            PluginToolRequest::Schema { address } | PluginToolRequest::Call { address, .. } => {
                let (row, name) = rows
                    .iter()
                    .find_map(|row| Some((row, address.strip_prefix(&prefix(&row.id))?)))
                    .filter(|(_, name)| !name.is_empty())
                    .ok_or(PluginError::Local(
                        "This local server is unavailable or turned off.",
                    ))?;
                let connection = self.connection(row).await?;
                if let PluginToolRequest::Call { arguments, .. } = request {
                    return self.call(row, connection, name, arguments.clone()).await;
                }
                let tools = discover(&connection).await?;
                let tool =
                    tools
                        .iter()
                        .find(|tool| tool.name == name)
                        .ok_or(PluginError::Local(
                            "This local tool is no longer available.",
                        ))?;
                serde_json::to_value(tool).map_err(|_| PluginError::InvalidResponse)
            }
        }
    }

    async fn call(
        &self,
        row: &Row,
        connection: Arc<Connection>,
        name: &str,
        arguments: serde_json::Map<String, Value>,
    ) -> Result<Value, PluginError> {
        let result = tokio::time::timeout(
            REQUEST,
            connection.peer.call_tool_once(
                CallToolRequestParams::new(name.to_owned()).with_arguments(arguments),
            ),
        )
        .await;
        let Ok(Ok(CallToolResponse::Complete(result))) = result else {
            // Retire the channel without resending; the outcome is uncertain.
            if let Some(slot) = self.slots.lock().await.get(&row.id).cloned() {
                let mut current = slot.connection.lock().await;
                if current
                    .as_ref()
                    .is_some_and(|active| Arc::ptr_eq(active, &connection))
                {
                    *current = None;
                }
            }
            return Err(PluginError::Local(
                "The local tool outcome is uncertain. Check its effects before calling it again.",
            ));
        };
        let value = serde_json::to_value(&result).map_err(|_| PluginError::InvalidResponse)?;
        if value.to_string().len() > MAX_RESULT_BYTES {
            return Err(PluginError::Local("The local tool result exceeds 4 MiB."));
        }
        Ok(value)
    }

    /// Lazily start one process per enabled server and keep it until disabled or removed.
    async fn connection(&self, row: &Row) -> Result<Arc<Connection>, PluginError> {
        let slot = self
            .slots
            .lock()
            .await
            .entry(row.id.clone())
            .or_default()
            .clone();
        let mut current = slot.connection.lock().await;
        if let Some(connection) = current.as_ref().filter(|c| !c.peer.is_transport_closed()) {
            return Ok(connection.clone());
        }
        *current = None;
        if slot
            .failed_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|at| at.elapsed() < RETRY_AFTER_FAILURE)
        {
            return Err(PluginError::Local(
                "The local server failed to start. Try again shortly.",
            ));
        }
        let started = tokio::time::timeout(STARTUP, self.connect(row))
            .await
            .unwrap_or(Err(PluginError::Local(
                "The local server did not start in time.",
            )));
        match started {
            Ok(connection) => {
                *current = Some(connection.clone());
                Ok(connection)
            }
            Err(error) => {
                *slot.failed_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                Err(error)
            }
        }
    }

    async fn connect(&self, row: &Row) -> Result<Arc<Connection>, PluginError> {
        let mut environment = (self.environment)();
        environment.extend(row.env.clone());
        let mut command = tokio::process::Command::new(&row.command);
        command
            .args(&row.args)
            .env_clear()
            .envs(&environment)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if let Some(home) = environment.get("HOME").or(environment.get("USERPROFILE")) {
            command.current_dir(home);
        }
        #[cfg(unix)]
        command.process_group(0);
        let mut child = OwnedChild(Some(command.spawn().map_err(|_| {
            PluginError::Local(
                "The local server command could not be started. Check the command and its PATH.",
            )
        })?));
        let process = child.0.as_mut().ok_or(PluginError::Unavailable)?;
        let (Some(output), Some(input)) = (process.stdout.take(), process.stdin.take()) else {
            return Err(PluginError::Unavailable);
        };
        let transport = AsyncRwTransport::<RoleClient, _, _>::new(output, input)
            .with_max_line_length(4 * 1024 * 1024);
        let owner = ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("zuse", env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(ProtocolVersion::V_2025_06_18)
        .serve_with_lifecycle(transport, ClientLifecycleMode::Initialize)
        .await
        .map_err(|_| PluginError::Local("The local server did not complete MCP initialization."))?;
        let peer = owner.peer().clone();
        peer.set_response_cache_config(rmcp::service::ClientCacheConfig::disabled())
            .await;
        Ok(Arc::new(Connection {
            peer,
            _owner: owner,
            _child: child,
            tools: StdMutex::new(None),
        }))
    }
}

/// Bounded, cached tools/list across pages.
async fn discover(connection: &Connection) -> Result<Arc<Vec<Tool>>, PluginError> {
    if let Some((at, tools)) = &*connection.tools.lock().unwrap_or_else(|e| e.into_inner()) {
        if at.elapsed() < DISCOVERY_CACHE {
            return Ok(tools.clone());
        }
    }
    let failed = || PluginError::Local("The local server could not list its tools.");
    let mut tools = vec![];
    let mut cursor = None;
    for _ in 0..10 {
        let page = tokio::time::timeout(
            REQUEST,
            connection
                .peer
                .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor))),
        )
        .await
        .map_err(|_| failed())?
        .map_err(|_| failed())?;
        tools.extend(page.tools);
        cursor = page.next_cursor;
        if cursor.is_none() || tools.len() >= MAX_TOOLS {
            break;
        }
    }
    if cursor.is_some() && tools.len() < MAX_TOOLS {
        return Err(failed());
    }
    tools.truncate(MAX_TOOLS);
    let tools = Arc::new(tools);
    *connection.tools.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((Instant::now(), tools.clone()));
    Ok(tools)
}

fn connected(id: &str) -> PluginResponse {
    PluginResponse::Attempt {
        id: id.into(),
        connection_id: id.into(),
        state: "connected".into(),
        authorization_url: None,
        expires_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or_default(),
        error: None,
        plugin_id: id.into(),
    }
}

fn uuid_like() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests;
