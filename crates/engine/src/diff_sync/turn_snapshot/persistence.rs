//! One private, self-contained latest-turn pack per chat; no checkout objects written.
use super::*;
use crate::diff_sync::TurnSnapshot;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

const SAVED: &str = "saved";
const MAX_RECORD: u64 = 16 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    version: u32,
    chat_id: String,
    root: PathBuf,
    repository_objects: PathBuf,
    store_id: String,
    tree: String,
    skipped: Vec<Vec<u8>>,
    at: chrono::DateTime<chrono::Utc>,
}
fn saved_root(data: &Path) -> PathBuf {
    data.join(STORE_DIR).join(SAVED)
}
fn record_path(data: &Path, chat: &str) -> PathBuf {
    saved_root(data).join(format!("{:x}.json", Sha256::digest(chat.as_bytes())))
}
fn fail(message: &str) -> EngineError {
    EngineError::Other(message.into())
}
fn private_dir(path: &Path) -> Result<(), EngineError> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn read_record(data: &Path, chat: &str) -> Result<Option<Record>, EngineError> {
    let path = record_path(data, chat);
    let metadata = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
        Ok(m) => m,
    };
    if !metadata.is_file() || metadata.len() > MAX_RECORD {
        return Err(fail(
            "Latest-turn metadata is invalid; the file was preserved.",
        ));
    }
    let r: Record = serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|_| fail("Latest-turn metadata is damaged; the file was preserved."))?;
    if r.version != 1
        || r.chat_id != chat
        || !r.root.is_absolute()
        || !r.repository_objects.is_absolute()
        || uuid::Uuid::parse_str(&r.store_id).is_err()
        || !matches!(r.tree.len(), 40 | 64)
        || !r.tree.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(fail(
            "Latest-turn metadata has an invalid identity; the file was preserved.",
        ));
    }
    Ok(Some(r))
}
pub(in crate::diff_sync) fn load(
    data: &Path,
    chat: &str,
) -> Result<Option<TurnSnapshot>, EngineError> {
    let Some(r) = read_record(data, chat)? else {
        return Ok(None);
    };
    let dir = std::path::absolute(saved_root(data).join("objects").join(&r.store_id))?;
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir())
        || !dir.join("objects/pack").is_dir()
    {
        return Err(fail(
            "Latest-turn objects are missing; the metadata was preserved.",
        ));
    }
    Ok(Some(TurnSnapshot {
        root: r.root,
        at: r.at,
        tree: TreeSnapshot {
            tree: r.tree,
            skipped: Arc::new(r.skipped),
            store: Arc::new(ObjectStore {
                dir,
                alternates: alternates_value(&[&r.repository_objects]),
                _base: None,
                persistent: AtomicBool::new(true),
            }),
        },
    }))
}
pub(in crate::diff_sync) fn forget(data: &Path, chat: &str) -> Result<(), EngineError> {
    // Refuse to overwrite malformed records with a new capture.
    if read_record(data, chat)?.is_some() {
        std::fs::remove_file(record_path(data, chat))?;
        #[cfg(unix)]
        std::fs::File::open(saved_root(data))?.sync_all()?;
    }
    Ok(())
}
impl TreeSnapshot {
    pub(in crate::diff_sync) fn retire(&self) {
        self.store.persistent.store(false, Ordering::Release);
    }
    pub(in crate::diff_sync) fn skipped_path(&self, path: &str) -> bool {
        let p = path.as_bytes();
        self.skipped.iter().any(|skip| {
            p == skip
                || p.strip_prefix(skip.as_slice())
                    .is_some_and(|tail| tail.starts_with(b"/"))
        })
    }
    pub(in crate::diff_sync) async fn persist_latest(
        &self,
        data: &Path,
        chat: &str,
        root: &Path,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Self, EngineError> {
        // Corrupt metadata remains available for recovery, even after a later turn.
        let _ = read_record(data, chat)?;
        let parent = std::path::absolute(saved_root(data))?;
        private_dir(&parent)?;
        private_dir(&parent.join("objects"))?;
        let id = uuid::Uuid::new_v4().to_string();
        let dir = parent.join("objects").join(&id);
        private_dir(&dir)?;
        private_dir(&dir.join("objects"))?;
        private_dir(&dir.join("objects/pack"))?;
        let objects = capture_git(
            root,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "objects",
            ],
            16 * 1024,
        )
        .await?;
        let repository_objects = PathBuf::from(path_argument(
            objects
                .stdout
                .strip_suffix(b"\n")
                .unwrap_or(&objects.stdout),
        ));
        let store = Arc::new(ObjectStore {
            dir,
            alternates: alternates_value(&[&repository_objects]),
            _base: None,
            persistent: AtomicBool::new(false),
        });
        // A full non-thin pack owns every tree/blob, so checkout pruning cannot
        // invalidate it. Git streams to disk; disable delta windows to bound RAM.
        let prefix = store.objects_dir().join("pack/snapshot");
        let packed = async {
            let mut cmd = tokio::process::Command::new("git");
            cmd.current_dir(root)
                .args([
                    "-c",
                    "pack.threads=1",
                    "-c",
                    "pack.windowMemory=16m",
                    "pack-objects",
                    "--revs",
                    "--window=0",
                ])
                .arg(&prefix)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true);
            self.apply_to(&mut cmd);
            let mut child = cmd.spawn()?;
            let mut input = child.stdin.take().unwrap();
            input
                .write_all(format!("{}\n", self.tree()).as_bytes())
                .await?;
            drop(input);
            if !child.wait().await?.success() {
                return Err(fail("Could not preserve latest-turn objects."));
            }
            Ok::<_, EngineError>(())
        };
        tokio::time::timeout(SNAPSHOT_TIMEOUT, packed)
            .await
            .map_err(|_| fail("Preserving latest-turn objects timed out."))??;
        let pack_dir = store.objects_dir().join("pack");
        for entry in std::fs::read_dir(&pack_dir)? {
            std::fs::File::open(entry?.path())?.sync_all()?;
        }
        #[cfg(unix)]
        for path in [
            &pack_dir,
            &store.objects_dir(),
            &store.dir,
            &parent.join("objects"),
        ] {
            std::fs::File::open(path)?.sync_all()?;
        }
        let record = Record {
            version: 1,
            chat_id: chat.into(),
            root: root.into(),
            repository_objects,
            store_id: id,
            tree: self.tree.clone(),
            skipped: (*self.skipped).clone(),
            at,
        };
        let bytes = serde_json::to_vec(&record)
            .map_err(|_| fail("Could not encode latest-turn metadata."))?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(fail("Latest-turn metadata exceeds its storage limit."));
        }
        let mut file = tempfile::NamedTempFile::new_in(&parent)?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(record_path(data, chat))
            .map_err(|e| EngineError::Other(e.error.to_string()))?;
        // No await between publishing the durable pointer and retaining its store.
        store.persistent.store(true, Ordering::Release);
        #[cfg(unix)]
        std::fs::File::open(&parent)?.sync_all()?;
        Ok(Self {
            tree: self.tree.clone(),
            store,
            skipped: self.skipped.clone(),
        })
    }
}
/// Runs before turn admission is wired. Unknown/corrupt records inhibit collection.
pub(in crate::diff_sync) fn sweep_saved(data: &Path) {
    let parent = saved_root(data);
    let Ok(entries) = std::fs::read_dir(&parent) else {
        return;
    };
    let mut keep = std::collections::HashSet::new();
    for entry in entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|e| e == "json"))
    {
        let Ok(metadata) = entry.metadata() else {
            return;
        };
        if metadata.len() > MAX_RECORD {
            return;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            return;
        };
        let Ok(r) = serde_json::from_slice::<Record>(&bytes) else {
            return;
        };
        if r.version != 1 || uuid::Uuid::parse_str(&r.store_id).is_err() {
            return;
        }
        keep.insert(r.store_id);
    }
    let Ok(objects) = std::fs::read_dir(parent.join("objects")) else {
        return;
    };
    for entry in objects.flatten() {
        if !keep.contains(&entry.file_name().to_string_lossy().to_string()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}
