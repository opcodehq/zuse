//! Shared plugin gateway for hosted plugins and this computer's local stdio
//! servers. Identity and policy are resolved by the engine.
use crate::{
    WorkspaceHost,
    account::{Account, AccountError},
};
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use zuse_plugins::{
    PluginClient, PluginCredential, PluginCredentials, PluginError,
    local::{self, LocalServers},
};
use zuse_protocol::plugins::{
    MovedPlugin, PluginConnectionState, PluginRequest, PluginResponse, PluginReturnTo,
    PluginToolRequest, is_local_server,
};

#[async_trait]
impl PluginCredentials for Account {
    async fn current(&self) -> Result<PluginCredential, PluginError> {
        self.credential()
            .await
            .map(|c| PluginCredential {
                subject: c.subject,
                access_token: c.access_token,
            })
            .map_err(|e| match e {
                AccountError::SignedOut => PluginError::SignedOut,
                AccountError::Changed => PluginError::AccountChanged,
                // Keep the account's own reason (refresh failure, storage, ...).
                error => PluginError::Account(error.to_string()),
            })
    }
}

/// Runtime credentials are supplied by the enrolled guest, independently of WorkOS.
#[async_trait]
pub trait RuntimePlugins: Send + Sync {
    async fn computer_tasks(
        &self,
        _chat: &str,
        _request: &zuse_protocol::computer_tasks::Request,
    ) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("Computer tasks are unavailable on this host.")
    }
    /// A setup chat's image configuration tools.
    async fn machine_setup(
        &self,
        _chat: &str,
        _request: &zuse_protocol::machine_setup::Request,
    ) -> anyhow::Result<serde_json::Value> {
        anyhow::bail!("Machine image setup is unavailable on this host.")
    }
    async fn tools(&self, request: &PluginToolRequest) -> anyhow::Result<serde_json::Value>;
}

