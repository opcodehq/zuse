//! Shared local/account model registrations. Credentials stay behind this service.
use crate::account::Account;
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::{StreamExt, stream::BoxStream};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, OnceCell};
use tokio_util::sync::CancellationToken;
use zuse_model_accounts::{
    Connection, Credential, Error, ModelAccounts, Provider, SignInEvent,
    vault::{AccountTransport, Vault},
};

#[derive(Clone)]
pub struct ModelConnections(Arc<Inner>);
struct Inner {
    root: PathBuf,
    identity: Account,
    local: OnceCell<ModelAccounts>,
    account: Mutex<Option<(String, ModelAccounts)>>,
    retired: AtomicBool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub available: bool,
    pub local_available: bool,
    pub account_available: bool,
    pub account_error: bool,
    pub chatgpt_available: bool,
    pub supergrok_available: bool,
    pub connections: Vec<Connection>,
}
fn account_id(subject: &str, id: &str) -> String {
    format!("account:{}:{id}", URL_SAFE_NO_PAD.encode(subject))
}
fn provider(id: &str) -> Provider {
    if id.starts_with("supergrok:") {
        Provider::Supergrok
    } else {
        Provider::Chatgpt
    }
}
impl ModelConnections {
    pub fn new(root: PathBuf, identity: Account) -> Self {
        Self(Arc::new(Inner {
            root,
            identity,
            local: OnceCell::new(),
            account: Mutex::new(None),
            retired: AtomicBool::new(false),
        }))
    }
    async fn local(&self) -> Result<ModelAccounts, Error> {
        if self.0.retired.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        self.0
            .local
            .get_or_try_init(|| async { ModelAccounts::new(Vault::local(&self.0.root)?) })
            .await
            .cloned()
    }
    async fn account(&self) -> Result<Option<(String, ModelAccounts)>, Error> {
        if self.0.retired.load(Ordering::Acquire) {
            return Err(Error::Cancelled);
        }
        let subject = self.0.identity.status().user.map(|u| u.id);
        let mut slot = self.0.account.lock().await;
        if slot.as_ref().map(|(id, _)| id) != subject.as_ref() {
            if let Some((_, old)) = slot.take() {
                old.retire();
            }
        }
        let Some(subject) = subject else {
            return Ok(None);
        };
        if let Some((id, manager)) = &*slot {
            return Ok(Some((id.clone(), manager.clone())));
        }
        let host_id = self.local().await?.vault().host_id().to_owned();
        let transport = Arc::new(AccountStore {
            authority: self.0.identity.clone(),
            subject: subject.clone(),
            cancel: self.0.identity.session_cancellation(),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(15))
                .build()
                .map_err(|_| Error::Unavailable)?,
        });
        let manager = ModelAccounts::new(Vault::account(transport, host_id))?;
        *slot = Some((subject.clone(), manager.clone()));
        Ok(Some((subject, manager)))
    }
    pub async fn status(&self) -> Status {
        let local = self.local().await;
        let local_available = local.is_ok();
        let mut connections = Vec::new();
        let local_ok = if let Ok(local) = local {
            match local.list().await {
                Ok(list) => {
                    connections.extend(list);
                    true
                }
                Err(_) => false,
            }
        } else {
            false
        };
        let mut account_available = false;
        let mut account_error = false;
        match self.account().await {
            Ok(Some((subject, manager))) => match manager.list().await {
                Ok(mut list) => {
                    account_available = true;
                    for connection in &mut list {
                        connection.id = account_id(&subject, &connection.id);
                    }
                    connections.extend(list);
                }
                Err(_) => account_error = true,
            },
            Ok(None) => {}
            Err(_) => account_error = true,
        }
        connections.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Status {
            available: local_ok || account_available,
            local_available: local_available && local_ok,
            account_available,
            account_error,
            chatgpt_available: local_ok || account_available,
            supergrok_available: local_ok || account_available,
            connections,
        }
    }
    async fn owner(&self, id: &str) -> Result<(ModelAccounts, String, Option<String>), Error> {
        if let Some(rest) = id.strip_prefix("account:") {
            let (encoded, raw) = rest.split_once(':').ok_or(Error::Unknown)?;
            let subject = String::from_utf8(
                URL_SAFE_NO_PAD
                    .decode(encoded)
                    .map_err(|_| Error::Unknown)?,
            )
            .map_err(|_| Error::Unknown)?;
            let (current, manager) = self.account().await?.ok_or(Error::Changed)?;
            if current != subject {
                return Err(Error::Changed);
            }
            if raw.is_empty() {
                return Err(Error::Unknown);
            }
            Ok((manager, raw.into(), Some(current)))
        } else {
            Ok((self.local().await?, id.into(), None))
        }
    }
    pub async fn credential(&self, id: &str) -> Result<Credential, Error> {
        let (manager, raw, _) = self.owner(id).await?;
        let mut credential = manager.credential(provider(&raw), &raw).await?;
        credential.connection_id = id.into();
        Ok(credential)
    }
    pub async fn connect(
        &self,
        connection_id: Option<String>,
        selected: Provider,
        storage: &str,
    ) -> Result<BoxStream<'static, Value>, Error> {
        let (manager, raw, subject) = if let Some(id) = connection_id {
            let (manager, raw, subject) = self.owner(&id).await?;
            (manager, Some(raw), subject)
        } else if storage == "account" {
            let (subject, manager) = self.account().await?.ok_or(Error::Changed)?;
            (manager, None, Some(subject))
        } else if storage == "local" {
            (self.local().await?, None, None)
        } else {
            return Err(Error::Unknown);
        };
        let selected = raw.as_deref().map(provider).unwrap_or(selected);
        let login = manager.connect(selected, raw).await?;
        let full_id = subject
            .as_ref()
            .map_or_else(|| login.id.clone(), |s| account_id(s, &login.id));
        let receiver = login.events;
        let stream = futures::stream::unfold(
            (receiver, true, full_id, subject),
            |(mut receiver, first, id, subject)| async move {
                if !first && receiver.changed().await.is_err() {
                    return None;
                }
                let event = receiver.borrow_and_update().clone()?;
                let terminal = matches!(
                    event,
                    SignInEvent::Connected { .. } | SignInEvent::Error { .. }
                );
                let mut event = event;
                if let (Some(subject), SignInEvent::Connected { connection }) =
                    (&subject, &mut event)
                {
                    connection.id = account_id(subject, &connection.id);
                }
                let frame = json!({"connectionId":id,"event":event});
                // The sender is dropped after terminal delivery. Keeping the receiver
                // until the next poll avoids losing the final watch snapshot.
                let _ = terminal;
                Some((frame, (receiver, false, id, subject)))
            },
        );
        Ok(stream.boxed())
    }
    pub async fn complete_login(&self, id: &str, url: &str) -> Result<(), Error> {
        let (m, raw, _) = self.owner(id).await?;
        m.complete_login(provider(&raw), &raw, url).await
    }
    pub async fn rename(&self, id: &str, name: &str) -> Result<(), Error> {
        let (m, raw, _) = self.owner(id).await?;
        m.rename(provider(&raw), &raw, name).await
    }
    pub async fn preferred(&self, id: &str) -> Result<(), Error> {
        let (m, raw, _) = self.owner(id).await?;
        m.preferred(provider(&raw), &raw).await
    }
    pub async fn acknowledge_plan(&self, id: &str) -> Result<(), Error> {
        let (m, raw, _) = self.owner(id).await?;
        m.acknowledge_plan(provider(&raw), &raw).await
    }
    pub async fn disconnect(&self, id: &str) -> Result<bool, Error> {
        let (m, raw, _) = self.owner(id).await?;
        m.disconnect(provider(&raw), &raw).await
    }
    pub async fn cancel_login(&self, id: &str) -> Result<(), Error> {
        let (m, raw, _) = self.owner(id).await?;
        m.cancel(provider(&raw), &raw);
        Ok(())
    }
    pub async fn retire(&self) {
        self.0.retired.store(true, Ordering::Release);
        if let Some(local) = self.0.local.get() {
            local.retire();
        }
        if let Some((_, account)) = self.0.account.lock().await.take() {
            account.retire();
        }
    }
}
struct AccountStore {
    authority: Account,
    subject: String,
    cancel: CancellationToken,
    http: reqwest::Client,
}
#[async_trait]
impl AccountTransport for AccountStore {
    fn subject(&self) -> &str {
        &self.subject
    }
    fn cancellation(&self) -> CancellationToken {
        self.cancel.clone()
    }
    async fn request(&self, command: Value) -> Result<Value, Error> {
        let credential = self
            .authority
            .credential()
            .await
            .map_err(|_| Error::Changed)?;
        if credential.subject != self.subject || self.cancel.is_cancelled() {
            return Err(Error::Changed);
        }
        let response = self
            .http
            .post(format!(
                "{}/v1/model-connections/storage",
                self.authority.api_url()
            ))
            .bearer_auth(credential.access_token)
            .json(&command)
            .send()
            .await
            .map_err(|_| Error::Unavailable)?;
        if !response.status().is_success() {
            return Err(Error::Unavailable);
        }
        if response
            .content_length()
            .is_some_and(|n| n > 2 * 1024 * 1024)
        {
            return Err(Error::Storage);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| Error::Unavailable)?;
            if bytes.len() + chunk.len() > 2 * 1024 * 1024 {
                return Err(Error::Storage);
            }
            bytes.extend_from_slice(&chunk);
        }
        if self.cancel.is_cancelled()
            || self
                .authority
                .status()
                .user
                .as_ref()
                .is_none_or(|u| u.id != self.subject)
        {
            return Err(Error::Changed);
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?;
        value.get("value").cloned().ok_or(Error::Storage)
    }
}
