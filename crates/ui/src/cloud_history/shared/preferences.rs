//! Preferences belong to the API/account scope, rather than one task window.
use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use zuse_protocol::presentation::WorkspacePreferences;

const MAX_BYTES: u64 = 64 * 1024;
#[derive(Clone)]
pub(super) struct Store {
    root: Option<PathBuf>,
    active: Arc<AtomicBool>,
    revision: Arc<AtomicU64>,
    gate: Arc<Mutex<()>>,
}
impl Store {
    pub(super) fn new(target: &CloudTarget, seed: &Value) -> Self {
        use sha2::{Digest, Sha256};
        let root = seed["apiOrigin"]
            .as_str()
            .filter(|origin| {
                origin.starts_with("https://") || origin.starts_with("http://127.0.0.1")
            })
            .and_then(|origin| {
                let origin = url::Url::parse(origin).ok()?.to_string();
                std::env::var_os("HOME").map(|home| {
                    PathBuf::from(home)
                        .join(".zuse/cache/cloud-view-preferences")
                        .join(format!(
                            "{:x}",
                            Sha256::digest(
                                json!([origin, target.subject, target.organization])
                                    .to_string()
                                    .as_bytes()
                            )
                        ))
                })
            });
        Self {
            root,
            active: Arc::new(AtomicBool::new(true)),
            revision: Default::default(),
            gate: Default::default(),
        }
    }
    pub(super) fn next(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn path(&self) -> Option<PathBuf> {
        if !self.active.load(Ordering::SeqCst) {
            return None;
        }
        let root = self.root.as_ref()?;
        if let Ok(metadata) = std::fs::symlink_metadata(root) {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return None;
            }
        }
        Some(root.join("preferences.json"))
    }
    pub(super) fn load(&self) -> Option<Value> {
        let _guard = self.gate.lock().ok()?;
        let path = self.path()?;
        let metadata = std::fs::symlink_metadata(&path).ok()?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_BYTES {
            return None;
        }
        let mut value: WorkspacePreferences =
            serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
        value.normalize();
        serde_json::to_value(value).ok()
    }
    pub(super) fn save(&self, value: Value, revision: u64) {
        let Ok(mut value) = serde_json::from_value::<WorkspacePreferences>(value) else {
            return;
        };
        value.normalize();
        let Ok(bytes) = serde_json::to_vec(&value) else {
            return;
        };
        if bytes.len() as u64 > MAX_BYTES {
            return;
        }
        let Ok(_guard) = self.gate.lock() else {
            return;
        };
        let Some(path) = self.path() else {
            return;
        };
        if revision != self.revision.load(Ordering::SeqCst) {
            return;
        }
        let root = path.parent().unwrap();
        if std::fs::create_dir_all(root).is_err() {
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700)).is_err() {
                return;
            }
        }
        let temp = root.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            if self.active.load(Ordering::SeqCst)
                && revision == self.revision.load(Ordering::SeqCst)
            {
                std::fs::rename(&temp, &path)?;
            }
            Ok(())
        })();
        let _ = result;
        let _ = std::fs::remove_file(temp);
    }
    pub(super) fn purge(&self) {
        self.active.store(false, Ordering::SeqCst);
        let Ok(_guard) = self.gate.lock() else {
            return;
        };
        if let Some(root) = &self.root {
            let _ = std::fs::remove_file(root.join("preferences.json"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preferences_normalize_fence_old_writes_and_revoke_private_copy() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store {
            root: Some(temp.path().join("scoped")),
            active: Arc::new(AtomicBool::new(true)),
            revision: Default::default(),
            gate: Default::default(),
        };
        let old = store.next();
        let current = store.next();
        store.save(
            json!({"cloudProvider":"boxd","cloudSizes":{"boxd":"small"},"sidebarWidth":900}),
            current,
        );
        store.save(json!({"cloudProvider":"boat"}), old);
        let loaded = store.load().unwrap();
        assert_eq!(loaded["cloudProvider"], "boxd");
        assert_eq!(loaded["sidebarWidth"], 300);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(temp.path().join("scoped/preferences.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        store.purge();
        store.save(json!({"cloudProvider":"boat"}), store.next());
        assert!(store.load().is_none());
        assert!(!temp.path().join("scoped/preferences.json").exists());
    }
}