#[derive(Clone)]
pub struct Plugins {
    account: Account,
    /// Hosted plugins live in the cloud API, the same service cloud chats ask
    /// for their tools, so a plugin connected here reaches every chat.
    api_origin: String,
    runtime: Arc<std::sync::RwLock<Option<Arc<dyn RuntimePlugins>>>>,
    workspace: WorkspaceHost,
    bindings: Arc<Mutex<Connection>>,
    callback: Arc<tokio::sync::Mutex<Option<crate::plugin_callback::PluginCallback>>>,
    client: Arc<tokio::sync::Mutex<Option<(String, PluginClient)>>>,
    /// `None` where launching local commands is not allowed, such as hosted runtimes.
    local_root: Option<PathBuf>,
    computer_bridge: Arc<crate::computer_bridge::Connector>,
    local: Arc<tokio::sync::Mutex<Option<(String, Arc<LocalServers>)>>>,
    /// What each workspace had on the previous plugin service (the account
    /// API), read once per run so the Plugins page can offer Reconnect.
    previous: Arc<tokio::sync::Mutex<BTreeMap<(String, String), Vec<MovedPlugin>>>>,
}
impl Plugins {
    pub fn open(
        root: &Path,
        account: Account,
        api_origin: String,
        workspace: WorkspaceHost,
        local_servers: bool,
    ) -> anyhow::Result<Self> {
        let db = open_bindings(root)?;
        Ok(Self {
            account,
            api_origin,
            runtime: Arc::new(std::sync::RwLock::new(None)),
            workspace,
            bindings: Arc::new(Mutex::new(db)),
            callback: Arc::new(tokio::sync::Mutex::new(None)),
            client: Arc::new(tokio::sync::Mutex::new(None)),
            computer_bridge: Arc::default(),
            local_root: local_servers.then(|| root.to_path_buf()),
            local: Arc::new(tokio::sync::Mutex::new(None)),
            previous: Arc::default(),
        })
    }
    /// One store per account; switching accounts stops the previous account's servers.
    async fn local(&self, subject: &str) -> Result<Arc<LocalServers>, PluginError> {
        let root = self.local_root.as_ref().ok_or(PluginError::Local(
            "Local MCP servers are not available on this computer.",
        ))?;
        let mut slot = self.local.lock().await;
        if let Some((owner, store)) = &*slot {
            if owner == subject {
                return Ok(store.clone());
            }
        }
        let store = Arc::new(LocalServers::new(
            root,
            subject,
            Arc::new(local_environment),
        ));
        *slot = Some((subject.to_owned(), store.clone()));
        Ok(store)
    }
    /// Local management never reaches the hosted API and only targets Personal.
    async fn local_request(
        &self,
        request: &PluginRequest,
        subject: &str,
    ) -> Result<Option<PluginResponse>, PluginError> {
        if !targets_local(request) {
            return Ok(None);
        }
        if request.tenant_id() != Some(format!("personal:{subject}").as_str()) {
            return Err(PluginError::Local(
                "Local servers belong to your personal workspace.",
            ));
        }
        let store = self.local(subject).await?;
        Ok(Some(match request {
            PluginRequest::AddLocalServer {
                request_id,
                name,
                url: Some(url),
                headers,
                ..
            } => {
                store
                    .add_url(request_id, name, url, headers.clone())
                    .await?
            }
            PluginRequest::AddLocalServer {
                request_id,
                name,
                command,
                args,
                env,
                ..
            } => {
                store
                    .add(request_id, name, command, args.clone(), env.clone())
                    .await?
            }
            PluginRequest::RemoveServer { plugin_id: id, .. }
            | PluginRequest::Disconnect {
                connection_id: id, ..
            } => store.remove(id).await?,
            PluginRequest::SetEnabled {
                connection_id,
                enabled,
                ..
            } => store.set_enabled(connection_id, *enabled).await?,
            _ => return Ok(None),
        }))
    }
    pub async fn start_computer_bridge(&self, label: &str) -> anyhow::Result<String> {
        let root = self
            .local_root
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Start this connector on a native computer"))?;
        self.computer_bridge
            .start(root, self.account.clone(), label)
            .await
    }
    pub async fn stop_computer_bridge(&self) -> anyhow::Result<()> {
        match &self.local_root {
            Some(root) => self.computer_bridge.stop(root).await,
            None => Ok(()),
        }
    }
    /// Engine shutdown: stop running commands; the connector resumes next start.
    pub async fn shutdown_computer_bridge(&self) {
        self.computer_bridge.shutdown().await;
    }
    /// Resume the connector this computer enabled before the engine restarted.
    pub async fn resume_computer_bridge(&self) {
        if let Some(root) = &self.local_root {
            self.computer_bridge
                .resume(root, self.account.clone())
                .await;
        }
    }
    /// Make this computer the chat's primary when it is enabled for the
    /// account; `None` otherwise.
    pub async fn claim_computer_primary(
        &self,
        account_id: &str,
        workspace: &str,
        chat: &str,
        command: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        let Some(root) = &self.local_root else {
            return Ok(None);
        };
        self.computer_bridge
            .claim_primary(root, &self.account, account_id, workspace, chat, command)
            .await
    }
    /// Report the folder a chat is synced into on this computer.
    pub async fn report_synced_checkout(
        &self,
        account_id: &str,
        workspace: &str,
        chat: &str,
    ) -> anyhow::Result<()> {
        let Some(root) = &self.local_root else {
            return Ok(());
        };
        self.computer_bridge
            .report_synced_checkout(root, &self.account, account_id, workspace, chat)
            .await
    }
    pub fn computer_bridge_status(&self) -> crate::computer_bridge::BridgeStatus {
        self.computer_bridge.status()
    }
    pub async fn computer_tasks(
        &self,
        chat_id: &str,
        request: &zuse_protocol::computer_tasks::Request,
    ) -> anyhow::Result<serde_json::Value> {
        let chat = self
            .workspace
            .chat(chat_id)?
            .ok_or_else(|| anyhow::anyhow!("Unknown chat"))?;
        anyhow::ensure!(
            self.workspace.is_host(chat_id) && !chat.archived,
            "Chat is unavailable"
        );
        if matches!(
            request,
            zuse_protocol::computer_tasks::Request::Run { .. }
                | zuse_protocol::computer_tasks::Request::Command { .. }
        ) {
            anyhow::ensure!(
                chat.config.as_ref().is_some_and(|c| !c.plan),
                "Leave plan mode to run computer tasks"
            );
        }
        let runtime = self
            .runtime
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Computer tasks require a cloud chat"))?;
        runtime.computer_tasks(chat_id, request).await
    }
    pub async fn machine_setup(
        &self,
        chat_id: &str,
        request: &zuse_protocol::machine_setup::Request,
    ) -> anyhow::Result<serde_json::Value> {
        let chat = self
            .workspace
            .chat(chat_id)?
            .ok_or_else(|| anyhow::anyhow!("Unknown chat"))?;
        anyhow::ensure!(
            self.workspace.is_host(chat_id) && !chat.archived,
            "Chat is unavailable"
        );
        let runtime = self
            .runtime
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Machine image setup requires a cloud setup chat"))?;
        runtime.machine_setup(chat_id, request).await
    }

