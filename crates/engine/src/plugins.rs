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
        let db = Connection::open(root.join("plugin-sessions.sqlite"))?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS bindings(chat_id TEXT PRIMARY KEY, subject TEXT NOT NULL);")?;
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
        client.request(&request, &cancel).await
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
        {
            let db = self.bindings.lock().unwrap_or_else(|e| e.into_inner());
            bind(&db, chat_id, &credential.subject)?;
        }
        let client = self.client(&credential.subject).await?;
        Ok(client.tools(request, permitted, &cancel).await?)
    }
}

fn bind(db: &Connection, chat_id: &str, subject: &str) -> anyhow::Result<()> {
    // INSERT OR IGNORE and read occur under the gateway mutex, across all RPC
    // services. SQLite preserves the binding when the engine restarts.
    db.execute(
        "INSERT OR IGNORE INTO bindings(chat_id,subject) VALUES (?1,?2)",
        params![chat_id, subject],
    )?;
    let owner: Option<String> = db
        .query_row(
            "SELECT subject FROM bindings WHERE chat_id=?1",
            [chat_id],
            |row| row.get(0),
        )
        .optional()?;
    if owner.as_deref() != Some(subject) {
        return Err(PluginError::AccountChanged.into());
    }
    Ok(())
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
        let path = dir.path().join("bindings.sqlite");
        let db = Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE bindings(chat_id TEXT PRIMARY KEY,subject TEXT NOT NULL)")
            .unwrap();
        bind(&db, "chat-a", "user-a").unwrap();
        bind(&db, "chat-b", "user-a").unwrap();
        drop(db);
        let db = Connection::open(path).unwrap();
        bind(&db, "chat-a", "user-a").unwrap();
        assert!(bind(&db, "chat-a", "user-b").is_err());
        bind(&db, "chat-c", "user-b").unwrap();
        assert!(bind(&db, "chat-b", "user-b").is_err());
    }
}
