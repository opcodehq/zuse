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
    PluginRequest, PluginResponse, PluginReturnTo, PluginToolRequest, is_local_server,
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
                _ => PluginError::Unavailable,
            })
    }
}

#[derive(Clone)]
pub struct Plugins {
    account: Account,
    workspace: WorkspaceHost,
    bindings: Arc<Mutex<Connection>>,
    callback: Arc<tokio::sync::Mutex<Option<crate::plugin_callback::PluginCallback>>>,
    client: Arc<tokio::sync::Mutex<Option<(String, PluginClient)>>>,
    /// `None` where launching local commands is not allowed, such as hosted runtimes.
    local_root: Option<PathBuf>,
    local: Arc<tokio::sync::Mutex<Option<(String, Arc<LocalServers>)>>>,
}
impl Plugins {
    pub fn open(
        root: &Path,
        account: Account,
        workspace: WorkspaceHost,
        local_servers: bool,
    ) -> anyhow::Result<Self> {
        let db = open_bindings(root)?;
        Ok(Self {
            account,
            workspace,
            bindings: Arc::new(Mutex::new(db)),
            callback: Arc::new(tokio::sync::Mutex::new(None)),
            client: Arc::new(tokio::sync::Mutex::new(None)),
            local_root: local_servers.then(|| root.to_path_buf()),
            local: Arc::new(tokio::sync::Mutex::new(None)),
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
    async fn client(&self, subject: &str) -> Result<PluginClient, PluginError> {
        let mut slot = self.client.lock().await;
        if let Some((owner, client)) = &*slot {
            if owner == subject {
                return Ok(client.clone());
            }
        }
        let client = PluginClient::new(
            self.account.api_url(),
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
            ..
        } = &mut response
        {
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