    pub fn set_runtime(&self, runtime: Arc<dyn RuntimePlugins>) {
        *self.runtime.write().unwrap_or_else(|e| e.into_inner()) = Some(runtime);
    }
    async fn client(&self, subject: &str) -> Result<PluginClient, PluginError> {
        let mut slot = self.client.lock().await;
        if let Some((owner, client)) = &*slot {
            if owner == subject {
                return Ok(client.clone());
            }
        }
        let client = PluginClient::new(
            &self.api_origin,
            Arc::new(BoundAccount {
                account: self.account.clone(),
                subject: subject.to_owned(),
            }),
        )
        .await?;
        *slot = Some((subject.to_owned(), client.clone()));
        Ok(client)
    }
    pub async fn request(&self, request: &PluginRequest) -> Result<PluginResponse, PluginError> {
        let cancel = self.account.session_cancellation();
        let credential = PluginCredentials::current(&self.account).await?;
        if let Some(response) = self.local_request(request, &credential.subject).await? {
            return Ok(response);
        }
        let client = self.client(&credential.subject).await?;
        let mut request = request.clone();
        if let PluginRequest::List { tenant_id } = &mut request {
            if tenant_id.is_none() {
                *tenant_id = Some(selected(
                    &self.bindings.lock().unwrap_or_else(|e| e.into_inner()),
                    &credential.subject,
                )?);
            }
        }
        if let PluginRequest::Connect { return_to, .. }
        | PluginRequest::AddServer { return_to, .. } = &mut request
        {
            if matches!(return_to, Some(PluginReturnTo::Desktop { .. })) {
                let mut callback = self.callback.lock().await;
                if callback.as_ref().is_none_or(|c| c.cancel.is_cancelled()) {
                    *callback = Some(
                        crate::plugin_callback::PluginCallback::start(
                            client.clone(),
                            cancel.clone(),
                        )
                        .await
                        .map_err(|_| PluginError::Unavailable)?,
                    );
                }
                *return_to = Some(PluginReturnTo::Desktop {
                    port: callback.as_ref().unwrap().port,
                });
            }
        }
        let mut response = client.request(&request, &cancel).await?;
        if let PluginResponse::Snapshot {
            tenants, tenant_id, ..
        } = &mut response
        {
            let mut available = match client.workspaces(&cancel).await {
                Ok(available) => available,
                Err(
                    error @ (PluginError::AccountChanged
                    | PluginError::SignedOut
                    | PluginError::Cancelled),
                ) => return Err(error),
                // Membership discovery is optional. Never replace a valid selected
                // snapshot with Personal when this ancillary endpoint is unavailable.
                Err(_) => tenants.clone(),
            };
            // Keep the authorized selected snapshot visible during API version skew.
            if !available.iter().any(|tenant| &tenant.id == tenant_id) {
                available.extend(
                    tenants
                        .iter()
                        .filter(|tenant| &tenant.id == tenant_id)
                        .cloned(),
                );
            }
            *tenants = available;
        }
        if let PluginResponse::Snapshot {
            tenant_id,
            catalog,
            connections,
            moved,
            ..
        } = &mut response
        {
            let previous = self.previous(&credential.subject, tenant_id).await;
            *moved = not_reconnected(previous, catalog, connections);
            if *tenant_id == format!("personal:{}", credential.subject) && self.local_root.is_some()
            {
                // An unreadable local file hides local rows; hosted plugins stay usable.
                match self.local(&credential.subject).await?.snapshot().await {
                    Ok((local_catalog, local_connections)) => {
                        catalog.splice(0..0, local_catalog);
                        connections.splice(0..0, local_connections);
                    }
                    Err(error) => tracing::warn!(%error, "local plugin servers unavailable"),
                }
            }
        }
        Ok(response)
    }
    /// Hosted plugins used to live on the account API. Its list (names
    /// only, never credentials) is read once per workspace and run, so
    /// plugins connected there can be connected again here. Unreachable or
    /// the same service: nothing to offer.
    async fn previous(&self, subject: &str, tenant: &str) -> Vec<MovedPlugin> {
        let legacy = self.account.api_url();
        if same_origin(legacy, &self.api_origin) {
            return Vec::new();
        }
        let key = (subject.to_owned(), tenant.to_owned());
        let mut cache = self.previous.lock().await;
        if let Some(previous) = cache.get(&key) {
            return previous.clone();
        }
        let read = async {
            let client = PluginClient::new(
                legacy,
                Arc::new(BoundAccount {
                    account: self.account.clone(),
                    subject: subject.to_owned(),
                }),
            )
            .await?;
            let request = PluginRequest::List {
                tenant_id: Some(tenant.to_owned()),
            };
            client
                .request(&request, &self.account.session_cancellation())
                .await
        };
        let previous = match tokio::time::timeout(PREVIOUS_LIST_DEADLINE, read).await {
            Ok(Ok(PluginResponse::Snapshot {
                tenant_id,
                catalog,
                connections,
                ..
            })) if tenant_id == tenant => previous_plugins(&catalog, &connections),
            Ok(Ok(_)) => Vec::new(),
            Ok(Err(error)) => {
                tracing::info!(%error, "previous plugin list unavailable");
                Vec::new()
            }
            Err(_) => Vec::new(),
        };
        cache.insert(key, previous.clone());
        previous
    }
    pub async fn select_workspace(
        &self,
        request: &PluginRequest,
    ) -> Result<PluginResponse, PluginError> {
        let PluginRequest::List {
            tenant_id: Some(tenant),
        } = request
        else {
            return Err(PluginError::InvalidWorkspace);
        };
        let cancel = self.account.session_cancellation();
        let credential = PluginCredentials::current(&self.account).await?;
        // Authorize and validate the returned workspace before saving a preference.
        let response = self.request(request).await?;
        let PluginResponse::Snapshot { tenant_id, .. } = &response else {
            return Err(PluginError::InvalidResponse);
        };
        if tenant_id != tenant || cancel.is_cancelled() {
            return Err(PluginError::AccountChanged);
        }
        let db = self.bindings.lock().unwrap_or_else(|e| e.into_inner());
        db.execute("INSERT INTO preferences(subject,tenant) VALUES (?1,?2) ON CONFLICT(subject) DO UPDATE SET tenant=excluded.tenant", params![credential.subject,tenant]).map_err(|_| PluginError::Unavailable)?;
        Ok(response)
    }
    pub async fn tools(
        &self,
        chat_id: &str,
        request: &PluginToolRequest,
    ) -> anyhow::Result<serde_json::Value> {
        let chat = self
            .workspace
            .chat(chat_id)?
            .ok_or_else(|| anyhow::anyhow!("Unknown plugin session."))?;
        if !self.workspace.is_host(chat_id) {
            anyhow::bail!("Plugin requests must run on the session's host.");
        }
        // External calls follow the session's tool policy. Discovery remains available
        // in plan-mode sessions; callers cannot supply a permission override.
        let permitted = chat.config.as_ref().is_some_and(|c| !c.plan);
        if request.invokes_external_tool() && !permitted {
            return Err(PluginError::PermissionRequired.into());
        }
        let runtime = self
            .runtime
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(runtime) = runtime {
            return runtime.tools(request).await;
        }
        let cancel = self.account.session_cancellation();
        let credential = self.account.credential().await?;
        let tenant = {
            let db = self.bindings.lock().unwrap_or_else(|e| e.into_inner());
            bind(&db, chat_id, &credential.subject)?
        };
        // Local servers join chats bound to this account's Personal workspace.
        let local =
            if tenant == format!("personal:{}", credential.subject) && self.local_root.is_some() {
                Some(self.local(&credential.subject).await?)
            } else {
                None
            };
        let hosted = async {
            self.client(&credential.subject)
                .await?
                .tools_in_tenant(&tenant, request, permitted, &cancel)
                .await
        };
        match request {
            PluginToolRequest::Schema { address } | PluginToolRequest::Call { address, .. }
                if local::owns_address(address) =>
            {
                let local = local.ok_or(PluginError::Local(
                    "This local server is not available in this session.",
                ))?;
                Ok(local.tools(request).await?)
            }
            PluginToolRequest::List | PluginToolRequest::Search { .. } if local.is_some() => {
                let local = local.as_ref().map(|local| local.tools(request));
                let (hosted, local) = tokio::join!(hosted, async {
                    match local {
                        Some(local) => Some(local.await),
                        None => None,
                    }
                });
                Ok(merge(hosted, local)?)
            }
            _ => Ok(hosted.await?),
        }
    }
}

