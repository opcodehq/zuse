//! Shared hosted-plugin gateway. Identity and policy are resolved by the engine.
use crate::{
    WorkspaceHost,
    account::{Account, AccountError},
};
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
use zuse_plugins::{PluginClient, PluginCredential, PluginCredentials, PluginError};
use zuse_protocol::{
    SandboxLevel,
    plugins::{PluginRequest, PluginResponse, PluginReturnTo, PluginToolRequest},
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
}
impl Plugins {
    pub fn open(root: &Path, account: Account, workspace: WorkspaceHost) -> anyhow::Result<Self> {
        let db = open_bindings(root)?;
        Ok(Self {
            account,
            workspace,
            bindings: Arc::new(Mutex::new(db)),
            callback: Arc::new(tokio::sync::Mutex::new(None)),
            client: Arc::new(tokio::sync::Mutex::new(None)),
        })
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
        // in read-only sessions; callers cannot supply a permission override.
        let permitted = chat
            .config
            .as_ref()
            .is_some_and(|c| c.sandbox != SandboxLevel::ReadOnly);
        if request.invokes_external_tool() && !permitted {
            return Err(PluginError::PermissionRequired.into());
        }
        let cancel = self.account.session_cancellation();
        let credential = self.account.credential().await?;
        let tenant = {
            let db = self.bindings.lock().unwrap_or_else(|e| e.into_inner());
            bind(&db, chat_id, &credential.subject)?
        };
        let client = self.client(&credential.subject).await?;
        Ok(client
            .tools_in_tenant(&tenant, request, permitted, &cancel)
            .await?)
    }
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
}
