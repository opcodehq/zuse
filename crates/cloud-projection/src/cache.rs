//! Native ciphertext-only page cache. Decrypted transcripts never reach disk;
//! page keys live in `secure_cache` (the Keychain, or a private per-user store
//! where the Keychain is unavailable). A cached page only paints provisionally
//! until a fresh authorized read replaces it.
use crate::{Head, PAGE_LIMIT, digest};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedHead {
    workspace: String,
    session: String,
    identity: String,
    cursor: crate::Cursor,
    generation: i64,
    key: String,
    saved_at: u64,
}
impl Drop for SavedHead {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.key.zeroize();
    }
}
fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(0)
}
fn service(root: &Path) -> String {
    format!(
        "sh.zuse.cloud-history.{}",
        digest(root.to_string_lossy().as_bytes())
    )
}
pub struct PageCache {
    db: Mutex<Connection>,
    namespace: String,
    root: PathBuf,
    keys: Mutex<HashMap<String, (Zeroizing<String>, Instant)>>,
    heads: Mutex<HashMap<String, (Head, Instant)>>,
}
static CACHES: OnceLock<Mutex<HashMap<String, Arc<PageCache>>>> = OnceLock::new();

impl PageCache {
    pub fn shared(
        root: &Path,
        origin: &str,
        scope: Option<&str>,
        account: &str,
    ) -> Result<Arc<Self>> {
        let candidate = Self::open(root, origin, scope, account)?;
        let identity = digest(&serde_json::to_vec(&(
            root.to_string_lossy(),
            &candidate.namespace,
        ))?);
        let mut caches = CACHES.get_or_init(Default::default).lock().unwrap();
        if let Some(cache) = caches.get(&identity) {
            return Ok(cache.clone());
        }
        if caches.len() >= 8 {
            if let Some(old) = caches.keys().next().cloned() {
                caches.remove(&old);
            }
        }
        let cache = Arc::new(candidate);
        caches.insert(identity, cache.clone());
        Ok(cache)
    }
    pub fn open(root: &Path, origin: &str, scope: Option<&str>, account: &str) -> Result<Self> {
        ensure!(!account.is_empty(), "cache_account_required");
        std::fs::create_dir_all(root)?;
        let path = root.join("encrypted-pages.sqlite3");
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            ensure!(
                !std::fs::symlink_metadata(root)?.file_type().is_symlink(),
                "cache_directory_invalid"
            );
            std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))?;
            // O_NOFOLLOW is not portable; reject existing symlinks and use create_new
            // for the initial file. This cache lives in the user's private directory.
            if !path.exists() {
                let result = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path);
                if let Err(error) = result {
                    if error.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(error.into());
                    }
                }
            }
            ensure!(
                std::fs::symlink_metadata(&path)?.file_type().is_file(),
                "cache_file_invalid"
            );
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        let db = Connection::open(path)?;
        db.busy_timeout(std::time::Duration::from_millis(100))?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS encrypted_pages(namespace TEXT NOT NULL,identity TEXT NOT NULL,ciphertext TEXT NOT NULL,digest TEXT NOT NULL,PRIMARY KEY(namespace,identity))")?;
        let namespace = digest(&serde_json::to_vec(&(origin, scope, account))?);
        Ok(Self {
            db: Mutex::new(db),
            namespace,
            root: root.into(),
            keys: Mutex::new(HashMap::new()),
            heads: Mutex::new(HashMap::new()),
        })
    }

    fn identity(head: &Head, before: Option<i64>) -> String {
        digest(
            &serde_json::to_vec(&(
                &head.workspace_id,
                &head.session_id,
                head.runtime_generation,
                &head.cursor,
                before,
            ))
            .expect("serializable identity"),
        )
    }

    pub fn authorize(&self, head: &Head, token: &str, encoded_key: &str) {
        let mut keys = self.keys.lock().unwrap();
        keys.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(30));
        keys.insert(
            format!(
                "{}:{}",
                Self::identity(head, None),
                digest(token.as_bytes())
            ),
            (Zeroizing::new(encoded_key.to_owned()), Instant::now()),
        );
        if keys.len() > 64 {
            if let Some(old) = keys
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(key, _)| key.clone())
            {
                keys.remove(&old);
            }
        }
        drop(keys);
        let small = match &head.transcript.frame {
            crate::TranscriptFrame::Reset { reset } => {
                reset.len() <= 2048
                    && reset
                        .iter()
                        .flat_map(|entry| entry.parts.iter())
                        .try_fold(0usize, |n, part| {
                            n.checked_add(part.byte_len())
                                .filter(|n| *n <= 2 * 1024 * 1024)
                        })
                        .is_some()
            }
            _ => false,
        };
        if !small || head.conversations.len() > 256 {
            return;
        }
        self.persist_key(head, encoded_key);
        let mut heads = self.heads.lock().unwrap();
        heads.retain(|_, (_, at)| at.elapsed() < Duration::from_secs(30));
        if heads.len() >= 16 {
            if let Some(old) = heads
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(key, _)| key.clone())
            {
                heads.remove(&old);
            }
        }
        heads.insert(
            digest(
                &serde_json::to_vec(&(
                    &head.workspace_id,
                    &head.session_id,
                    digest(token.as_bytes()),
                ))
                .unwrap(),
            ),
            (head.clone(), Instant::now()),
        );
    }

    /// A persisted key permits provisional local painting only. It never installs
    /// a current authorization lease or permits page reuse/runtime execution.
    pub fn provisional_head(&self, workspace: &str, session: Option<&str>) -> Result<Option<Head>> {
        let Some(bytes) = crate::secure_cache::read(&service(&self.root), &self.namespace) else {
            return Ok(None);
        };
        let bytes = zeroize::Zeroizing::new(bytes);
        let saved: Vec<SavedHead> = serde_json::from_slice(&bytes)?;
        let now = now_seconds();
        let Some(saved) = saved
            .iter()
            .filter(|v| {
                v.workspace == workspace
                    && session.is_none_or(|s| v.session == s)
                    && now >= v.saved_at
                    && now - v.saved_at <= 7 * 86400
            })
            .max_by_key(|v| v.saved_at)
        else {
            return Ok(None);
        };
        let row:Option<(String,String)>=self.db.lock().unwrap().query_row("SELECT ciphertext,digest FROM encrypted_pages WHERE namespace=?1 AND identity=?2 AND length(ciphertext)<=?3",params![self.namespace,saved.identity,2*PAGE_LIMIT],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        let Some((ciphertext, hash)) =
            row.filter(|(ciphertext, hash)| digest(ciphertext.as_bytes()) == *hash)
        else {
            return Ok(None);
        };
        let _ = hash;
        let head: Head = crate::TranscriptKey::parse(&saved.key)?.open(
            workspace,
            &saved.session,
            &saved.cursor,
            None,
            &ciphertext,
        )?;
        ensure!(
            head.workspace_id == workspace
                && head.session_id == saved.session
                && head.cursor == saved.cursor
                && head.runtime_generation == saved.generation
                && Self::identity(&head, None) == saved.identity,
            "cache_head_scope_mismatch"
        );
        Ok(Some(head))
    }
    fn persist_key(&self, head: &Head, key: &str) {
        let service = service(&self.root);
        let mut saved = crate::secure_cache::read(&service, &self.namespace)
            .map(zeroize::Zeroizing::new)
            .and_then(|data| serde_json::from_slice::<Vec<SavedHead>>(&data).ok())
            .unwrap_or_default();
        let now = now_seconds();
        saved.retain(|v| {
            !(v.workspace == head.workspace_id && v.session == head.session_id)
                && now >= v.saved_at
                && now - v.saved_at <= 7 * 86400
        });
        saved.sort_by_key(|v| v.saved_at);
        if saved.len() >= 16 {
            saved.remove(0);
        }
        saved.push(SavedHead {
            workspace: head.workspace_id.clone(),
            session: head.session_id.clone(),
            identity: Self::identity(head, None),
            cursor: head.cursor.clone(),
            generation: head.runtime_generation,
            key: key.into(),
            saved_at: now,
        });
        if let Ok(bytes) = serde_json::to_vec(&saved) {
            let bytes = zeroize::Zeroizing::new(bytes);
            crate::secure_cache::write(&service, &self.namespace, &bytes);
        }
    }
    pub fn cached_head(&self, workspace: &str, session: &str, token: &str) -> Result<Option<Head>> {
        let identity = digest(&serde_json::to_vec(&(
            workspace,
            session,
            digest(token.as_bytes()),
        ))?);
        let head = self
            .heads
            .lock()
            .unwrap()
            .get(&identity)
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(30))
            .map(|(head, _)| head.clone());
        let Some(head) = head else {
            return Ok(None);
        };
        let Some((ciphertext, key)) = self.read_record(&head, None, token)? else {
            return Ok(None);
        };
        let decoded: Head = crate::TranscriptKey::parse(&key)?.open(
            workspace,
            session,
            &head.cursor,
            None,
            &ciphertext,
        )?;
        ensure!(
            decoded.workspace_id == workspace
                && decoded.session_id == session
                && decoded.cursor == head.cursor
                && decoded.runtime_generation == head.runtime_generation,
            "cache_head_scope_mismatch"
        );
        Ok(Some(decoded))
    }
    pub fn write_head(&self, head: &Head, ciphertext: &str) -> Result<()> {
        self.write_record(head, None, ciphertext)
    }

    pub fn read(
        &self,
        head: &Head,
        before: i64,
        token: &str,
    ) -> Result<Option<(String, Zeroizing<String>)>> {
        self.read_record(head, Some(before), token)
    }
    fn read_record(
        &self,
        head: &Head,
        before: Option<i64>,
        token: &str,
    ) -> Result<Option<(String, Zeroizing<String>)>> {
        let keys = self.keys.lock().unwrap();
        let Some((key, _)) = keys
            .get(&format!(
                "{}:{}",
                Self::identity(head, None),
                digest(token.as_bytes())
            ))
            .filter(|(_, at)| at.elapsed() < Duration::from_secs(30))
        else {
            return Ok(None);
        };
        let row: Option<(String, String)> = self.db.lock().unwrap().query_row(
            "SELECT ciphertext,digest FROM encrypted_pages WHERE namespace=?1 AND identity=?2 AND length(ciphertext)<=?3",
            params![self.namespace, Self::identity(head, before), (2 * PAGE_LIMIT) as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        Ok(row
            .filter(|(ciphertext, hash)| digest(ciphertext.as_bytes()) == *hash)
            .map(|(ciphertext, _)| (ciphertext, Zeroizing::new(key.to_string()))))
    }

    pub fn write(&self, head: &Head, before: i64, ciphertext: &str) -> Result<()> {
        self.write_record(head, Some(before), ciphertext)
    }
    fn write_record(&self, head: &Head, before: Option<i64>, ciphertext: &str) -> Result<()> {
        ensure!(ciphertext.len() <= 2 * PAGE_LIMIT, "cache_page_too_large");
        let db = self.db.lock().unwrap();
        db.execute(
            "INSERT INTO encrypted_pages(namespace,identity,ciphertext,digest) VALUES(?1,?2,?3,?4) ON CONFLICT(namespace,identity) DO UPDATE SET ciphertext=excluded.ciphertext,digest=excluded.digest",
            params![self.namespace, Self::identity(head, before), ciphertext, digest(ciphertext.as_bytes())],
        )?;
        db.execute("DELETE FROM encrypted_pages WHERE rowid IN (SELECT rowid FROM (SELECT rowid,SUM(length(ciphertext)) OVER(ORDER BY rowid DESC) AS bytes FROM encrypted_pages) WHERE bytes>67108864)",[])?;
        db.execute("DELETE FROM encrypted_pages WHERE rowid IN (SELECT rowid FROM encrypted_pages ORDER BY rowid DESC LIMIT -1 OFFSET 256)",[])?;
        Ok(())
    }
    pub fn purge(&self) -> Result<()> {
        crate::secure_cache::clear(&service(&self.root), Some(&self.namespace));
        self.keys.lock().unwrap().clear();
        self.heads.lock().unwrap().clear();
        self.db.lock().unwrap().execute(
            "DELETE FROM encrypted_pages WHERE namespace=?1",
            [&self.namespace],
        )?;
        Ok(())
    }
    pub fn purge_root(root: &Path) -> Result<()> {
        crate::secure_cache::clear(&service(root), None);
        if let Some(registry) = CACHES.get() {
            let mut caches = registry.lock().unwrap();
            for cache in caches.values().filter(|cache| cache.root == root) {
                cache.purge()?;
            }
            caches.retain(|_, cache| cache.root != root);
        }
        let path = root.join("encrypted-pages.sqlite3");
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        ensure!(metadata.file_type().is_file(), "cache_file_invalid");
        let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        db.busy_timeout(Duration::from_millis(100))?;
        db.execute("DELETE FROM encrypted_pages", [])?;
        Ok(())
    }
}