/// Reading the previous service never holds up the Plugins page for long.
const PREVIOUS_LIST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

fn same_origin(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// One entry per plugin that had a usable or signed-out account there.
fn previous_plugins(
    catalog: &[zuse_protocol::plugins::PluginDefinition],
    connections: &[zuse_protocol::plugins::PluginConnection],
) -> Vec<MovedPlugin> {
    let mut previous: Vec<MovedPlugin> = Vec::new();
    for connection in connections {
        if matches!(connection.state, PluginConnectionState::Connecting)
            || is_local_server(&connection.plugin_id)
            || previous
                .iter()
                .any(|plugin| plugin.plugin_id == connection.plugin_id)
        {
            continue;
        }
        let definition = catalog
            .iter()
            .find(|definition| definition.id == connection.plugin_id);
        previous.push(MovedPlugin {
            plugin_id: connection.plugin_id.clone(),
            name: definition
                .map(|definition| definition.name.clone())
                .unwrap_or_else(|| connection.label.clone()),
            custom: definition.is_some_and(|definition| definition.custom),
        });
    }
    previous
}

/// Previous plugins with no account here yet. Catalog plugins match by id;
/// servers added by URL get new ids, so they match by name.
fn not_reconnected(
    previous: Vec<MovedPlugin>,
    catalog: &[zuse_protocol::plugins::PluginDefinition],
    connections: &[zuse_protocol::plugins::PluginConnection],
) -> Vec<MovedPlugin> {
    previous
        .into_iter()
        .filter(|plugin| {
            !connections.iter().any(|connection| {
                connection.plugin_id == plugin.plugin_id
                    || (plugin.custom
                        && catalog.iter().any(|definition| {
                            definition.custom
                                && definition.id == connection.plugin_id
                                && definition.name.eq_ignore_ascii_case(&plugin.name)
                        }))
            })
        })
        .collect()
}

fn targets_local(request: &PluginRequest) -> bool {
    match request {
        PluginRequest::AddLocalServer { .. } => true,
        PluginRequest::RemoveServer { plugin_id: id, .. }
        | PluginRequest::Disconnect {
            connection_id: id, ..
        }
        | PluginRequest::SetEnabled {
            connection_id: id, ..
        } => is_local_server(id),
        _ => false,
    }
}

/// Discovery succeeds when either source answers; an error surfaces only when
/// neither contributed a result.
fn merge(
    hosted: Result<serde_json::Value, PluginError>,
    local: Option<Result<serde_json::Value, PluginError>>,
) -> Result<serde_json::Value, PluginError> {
    let mut results = vec![];
    for value in [
        hosted.as_ref().ok(),
        local.as_ref().and_then(|r| r.as_ref().ok()),
    ]
    .into_iter()
    .flatten()
    {
        results.extend(value.as_array().cloned().unwrap_or_default());
    }
    if results.is_empty() {
        if let Err(error) = hosted {
            return Err(error);
        }
        if let Some(Err(error)) = local {
            return Err(error);
        }
    }
    Ok(serde_json::Value::Array(results))
}

/// Local servers see the user's login-shell PATH, like other launched tools.
fn local_environment() -> BTreeMap<String, String> {
    let mut environment = local::base_environment();
    let mut paths: Vec<PathBuf> = environment
        .get("PATH")
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    if let Some(shell) = zuse_providers::shell_env::login_shell_path() {
        paths.extend(std::env::split_paths(shell));
    }
    let mut seen = std::collections::HashSet::new();
    paths.retain(|path| !path.as_os_str().is_empty() && seen.insert(path.clone()));
    if let Some(path) = std::env::join_paths(paths)
        .ok()
        .and_then(|path| path.into_string().ok())
    {
        environment.insert("PATH".into(), path);
    }
    environment
}

fn open_bindings(root: &Path) -> anyhow::Result<Connection> {
    let db = Connection::open(root.join("plugin-sessions.sqlite"))?;
    db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS bindings(chat_id TEXT PRIMARY KEY, subject TEXT NOT NULL);")?;
    let has_tenant = db
        .prepare("PRAGMA table_info(bindings)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|name| name == "tenant");
    if !has_tenant {
        db.execute_batch("ALTER TABLE bindings ADD COLUMN tenant TEXT;")?;
    }
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS preferences(subject TEXT PRIMARY KEY, tenant TEXT NOT NULL);",
    )?;
    Ok(db)
}
fn selected(db: &Connection, subject: &str) -> Result<String, PluginError> {
    db.query_row(
        "SELECT tenant FROM preferences WHERE subject=?1",
        [subject],
        |row| row.get(0),
    )
    .optional()
    .map(|tenant| tenant.unwrap_or_else(|| format!("personal:{subject}")))
    .map_err(|_| PluginError::Unavailable)
}
fn bind(db: &Connection, chat_id: &str, subject: &str) -> anyhow::Result<String> {
    let selected = selected(db, subject)?;
    // The transaction also fences independent database readers during migration.
    let tx = db.unchecked_transaction()?;
    tx.execute(
        "INSERT OR IGNORE INTO bindings(chat_id,subject,tenant) VALUES (?1,?2,?3)",
        params![chat_id, subject, selected],
    )?;
    let (owner, tenant): (String, Option<String>) = tx.query_row(
        "SELECT subject,tenant FROM bindings WHERE chat_id=?1",
        [chat_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if owner != subject {
        return Err(PluginError::AccountChanged.into());
    }
    // Historical sessions used Personal. Never relabel one after a workspace switch.
    let tenant = tenant.unwrap_or_else(|| format!("personal:{owner}"));
    tx.execute(
        "UPDATE bindings SET tenant=?2 WHERE chat_id=?1 AND tenant IS NULL",
        params![chat_id, tenant],
    )?;
    tx.commit()?;
    Ok(tenant)
}
struct BoundAccount {
    account: Account,
    subject: String,
}
#[async_trait]
impl PluginCredentials for BoundAccount {
    async fn current(&self) -> Result<PluginCredential, PluginError> {
        let current = PluginCredentials::current(&self.account).await?;
        if current.subject != self.subject {
            return Err(PluginError::AccountChanged);
        }
        Ok(current)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct RuntimeFixture(std::sync::atomic::AtomicUsize);
    #[async_trait]
    impl RuntimePlugins for RuntimeFixture {
        async fn tools(&self, _: &PluginToolRequest) -> anyhow::Result<serde_json::Value> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(serde_json::json!([]))
        }
    }
    #[tokio::test]
    async fn runtime_plugins_work_without_workos_and_preserve_session_permissions() {
        let root = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(root.path()).unwrap(),
            Arc::new(crate::HarnessRegistry::new()),
            zuse_protocol::HarnessId::Mock,
            None,
        )
        .unwrap();
        assert!(matches!(
            core.account.credential().await,
            Err(AccountError::SignedOut)
        ));
        let mut config: zuse_protocol::ChatConfig = serde_json::from_value(
            serde_json::json!({"harness":"mock","model":"fixture","sandbox":"read-only","plan":true}),
        )
        .unwrap();
        core.workspace
            .create_chat(
                "runtime-chat",
                None,
                Some(&core.device_id),
                Some(config.clone()),
                Some(root.path().to_string_lossy().into_owned()),
            )
            .unwrap();
        let runtime = Arc::new(RuntimeFixture(std::sync::atomic::AtomicUsize::new(0)));
        core.plugins.set_runtime(runtime.clone());
        core.plugins
            .tools("runtime-chat", &PluginToolRequest::List)
            .await
            .unwrap();
        let call = PluginToolRequest::Call {
            address: "tools.fixture.user.c.echo".into(),
            arguments: Default::default(),
        };
        assert!(core.plugins.tools("runtime-chat", &call).await.is_err());
        assert_eq!(runtime.0.load(std::sync::atomic::Ordering::SeqCst), 1);
        config.plan = false;
        core.workspace
            .set_chat_config("runtime-chat", &config)
            .unwrap();
        core.plugins.tools("runtime-chat", &call).await.unwrap();
        assert_eq!(runtime.0.load(std::sync::atomic::Ordering::SeqCst), 2);
        core.shutdown().await;
    }
    #[test]
    fn session_binding_survives_restart_and_rejects_another_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin-sessions.sqlite");
        let db = open_bindings(dir.path()).unwrap();
        bind(&db, "chat-a", "user-a").unwrap();
        bind(&db, "chat-b", "user-a").unwrap();
        drop(db);
        let db = Connection::open(path).unwrap();
        bind(&db, "chat-a", "user-a").unwrap();
        assert!(bind(&db, "chat-a", "user-b").is_err());
        bind(&db, "chat-c", "user-b").unwrap();
        assert!(bind(&db, "chat-b", "user-b").is_err());
    }
    #[test]
    fn existing_sessions_keep_workspace_and_legacy_sessions_stay_personal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin-sessions.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE bindings(chat_id TEXT PRIMARY KEY,subject TEXT NOT NULL); INSERT INTO bindings(chat_id,subject) VALUES ('legacy','user');").unwrap();
        drop(db);
        let db = open_bindings(dir.path()).unwrap();
        assert_eq!(bind(&db, "personal-chat", "user").unwrap(), "personal:user");
        db.execute(
            "INSERT INTO preferences VALUES (?1,?2)",
            params!["user", "organization:team"],
        )
        .unwrap();
        assert_eq!(bind(&db, "legacy", "user").unwrap(), "personal:user");
        assert_eq!(bind(&db, "team-chat", "user").unwrap(), "organization:team");
        assert_eq!(bind(&db, "personal-chat", "user").unwrap(), "personal:user");
        db.execute(
            "UPDATE preferences SET tenant='organization:other' WHERE subject='user'",
            [],
        )
        .unwrap();
        assert_eq!(bind(&db, "team-chat", "user").unwrap(), "organization:team");
        drop(db);
        let db = Connection::open(path).unwrap();
        assert_eq!(bind(&db, "team-chat", "user").unwrap(), "organization:team");
        assert_eq!(
            bind(&db, "next-chat", "user").unwrap(),
            "organization:other"
        );
        assert_eq!(
            bind(&db, "other-account", "other").unwrap(),
            "personal:other"
        );
        assert!(bind(&db, "team-chat", "other").is_err());
    }
    #[test]
    fn local_management_is_routed_by_identity_and_never_by_hosted_actions() {
        let local = |id: &str| PluginRequest::SetEnabled {
            tenant_id: "personal:user".into(),
            connection_id: id.into(),
            enabled: false,
        };
        assert!(targets_local(&local("local-1")));
        assert!(!targets_local(&local("hosted-1")));
        assert!(targets_local(&PluginRequest::RemoveServer {
            tenant_id: "personal:user".into(),
            plugin_id: "local-1".into(),
        }));
        assert!(targets_local(&PluginRequest::Disconnect {
            tenant_id: "personal:user".into(),
            connection_id: "local-1".into(),
        }));
        assert!(!targets_local(&PluginRequest::List { tenant_id: None }));
        assert!(!targets_local(&PluginRequest::Connect {
            tenant_id: "personal:user".into(),
            plugin_id: "local-1".into(),
            label: "Files".into(),
            request_id: "r".into(),
            return_to: None,
            api_key: None,
        }));
    }
    #[test]
    fn discovery_merges_sources_and_reports_errors_only_without_results() {
        let hosted = || Ok(serde_json::json!([{"address":"mail/send"}]));
        let local = || Ok(serde_json::json!([{"address":"tools.local-1.user.local-1.echo"}]));
        assert_eq!(
            merge(hosted(), Some(local()))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            merge(Err(PluginError::Unavailable), Some(local())).unwrap()[0]["address"],
            "tools.local-1.user.local-1.echo"
        );
        assert_eq!(
            merge(hosted(), Some(Err(PluginError::Unavailable))).unwrap()[0]["address"],
            "mail/send"
        );
        assert!(matches!(
            merge(Err(PluginError::SignedOut), Some(Ok(serde_json::json!([])))),
            Err(PluginError::SignedOut)
        ));
        assert!(matches!(
            merge(
                Ok(serde_json::json!([])),
                Some(Err(PluginError::Local("x")))
            ),
            Err(PluginError::Local(_))
        ));
        assert!(
            merge(Ok(serde_json::json!([])), None)
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    /// The hosted list as the API sends it: ~1200 catalog entries (about 400 KB)
    /// and a hosted connection, the shape the Rust cloud serializes.
    #[cfg(feature = "test-support")]
    fn hosted_snapshot(tenant: &str) -> serde_json::Value {
        let catalog: Vec<_> = (0..1232)
            .map(|i| {
                serde_json::json!({
                    "id": format!("plugin-{i}"),
                    "name": format!("Plugin {i}"),
                    "description": "Find and update issues, projects, and documents across the workspace. ".repeat(3),
                    "domain": format!("plugin-{i}.example"),
                    "category": if i % 3 == 0 { serde_json::Value::Null } else { serde_json::json!("productivity") },
                    "featured": i < 12,
                    "custom": false,
                    "url": null,
                    "apiKeyHeader": null,
                })
            })
            .collect();
        serde_json::json!({
            "kind": "snapshot",
            "canManage": true,
            "tenants": [{"id": tenant, "kind": "personal", "name": "Personal"}],
            "tenantId": tenant,
            "endpoint": format!("https://api.example/v1/plugins/{tenant}/mcp"),
            "catalog": catalog,
            "connections": [{"id":"conn-1","pluginId":"plugin-1","label":"Plugin 1","owner":"user","state":"connected","createdAt":1763380000000_i64,"enabled":true}],
        })
    }
    #[cfg(feature = "test-support")]
    async fn hosted_fixture(
        list: axum::response::Response,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use axum::{
            Router,
            routing::{get, post},
        };
        let list = Arc::new(Mutex::new(Some(list)));
        let app = Router::new()
            .route(
                "/v1/plugins",
                post(move || {
                    let list = list.clone();
                    async move { list.lock().unwrap().take().expect("one list request") }
                }),
            )
            .route(
                "/v1/organizations",
                get(|| async { axum::Json(serde_json::json!([])) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (origin, server)
    }
    #[cfg(feature = "test-support")]
    async fn hosted_plugins(root: &Path, origin: String) -> (crate::EngineCore, Plugins) {
        hosted_plugins_with_account(root, origin, "http://127.0.0.1:1".into()).await
    }
    #[cfg(feature = "test-support")]
    async fn hosted_plugins_with_account(
        root: &Path,
        origin: String,
        account_origin: String,
    ) -> (crate::EngineCore, Plugins) {
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(root).unwrap(),
            Arc::new(crate::HarnessRegistry::new()),
            zuse_protocol::HarnessId::Mock,
            None,
        )
        .unwrap();
        // The account service is elsewhere: plugin traffic must never use it.
        let account = Account::fixture_signed_in(root, account_origin, "user_a");
        let plugins = Plugins::open(root, account, origin, core.workspace.clone(), true).unwrap();
        (core, plugins)
    }
    /// The full desktop read: engine list over HTTP, the RPC value it returns,
    /// and the shared view state the plugins page renders from.
    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn hosted_list_reaches_the_plugins_page_state_with_the_full_catalog() {
        use axum::response::IntoResponse;
        let root = tempfile::tempdir().unwrap();
        let body = hosted_snapshot("personal:user_a");
        assert!(serde_json::to_vec(&body).unwrap().len() > 300_000);
        let (origin, server) = hosted_fixture(axum::Json(body).into_response()).await;
        let (core, plugins) = hosted_plugins(root.path(), origin).await;
        let response = plugins
            .request(&PluginRequest::List { tenant_id: None })
            .await
            .unwrap();
        let mut page = zuse_protocol::plugin_state::PluginState::default();
        let request = page.begin(serde_json::json!({"action":"list"})).unwrap();
        assert_eq!(request, serde_json::json!({"action":"list"}));
        let rpc_value = serde_json::to_value(&response).unwrap();
        assert_eq!(page.apply(&rpc_value).unwrap(), None);
        assert_eq!(page.snapshot["catalog"].as_array().unwrap().len(), 1232);
        assert_eq!(page.snapshot["connections"][0]["id"], "conn-1");
        assert_eq!(page.tenant(), Some("personal:user_a"));
        assert!(page.load_error.is_none());
        server.abort();
        core.shutdown().await;
    }
    /// A failed read names its cause instead of a generic "unavailable".
    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn hosted_list_failures_keep_their_reason() {
        use axum::{http::StatusCode, response::IntoResponse};
        let cases = [
            (
                (
                    StatusCode::OK,
                    r#"{"kind":"snapshot","tenantId":"personal:user_a"}"#,
                )
                    .into_response(),
                "missing field",
            ),
            (
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    r#"{"error":"plugins_unavailable"}"#,
                )
                    .into_response(),
                "plugins_unavailable",
            ),
        ];
        for (reply, reason) in cases {
            let root = tempfile::tempdir().unwrap();
            let (origin, server) = hosted_fixture(reply).await;
            let (core, plugins) = hosted_plugins(root.path(), origin).await;
            let error = plugins
                .request(&PluginRequest::List { tenant_id: None })
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(reason), "{error}");
            let mut page = zuse_protocol::plugin_state::PluginState::default();
            page.begin(serde_json::json!({"action":"list"})).unwrap();
            assert!(page.failed_with(error.clone()).is_none());
            assert_eq!(
                page.unavailable_message(),
                format!("Plugins are unavailable: {error}")
            );
            server.abort();
            core.shutdown().await;
        }
    }
    /// Cloud chats read their plugins from the cloud API. The desktop must
    /// connect plugins there too, not on the separate account service, or a
    /// plugin shows Connected while no chat can use it.
    #[tokio::test]
    async fn hosted_plugins_use_the_cloud_api_cloud_chats_read() {
        let root = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble_with_profile(
            crate::EngineProfile::local(root.path()).unwrap(),
            Arc::new(crate::HarnessRegistry::new()),
            zuse_protocol::HarnessId::Mock,
            None,
        )
        .unwrap();
        assert_eq!(
            core.plugins.api_origin,
            crate::cloud::configured_api_origin()
        );
        let defaults = crate::api_origins::resolve(None, None);
        assert_ne!(defaults.general, defaults.cloud);
        core.shutdown().await;
    }
    /// A plugin connected while a chat is running reaches that chat on its
    /// next discovery: the gateway reads connections on every call.
    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn a_plugin_connected_mid_chat_is_listed_on_the_next_call() {
        use axum::{Router, routing::post};
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = connected.clone();
        let app = Router::new().route(
            "/v1/plugins/tools",
            post(move || {
                let connected = seen.load(std::sync::atomic::Ordering::SeqCst);
                async move {
                    axum::Json(if connected {
                        serde_json::json!([{"connectionId":"conn-1","pluginId":"posthog","label":"PostHog"}])
                    } else {
                        serde_json::json!([])
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let root = tempfile::tempdir().unwrap();
        let (core, plugins) = hosted_plugins(root.path(), origin).await;
        let config: zuse_protocol::ChatConfig = serde_json::from_value(
            serde_json::json!({"harness":"mock","model":"fixture","sandbox":"read-only","plan":false}),
        )
        .unwrap();
        core.workspace
            .create_chat(
                "running-chat",
                None,
                Some(&core.device_id),
                Some(config),
                Some(root.path().to_string_lossy().into_owned()),
            )
            .unwrap();
        // No local servers in this store: the list is the hosted one.
        let list = || plugins.tools("running-chat", &PluginToolRequest::List);
        assert_eq!(list().await.unwrap(), serde_json::json!([]));
        connected.store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(list().await.unwrap()[0]["pluginId"], "posthog");
        server.abort();
        core.shutdown().await;
    }
    /// After the move to the cloud API, the Plugins page offers Reconnect for
    /// what the workspace had on the account API, until it is connected here.
    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn plugins_from_the_previous_service_are_offered_to_reconnect() {
        use axum::response::IntoResponse;
        let snapshot = |connections: serde_json::Value| {
            serde_json::json!({
                "kind": "snapshot",
                "tenants": [{"id": "personal:user_a", "kind": "personal", "name": "Personal"}],
                "tenantId": "personal:user_a",
                "endpoint": "https://api.example/v1/plugins/personal%3Auser_a/mcp",
                "catalog": [
                    {"id":"posthog","name":"PostHog","description":"Analytics"},
                    {"id":"linear","name":"Linear","description":"Issues"},
                    {"id":"custom-old","name":"Docs","description":"docs.example","custom":true},
                    {"id":"custom-new","name":"Docs","description":"docs.example","custom":true},
                    {"id":"notion","name":"Notion","description":"Docs"},
                ],
                "connections": connections,
            })
        };
        let row = |id: &str, plugin: &str, state: &str| serde_json::json!({"id":id,"pluginId":plugin,"label":plugin,"owner":"user","state":state,"createdAt":1.0,"enabled":true});
        let (legacy, legacy_server) = hosted_fixture(
            axum::Json(snapshot(serde_json::json!([
                row("a", "posthog", "connected"),
                row("b", "posthog", "needs-auth"),
                row("c", "linear", "connected"),
                row("d", "custom-old", "connected"),
                row("e", "notion", "connecting"),
            ])))
            .into_response(),
        )
        .await;
        let (cloud, cloud_server) = hosted_fixture(
            axum::Json(snapshot(serde_json::json!([row(
                "f",
                "linear",
                "connected"
            )])))
            .into_response(),
        )
        .await;
        let root = tempfile::tempdir().unwrap();
        let (core, plugins) = hosted_plugins_with_account(root.path(), cloud, legacy).await;
        let PluginResponse::Snapshot { moved, .. } = plugins
            .request(&PluginRequest::List { tenant_id: None })
            .await
            .unwrap()
        else {
            panic!("expected a snapshot");
        };
        assert_eq!(
            moved,
            [
                MovedPlugin {
                    plugin_id: "posthog".into(),
                    name: "PostHog".into(),
                    custom: false
                },
                MovedPlugin {
                    plugin_id: "custom-old".into(),
                    name: "Docs".into(),
                    custom: true
                },
            ]
        );
        legacy_server.abort();
        cloud_server.abort();
        core.shutdown().await;
    }
    #[test]
    fn reconnected_plugins_leave_the_reconnect_list() {
        let definition = |id: &str, name: &str, custom: bool| {
            serde_json::from_value::<zuse_protocol::plugins::PluginDefinition>(
                serde_json::json!({"id":id,"name":name,"description":"","custom":custom}),
            )
            .unwrap()
        };
        let connection = |plugin: &str| {
            serde_json::from_value::<zuse_protocol::plugins::PluginConnection>(serde_json::json!({
                "id":"x","pluginId":plugin,"label":plugin,"owner":"user","state":"connected","createdAt":1.0
            }))
            .unwrap()
        };
        let previous = vec![
            MovedPlugin {
                plugin_id: "posthog".into(),
                name: "PostHog".into(),
                custom: false,
            },
            MovedPlugin {
                plugin_id: "custom-old".into(),
                name: "Docs".into(),
                custom: true,
            },
        ];
        let catalog = [definition("custom-new", "docs", true)];
        assert_eq!(
            not_reconnected(
                previous.clone(),
                &catalog,
                &[connection("posthog"), connection("custom-new")]
            ),
            []
        );
        assert_eq!(not_reconnected(previous.clone(), &catalog, &[]), previous);
        assert!(same_origin("https://api.example/", "https://api.example"));
    }
    #[test]
    fn local_environment_keeps_an_allowlist_and_a_usable_path() {
        let environment = local_environment();
        assert!(environment.contains_key("PATH"));
        assert!(
            environment
                .keys()
                .all(|key| local::base_environment().contains_key(key))
        );
    }
}
