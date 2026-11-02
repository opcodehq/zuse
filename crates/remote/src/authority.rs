//! Profile-owned pairing grants. Successful persistence precedes authorization.
use crate::{Invitation, PairId, PairSecret, PeerMetadata};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, Zeroizing};

const INVITE_SECONDS: u64 = 600;
const MAX_PEERS: usize = 32;
const MAX_STORE_BYTES: u64 = 128 * 1024;
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Pairing storage is owned by another runtime")]
    Busy,
    #[error("Pairing storage is invalid; restore or inspect the saved file")]
    Invalid,
    #[error("Pairing storage could not be saved; restart after fixing storage")]
    Storage,
    #[error("Pairing authority is closed")]
    Closed,
    #[error("Pairing invitation has expired")]
    Expired,
    #[error("Pairing authorization was revoked")]
    Revoked,
    #[error("Use a device label of 1–128 bytes without control characters")]
    Label,
    #[error("Remove an existing pairing before adding another device")]
    Full,
}
fn now() -> Result<u64, Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .map_err(|_| Error::Storage)
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
fn label_valid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[derive(Clone)]
struct Entry {
    metadata: PeerMetadata,
    secret: PairSecret,
    cancel: CancellationToken,
}
struct State {
    entries: BTreeMap<String, Entry>,
    owner: Option<File>,
    closed: bool,
}
struct Shared {
    directory: PathBuf,
    state: Mutex<State>,
}
#[derive(Clone)]
pub struct Authority(Arc<Shared>);
/// A connection grant belongs to this authority instance and its revocation token.
pub struct Credentials {
    id: PairId,
    secret: PairSecret,
    cancel: CancellationToken,
    owner: Weak<Shared>,
}
impl Credentials {
    pub fn id(&self) -> &PairId {
        &self.id
    }
    pub fn secret(&self) -> &PairSecret {
        &self.secret
    }
    pub fn is_live(&self) -> bool {
        !self.cancel.is_cancelled() && self.owner.strong_count() != 0
    }
    pub async fn revoked(&self) {
        self.cancel.cancelled().await;
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    version: u32,
    peers: Vec<StoredPeer>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPeer {
    metadata: PeerMetadata,
    secret: String,
}
impl Drop for StoredPeer {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}
fn close(state: &mut State) {
    state.closed = true;
    for entry in state.entries.values() {
        entry.cancel.cancel();
    }
    state.entries.clear();
    if let Some(owner) = state.owner.take() {
        let _ = owner.unlock();
    }
}
impl Drop for Shared {
    fn drop(&mut self) {
        close(self.state.get_mut().unwrap_or_else(|e| e.into_inner()));
    }
}
impl Authority {
    pub fn open(profile: &Path) -> Result<Self, Error> {
        let directory = profile.join("pairing");
        std::fs::create_dir_all(&directory).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| Error::Storage)?;
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let owner = options
            .open(directory.join("owner.lock"))
            .map_err(|_| Error::Storage)?;
        owner.try_lock().map_err(|e| match e {
            std::fs::TryLockError::WouldBlock => Error::Busy,
            _ => Error::Storage,
        })?;
        let path = directory.join("peers.json");
        let mut entries = BTreeMap::new();
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.len() > MAX_STORE_BYTES {
                    return Err(Error::Invalid);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                        .map_err(|_| Error::Storage)?;
                }
                let mut bytes = Zeroizing::new(vec![]);
                File::open(&path)
                    .map_err(|_| Error::Storage)?
                    .take(MAX_STORE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| Error::Storage)?;
                if bytes.len() as u64 > MAX_STORE_BYTES {
                    return Err(Error::Invalid);
                }
                let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
                if stored.version != 1 || stored.peers.len() > MAX_PEERS {
                    return Err(Error::Invalid);
                }
                for peer in stored.peers {
                    PairId::parse(&peer.metadata.id).map_err(|_| Error::Invalid)?;
                    if !label_valid(&peer.metadata.label)
                        || peer.metadata.expires_at.is_none() != peer.metadata.paired_at.is_some()
                        || entries.contains_key(&peer.metadata.id)
                    {
                        return Err(Error::Invalid);
                    }
                    let secret = PairSecret::parse(&peer.secret).map_err(|_| Error::Invalid)?;
                    entries.insert(
                        peer.metadata.id.clone(),
                        Entry {
                            metadata: peer.metadata.clone(),
                            secret,
                            cancel: CancellationToken::new(),
                        },
                    );
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Storage),
        }
        Ok(Self(Arc::new(Shared {
            directory,
            state: Mutex::new(State {
                entries,
                owner: Some(owner),
                closed: false,
            }),
        })))
    }
    fn persist(&self, entries: &BTreeMap<String, Entry>) -> Result<(), Error> {
        let stored = Stored {
            version: 1,
            peers: entries
                .values()
                .map(|entry| StoredPeer {
                    metadata: entry.metadata.clone(),
                    secret: entry.secret.expose(),
                })
                .collect(),
        };
        let bytes = Zeroizing::new(serde_json::to_vec(&stored).map_err(|_| Error::Storage)?);
        if bytes.len() as u64 > MAX_STORE_BYTES {
            return Err(Error::Storage);
        }
        let suffix = PairId::generate().map_err(|_| Error::Storage)?;
        let temporary = self
            .0
            .directory
            .join(format!(".peers-{}.tmp", suffix.as_str()));
        let result = (|| -> std::io::Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            std::fs::rename(&temporary, self.0.directory.join("peers.json"))?;
            File::open(&self.0.directory)?.sync_all()?;
            Ok(())
        })();
        let _ = std::fs::remove_file(temporary);
        result.map_err(|_| Error::Storage)
    }
    fn commit(&self, state: &mut State, entries: BTreeMap<String, Entry>) -> Result<(), Error> {
        if let Err(error) = self.persist(&entries) {
            close(state);
            return Err(error);
        }
        for (id, entry) in &state.entries {
            if !entries.contains_key(id) {
                entry.cancel.cancel();
            }
        }
        state.entries = entries;
        Ok(())
    }
    pub fn create(&self, label: &str) -> Result<(PeerMetadata, Invitation), Error> {
        let label = label.trim();
        if !label_valid(label) {
            return Err(Error::Label);
        }
        let time = now()?;
        let invitation = Invitation::generate().map_err(|_| Error::Storage)?;
        let mut state = lock(&self.0.state);
        if state.closed {
            return Err(Error::Closed);
        }
        let mut entries = state.entries.clone();
        entries.retain(|_, e| e.metadata.expires_at.is_none_or(|expiry| time < expiry));
        if entries.len() >= MAX_PEERS {
            return Err(Error::Full);
        }
        if entries.contains_key(invitation.id.as_str()) {
            return Err(Error::Storage);
        }
        let metadata = PeerMetadata {
            id: invitation.id.as_str().into(),
            label: label.into(),
            created_at: time,
            expires_at: Some(time.saturating_add(INVITE_SECONDS)),
            paired_at: None,
        };
        entries.insert(
            metadata.id.clone(),
            Entry {
                metadata: metadata.clone(),
                secret: invitation.secret.clone(),
                cancel: CancellationToken::new(),
            },
        );
        self.commit(&mut state, entries)?;
        Ok((metadata, invitation))
    }
    pub fn list(&self) -> Result<Vec<PeerMetadata>, Error> {
        let state = lock(&self.0.state);
        if state.closed {
            return Err(Error::Closed);
        }
        Ok(state
            .entries
            .values()
            .map(|entry| entry.metadata.clone())
            .collect())
    }
    pub fn credentials(&self, id: &PairId) -> Result<Credentials, Error> {
        let state = lock(&self.0.state);
        if state.closed {
            return Err(Error::Closed);
        }
        let entry = state.entries.get(id.as_str()).ok_or(Error::Revoked)?;
        let time = now()?;
        if entry
            .metadata
            .expires_at
            .is_some_and(|expiry| time >= expiry)
        {
            entry.cancel.cancel();
            return Err(Error::Expired);
        }
        if entry.cancel.is_cancelled() {
            return Err(Error::Revoked);
        }
        Ok(Credentials {
            id: id.clone(),
            secret: entry.secret.clone(),
            cancel: entry.cancel.clone(),
            owner: Arc::downgrade(&self.0),
        })
    }
    /// Call after a valid encrypted application frame, before admitting RPC work.
    pub fn confirm(&self, credentials: &Credentials) -> Result<PeerMetadata, Error> {
        let owner = credentials.owner.upgrade().ok_or(Error::Closed)?;
        if !Arc::ptr_eq(&owner, &self.0) || !credentials.is_live() {
            return Err(Error::Revoked);
        }
        let time = now()?;
        let mut state = lock(&self.0.state);
        if state.closed {
            return Err(Error::Closed);
        }
        let entry = state
            .entries
            .get(credentials.id.as_str())
            .ok_or(Error::Revoked)?;
        if !bool::from(entry.secret.bytes().ct_eq(credentials.secret.bytes())) {
            return Err(Error::Revoked);
        }
        if entry
            .metadata
            .expires_at
            .is_some_and(|expiry| time >= expiry)
        {
            entry.cancel.cancel();
            return Err(Error::Expired);
        }
        if entry.metadata.paired_at.is_some() {
            return Ok(entry.metadata.clone());
        }
        let mut entries = state.entries.clone();
        let metadata = &mut entries
            .get_mut(credentials.id.as_str())
            .ok_or(Error::Revoked)?
            .metadata;
        metadata.expires_at = None;
        metadata.paired_at = Some(time);
        let metadata = metadata.clone();
        self.commit(&mut state, entries)?;
        Ok(metadata)
    }
    pub fn revoke(&self, id: &PairId) -> Result<(), Error> {
        let mut state = lock(&self.0.state);
        if state.closed {
            return Err(Error::Closed);
        }
        if !state.entries.contains_key(id.as_str()) {
            return Ok(());
        }
        let mut entries = state.entries.clone();
        entries.remove(id.as_str());
        self.commit(&mut state, entries)
    }
    pub fn close(&self) {
        close(&mut lock(&self.0.state));
    }
}
