//! Authenticated encrypted local records and the existing account storage lease.
use crate::{Error, PendingRegistration, Provider, Registration};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, OsRng, Payload, rand_core::RngCore},
};
use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;
const MAX_RECORD: usize = 64 * 1024;
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Active,
    Pending,
}
impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Pending => "pending",
        }
    }
}
#[async_trait]
pub trait AccountTransport: Send + Sync {
    fn subject(&self) -> &str;
    fn cancellation(&self) -> CancellationToken;
    async fn request(&self, command: Value) -> Result<Value, Error>;
}
struct Local {
    db: Mutex<Connection>,
    cipher: Aes256Gcm,
    gate: [Arc<AsyncMutex<()>>; 2],
    host_id: String,
}
#[derive(Clone)]
pub struct Vault {
    inner: Backend,
    host_id: String,
}
#[derive(Clone)]
enum Backend {
    Local(Arc<Local>),
    Account(Arc<dyn AccountTransport>),
}
pub struct Lease {
    backend: Backend,
    provider: Provider,
    token: String,
    _local: Option<OwnedMutexGuard<()>>,
    released: bool,
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn aad(provider: Provider, kind: Kind, id: &str) -> Vec<u8> {
    serde_json::to_vec(&("zuse-model-connection-v1", provider, kind.name(), id)).unwrap()
}
fn validate_id(id: &str) -> Result<(), Error> {
    if id.is_empty() || id.len() > 256 {
        Err(Error::Storage)
    } else {
        Ok(())
    }
}
impl Vault {
    pub fn local(root: &Path) -> Result<Self, Error> {
        let directory = root.join("model-accounts");
        std::fs::create_dir_all(&directory).map_err(|_| Error::Storage)?;
        private(&directory, 0o700)?;
        let key = key_file(&directory)?;
        let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| Error::Storage)?;
        let path = directory.join("connections.sqlite");
        let db = Connection::open(&path).map_err(|_| Error::Storage)?;
        private(&path, 0o600)?;
        db.busy_timeout(Duration::from_secs(10))
            .map_err(|_| Error::Storage)?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS records(ordinal INTEGER PRIMARY KEY AUTOINCREMENT,provider TEXT NOT NULL,kind TEXT NOT NULL,record_id TEXT NOT NULL,envelope BLOB NOT NULL,UNIQUE(provider,kind,record_id)); CREATE TABLE IF NOT EXISTS identity(id INTEGER PRIMARY KEY CHECK(id=1),host_id TEXT NOT NULL);").map_err(|_|Error::Storage)?;
        db.execute(
            "INSERT OR IGNORE INTO identity(id,host_id) VALUES(1,?1)",
            [format!("urn:uuid:{}", uuid::Uuid::new_v4())],
        )
        .map_err(|_| Error::Storage)?;
        let host_id: String = db
            .query_row("SELECT host_id FROM identity WHERE id=1", [], |r| r.get(0))
            .map_err(|_| Error::Storage)?;
        Ok(Self {
            inner: Backend::Local(Arc::new(Local {
                db: Mutex::new(db),
                cipher,
                gate: [Arc::new(AsyncMutex::new(())), Arc::new(AsyncMutex::new(()))],
                host_id: host_id.clone(),
            })),
            host_id,
        })
    }
    pub fn account(transport: Arc<dyn AccountTransport>, host_id: String) -> Self {
        Self {
            inner: Backend::Account(transport),
            host_id,
        }
    }
    pub fn host_id(&self) -> &str {
        &self.host_id
    }
    pub fn cancellation(&self) -> CancellationToken {
        match &self.inner {
            Backend::Local(_) => CancellationToken::new(),
            Backend::Account(t) => t.cancellation(),
        }
    }
    pub fn storage(&self) -> &'static str {
        match self.inner {
            Backend::Local(_) => "local",
            Backend::Account(_) => "account",
        }
    }
    pub fn subject(&self) -> Option<&str> {
        match &self.inner {
            Backend::Local(_) => None,
            Backend::Account(t) => Some(t.subject()),
        }
    }
    pub async fn acquire(&self, provider: Provider) -> Result<Lease, Error> {
        let mut lease = Lease {
            backend: self.inner.clone(),
            provider,
            token: uuid::Uuid::new_v4().to_string(),
            _local: None,
            released: false,
        };
        lease._local = match &self.inner {
            Backend::Local(inner) => Some(
                inner.gate[usize::from(provider == Provider::Supergrok)]
                    .clone()
                    .lock_owned()
                    .await,
            ),
            Backend::Account(transport) => {
                let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
                let cancel = transport.cancellation();
                loop {
                    let acquired = tokio::select! {biased;_=cancel.cancelled()=>return Err(Error::Changed),result=transport.request(json!({"provider":provider,"action":"acquire","token":lease.token}))=>result?};
                    if acquired.as_bool() == Some(true) {
                        break;
                    }
                    if acquired.as_bool() != Some(false) {
                        return Err(Error::Storage);
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Err(Error::Busy);
                    }
                    tokio::select! {biased;_=cancel.cancelled()=>return Err(Error::Changed),_=tokio::time::sleep(Duration::from_millis(250))=>{}}
                }
                None
            }
        };
        Ok(lease)
    }
}
impl Lease {
    async fn command(&self, action: &str, rest: Value) -> Result<Value, Error> {
        let Backend::Account(transport) = &self.backend else {
            return Err(Error::Storage);
        };
        let mut body = json!({"provider":self.provider,"action":action,"token":self.token});
        body.as_object_mut()
            .unwrap()
            .extend(rest.as_object().cloned().unwrap_or_default());
        let cancel = transport.cancellation();
        tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Changed),result=transport.request(body)=>result}
    }
    async fn read<T: DeserializeOwned>(&self, kind: Kind, id: &str) -> Result<Option<T>, Error> {
        validate_id(id)?;
        let bytes = match &self.backend {
            Backend::Local(inner) => {
                let blob:Option<Vec<u8>>=lock(&inner.db).query_row("SELECT envelope FROM records WHERE provider=?1 AND kind=?2 AND record_id=?3",params![self.provider.as_str(),kind.name(),id],|r|r.get(0)).optional().map_err(|_|Error::Storage)?;
                blob.map(|blob| open(inner, self.provider, kind, id, &blob))
                    .transpose()?
            }
            Backend::Account(_) => match self
                .command("read", json!({"kind":kind.name(),"id":id}))
                .await?
            {
                Value::Null => None,
                Value::String(value) if value.len() <= MAX_RECORD => {
                    Some(Zeroizing::new(value.into_bytes()))
                }
                _ => return Err(Error::Storage),
            },
        };
        bytes
            .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| Error::Storage))
            .transpose()
    }
    async fn write<T: Serialize>(
        &self,
        kind: Kind,
        id: &str,
        value: Option<&T>,
    ) -> Result<(), Error> {
        validate_id(id)?;
        let bytes = value
            .map(|v| {
                serde_json::to_vec(v)
                    .map(Zeroizing::new)
                    .map_err(|_| Error::Storage)
            })
            .transpose()?;
        if bytes.as_ref().is_some_and(|b| b.len() > MAX_RECORD) {
            return Err(Error::Storage);
        }
        match &self.backend {
            Backend::Local(inner) => {
                let db = lock(&inner.db);
                if let Some(bytes) = bytes {
                    let mut nonce = [0u8; 12];
                    OsRng
                        .try_fill_bytes(&mut nonce)
                        .map_err(|_| Error::Storage)?;
                    let mut envelope = vec![1];
                    envelope.extend_from_slice(&nonce);
                    envelope.extend_from_slice(
                        &inner
                            .cipher
                            .encrypt(
                                Nonce::from_slice(&nonce),
                                Payload {
                                    msg: &bytes,
                                    aad: &aad(self.provider, kind, id),
                                },
                            )
                            .map_err(|_| Error::Storage)?,
                    );
                    db.execute("INSERT INTO records(provider,kind,record_id,envelope) VALUES(?1,?2,?3,?4) ON CONFLICT(provider,kind,record_id) DO UPDATE SET envelope=excluded.envelope",params![self.provider.as_str(),kind.name(),id,envelope]).map_err(|_|Error::Storage)?;
                } else {
                    db.execute(
                        "DELETE FROM records WHERE provider=?1 AND kind=?2 AND record_id=?3",
                        params![self.provider.as_str(), kind.name(), id],
                    )
                    .map_err(|_| Error::Storage)?;
                }
            }
            Backend::Account(_) => {
                let value = bytes
                    .map(|b| String::from_utf8(b.to_vec()).map_err(|_| Error::Storage))
                    .transpose()?;
                self.command("write", json!({"kind":kind.name(),"id":id,"value":value}))
                    .await?;
            }
        }
        Ok(())
    }
    async fn list<T: DeserializeOwned>(&self, kind: Kind) -> Result<Vec<T>, Error> {
        match &self.backend {
            Backend::Local(inner) => {
                let db = lock(&inner.db);
                let mut statement=db.prepare("SELECT record_id,envelope FROM records WHERE provider=?1 AND kind=?2 ORDER BY ordinal").map_err(|_|Error::Storage)?;
                let rows = statement
                    .query_map(params![self.provider.as_str(), kind.name()], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
                    })
                    .map_err(|_| Error::Storage)?;
                rows.map(|row| {
                    let (id, blob) = row.map_err(|_| Error::Storage)?;
                    let bytes = open(inner, self.provider, kind, &id, &blob)?;
                    serde_json::from_slice(&bytes).map_err(|_| Error::Storage)
                })
                .collect()
            }
            Backend::Account(_) => {
                let values = self.command("list", json!({"kind":kind.name()})).await?;
                let list = values.as_array().ok_or(Error::Storage)?;
                if list.len() > 4096 {
                    return Err(Error::Storage);
                }
                list.iter()
                    .map(|v| {
                        let text = v
                            .as_str()
                            .filter(|v| v.len() <= MAX_RECORD)
                            .ok_or(Error::Storage)?;
                        serde_json::from_str(text).map_err(|_| Error::Storage)
                    })
                    .collect()
            }
        }
    }
    pub async fn registration(&self, id: &str) -> Result<Option<Registration>, Error> {
        self.read(Kind::Active, id).await
    }
    pub async fn registrations(&self) -> Result<Vec<Registration>, Error> {
        self.list(Kind::Active).await
    }
    pub async fn save(&self, registration: &Registration) -> Result<(), Error> {
        self.write(Kind::Active, &registration.id, Some(registration))
            .await
    }
    pub async fn pending(&self, id: &str) -> Result<Option<PendingRegistration>, Error> {
        self.read(Kind::Pending, id).await
    }
    pub async fn pending_all(&self) -> Result<Vec<PendingRegistration>, Error> {
        self.list(Kind::Pending).await
    }
    pub async fn save_pending(&self, value: &PendingRegistration) -> Result<(), Error> {
        self.write(Kind::Pending, &value.id, Some(value)).await
    }
    pub async fn remove_pending(&self, id: &str) -> Result<(), Error> {
        self.write::<PendingRegistration>(Kind::Pending, id, None)
            .await
    }
    pub async fn finish(mut self) {
        self.released = true;
        if matches!(self.backend, Backend::Account(_)) {
            let _ = self.command("release", json!({})).await;
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        if let Backend::Account(transport) = &self.backend {
            let transport = transport.clone();
            let command = json!({"provider":self.provider,"action":"release","token":self.token});
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = transport.request(command).await;
                });
            }
        }
    }
}
fn open(
    inner: &Local,
    provider: Provider,
    kind: Kind,
    id: &str,
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>, Error> {
    if blob.len() < 29 || blob[0] != 1 || blob.len() > MAX_RECORD + 29 {
        return Err(Error::Storage);
    }
    inner
        .cipher
        .decrypt(
            Nonce::from_slice(&blob[1..13]),
            Payload {
                msg: &blob[13..],
                aad: &aad(provider, kind, id),
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| Error::Storage)
}
fn private(path: &Path, mode: u32) -> Result<(), Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
            .map_err(|_| Error::Storage)?;
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
    Ok(())
}
fn key_file(directory: &Path) -> Result<Zeroizing<[u8; 32]>, Error> {
    let path = directory.join("key");
    if !path.exists() {
        let mut key = Zeroizing::new([0u8; 32]);
        OsRng
            .try_fill_bytes(key.as_mut())
            .map_err(|_| Error::Storage)?;
        let temp: PathBuf = directory.join(format!(".key-{}", uuid::Uuid::new_v4()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let created = (|| {
            let mut f = options.open(&temp).map_err(|_| Error::Storage)?;
            f.write_all(key.as_ref())
                .and_then(|_| f.sync_all())
                .map_err(|_| Error::Storage)?;
            match std::fs::hard_link(&temp, &path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(Error::Storage),
            }
            #[cfg(unix)]
            std::fs::File::open(directory)
                .and_then(|f| f.sync_all())
                .map_err(|_| Error::Storage)?;
            Ok(())
        })();
        let _ = std::fs::remove_file(temp);
        created?;
    }
    private(&path, 0o600)?;
    let bytes = Zeroizing::new(std::fs::read(&path).map_err(|_| Error::Storage)?);
    let key: [u8; 32] = bytes.as_slice().try_into().map_err(|_| Error::Storage)?;
    Ok(Zeroizing::new(key))
}
