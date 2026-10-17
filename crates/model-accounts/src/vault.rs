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
        if matches!(self.backend, Backend::Account(_)) {
            // Keep Drop armed until the request completes: cancellation can happen
            // after the server receives the request but before its response arrives.
            if self.command("release", json!({})).await.is_ok() {
                self.released = true;
            }
        } else {
            self.released = true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Notify;

    fn record(id: &str) -> Registration {
        Registration {
            id: id.into(),
            client_id: "test-client".into(),
            subject: "test-subject".into(),
            email: Some("person@example.test".into()),
            name: "Personal".into(),
            created_at: 1,
            preferred: false,
            plan_notice_seen: false,
            authorization_generation: 1,
            scope: Provider::Chatgpt.scope().into(),
            id_token: Some("fixture-id-token".into()),
            access_token: Some("fixture-access-token".into()),
            refresh_token: Some("fixture-refresh-token".into()),
            expires_at: Some(crate::now_ms() + 3600000),
            earliest_refresh_at: None,
        }
    }
    #[tokio::test]
    async fn persistent_records_are_encrypted_and_bound_to_their_context() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::local(directory.path()).unwrap();
        let host = vault.host_id().to_owned();
        let lease = vault.acquire(Provider::Chatgpt).await.unwrap();
        lease.save(&record("a")).await.unwrap();
        lease.save(&record("b")).await.unwrap();
        assert_eq!(
            lease
                .registration("a")
                .await
                .unwrap()
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("fixture-refresh-token")
        );
        lease.finish().await;
        drop(vault);
        let vault = Vault::local(directory.path()).unwrap();
        assert_eq!(vault.host_id(), host);
        let db =
            Connection::open(directory.path().join("model-accounts/connections.sqlite")).unwrap();
        let a: Vec<u8> = db
            .query_row(
                "SELECT envelope FROM records WHERE record_id='a'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let b: Vec<u8> = db
            .query_row(
                "SELECT envelope FROM records WHERE record_id='b'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_ne!(a, b);
        for plaintext in [
            b"fixture-refresh-token".as_slice(),
            b"fixture-access-token",
            b"person@example.test",
        ] {
            assert!(!a.windows(plaintext.len()).any(|bytes| bytes == plaintext));
        }
        db.execute("UPDATE records SET envelope=?1 WHERE record_id='a'", [&b])
            .unwrap();
        let lease = vault.acquire(Provider::Chatgpt).await.unwrap();
        assert!(matches!(lease.registration("a").await, Err(Error::Storage)));
        lease.finish().await;
        db.execute("UPDATE records SET envelope=?1 WHERE record_id='a'", [&a])
            .unwrap();
        db.execute(
            "UPDATE records SET provider='supergrok' WHERE record_id='a'",
            [],
        )
        .unwrap();
        let lease = vault.acquire(Provider::Supergrok).await.unwrap();
        assert!(matches!(lease.registration("a").await, Err(Error::Storage)));
        lease.finish().await;
        db.execute(
            "UPDATE records SET provider='chatgpt',kind='pending' WHERE record_id='a'",
            [],
        )
        .unwrap();
        let lease = vault.acquire(Provider::Chatgpt).await.unwrap();
        assert!(matches!(lease.pending("a").await, Err(Error::Storage)));
        lease.finish().await;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = directory.path().join("model-accounts");
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            for file in ["key", "connections.sqlite"] {
                assert_eq!(
                    std::fs::metadata(path.join(file))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
    }
    #[tokio::test]
    async fn a_damaged_key_is_never_replaced_and_provider_locks_are_independent() {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::local(directory.path()).unwrap();
        let lease = vault.acquire(Provider::Chatgpt).await.unwrap();
        let other = vault.acquire(Provider::Supergrok).await.unwrap();
        let waiting = {
            let vault = vault.clone();
            tokio::spawn(async move { vault.acquire(Provider::Chatgpt).await })
        };
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        lease.finish().await;
        waiting.await.unwrap().unwrap().finish().await;
        other.finish().await;
        drop(vault);
        let key = directory.path().join("model-accounts/key");
        std::fs::write(&key, b"damaged").unwrap();
        assert!(matches!(
            Vault::local(directory.path()),
            Err(Error::Storage)
        ));
        assert_eq!(std::fs::read(key).unwrap(), b"damaged");
    }
    #[test]
    fn shared_registrations_keep_the_existing_optional_field_contract() {
        // Shared records are also consumed by existing agents, whose schema
        // accepts absent optional properties rather than JSON nulls.
        let mut value = record("a");
        value.email = None;
        value.id_token = None;
        value.access_token = None;
        value.refresh_token = None;
        value.expires_at = None;
        value.earliest_refresh_at = None;
        let encoded = serde_json::to_value(&value).unwrap();
        for field in [
            "email",
            "idToken",
            "accessToken",
            "refreshToken",
            "expiresAt",
            "earliestRefreshAt",
        ] {
            assert!(encoded.get(field).is_none());
        }
        let mut legacy = encoded;
        legacy
            .as_object_mut()
            .unwrap()
            .remove("authorizationGeneration");
        legacy.as_object_mut().unwrap().remove("planNoticeSeen");
        let decoded: Registration = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.authorization_generation, 0);
        assert!(!decoded.plan_notice_seen);
        assert!(decoded.access_token.is_none());
    }
    struct Remote {
        hang_acquire: bool,
        hang_first_release: bool,
        calls: Mutex<Vec<Value>>,
        releases: AtomicUsize,
        started: Notify,
        released: Notify,
        cancel: CancellationToken,
    }
    impl Remote {
        fn new(hang_acquire: bool, hang_first_release: bool) -> Arc<Self> {
            Arc::new(Self {
                hang_acquire,
                hang_first_release,
                calls: Mutex::new(vec![]),
                releases: AtomicUsize::new(0),
                started: Notify::new(),
                released: Notify::new(),
                cancel: CancellationToken::new(),
            })
        }
        async fn wait(&self, notify: &Notify) {
            tokio::time::timeout(Duration::from_secs(3), notify.notified())
                .await
                .unwrap();
        }
    }
    #[async_trait]
    impl AccountTransport for Remote {
        fn subject(&self) -> &str {
            "fixture-subject"
        }
        fn cancellation(&self) -> CancellationToken {
            self.cancel.clone()
        }
        async fn request(&self, command: Value) -> Result<Value, Error> {
            lock(&self.calls).push(command.clone());
            match command["action"].as_str() {
                Some("acquire") => {
                    if self.hang_acquire {
                        self.started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    Ok(Value::Bool(true))
                }
                Some("release") => {
                    let previous = self.releases.fetch_add(1, Ordering::SeqCst);
                    if self.hang_first_release && previous == 0 {
                        self.started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    self.released.notify_one();
                    Ok(Value::Null)
                }
                _ => Err(Error::Storage),
            }
        }
    }
    #[tokio::test]
    async fn cancelled_acquisition_releases_the_same_remote_token() {
        let remote = Remote::new(true, false);
        let vault = Vault::account(remote.clone(), "fixture-host".into());
        let task = tokio::spawn(async move { vault.acquire(Provider::Chatgpt).await });
        remote.wait(&remote.started).await;
        task.abort();
        let _ = task.await;
        remote.wait(&remote.released).await;
        let calls = lock(&remote.calls);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["token"], calls[1]["token"]);
        assert_eq!(calls[1]["action"], "release");
    }
    #[tokio::test]
    async fn interrupted_release_still_runs_drop_cleanup() {
        let remote = Remote::new(false, true);
        let vault = Vault::account(remote.clone(), "fixture-host".into());
        let lease = vault.acquire(Provider::Chatgpt).await.unwrap();
        let task = tokio::spawn(lease.finish());
        remote.wait(&remote.started).await;
        task.abort();
        let _ = task.await;
        remote.wait(&remote.released).await;
        assert_eq!(remote.releases.load(Ordering::SeqCst), 2);
        let calls = lock(&remote.calls);
        assert_eq!(calls[0]["token"], calls[2]["token"]);
    }
}
