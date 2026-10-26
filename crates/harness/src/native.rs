//! Native root ownership and durable storage composition.
//!
//! Take the OS lease before reading a root journal and keep it until the host
//! has stopped its processes. A PID stamp is diagnostic, never lock authority.
use crate::{journal::JournalStore, *};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub struct RootStore {
    directory: PathBuf,
    root_id: String,
    store: JournalStore,
    gate: Arc<tokio::sync::Mutex<()>>,
    owner: File,
}
impl RootStore {
    pub fn acquire(profile: &Path, root_id: &str) -> Result<Self, Error> {
        let digest = format!("{:x}", Sha256::digest(root_id.as_bytes()));
        let directory = profile.join("harness").join(digest);
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
        let mut owner = options
            .open(directory.join("owner.lock"))
            .map_err(|_| Error::Storage)?;
        owner.try_lock().map_err(|error|match error {
            std::fs::TryLockError::WouldBlock=>Error::paused("Another runtime owns this Zuse conversation. Close it before recovering this journal."),
            std::fs::TryLockError::Error(_)=>Error::Storage,
        })?;
        let _ = owner.set_len(0);
        let _ = write!(owner, "{}", std::process::id());
        let _ = owner.flush();
        Ok(Self {
            root_id: root_id.into(),
            store: JournalStore::new(&directory).for_root(root_id),
            gate: Arc::new(tokio::sync::Mutex::new(())),
            directory,
            owner,
        })
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn load(&self) -> Result<Option<Checkpoint>, Error> {
        let state = self.store.load()?;
        if state
            .as_ref()
            .is_some_and(|state| state.root_id != self.root_id)
        {
            return Err(Error::InvalidCheckpoint);
        }
        Ok(state)
    }
    pub fn save(&self, state: &Checkpoint) -> Result<(), Error> {
        if state.root_id != self.root_id {
            return Err(Error::InvalidCheckpoint);
        }
        self.store.save(state)
    }
}
impl Drop for RootStore {
    fn drop(&mut self) {
        // Explicit unlock also handles forked children that have not exec'd yet.
        let _ = self.owner.unlock();
    }
}
struct NativeHost {
    inner: Arc<dyn Host>,
    store: Mutex<Option<Arc<RootStore>>>,
}
impl NativeHost {
    fn storage(&self) -> std::sync::MutexGuard<'_, Option<Arc<RootStore>>> {
        self.store.lock().unwrap_or_else(|error| error.into_inner())
    }
}
#[async_trait]
impl Host for NativeHost {
    fn tools(&self) -> Tools {
        self.inner.tools()
    }
    async fn load(&self) -> Result<Option<Checkpoint>, Error> {
        let root = self.storage().as_ref().ok_or(Error::Storage)?.clone();
        let guard = root.gate.clone().lock_owned().await;
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            root.load()
        })
        .await
        .map_err(|_| Error::Storage)?
    }
    async fn save(&self, state: &Checkpoint) -> Result<(), Error> {
        let root = self.storage().as_ref().ok_or(Error::Storage)?.clone();
        let guard = root.gate.clone().lock_owned().await;
        if state.root_id != root.root_id {
            return Err(Error::InvalidCheckpoint);
        }
        let Some(prepared) = root.store.prepare(state)? else {
            return Ok(());
        };
        // The worker owns both lease and admission guard if its caller drops.
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            root.store.commit(prepared)
        })
        .await
        .map_err(|_| Error::Storage)?
    }
    async fn instructions(&self) -> Result<String, Error> {
        self.inner.instructions().await
    }
    async fn model(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        self.inner.model(input).await
    }
    async fn execute(
        &self,
        name: &str,
        input: serde_json::Value,
        context: ToolContext,
    ) -> Result<ToolOutput, Error> {
        self.inner.execute(name, input, context).await
    }
    fn notify(&self, event: Event) {
        self.inner.notify(event);
    }
    async fn interrupt_agents(&self, ids: &[String]) -> Result<(), Error> {
        self.inner.interrupt_agents(ids).await
    }
    async fn close(&self) -> Result<(), Error> {
        let result = self.inner.close().await;
        // If close is cancelled, retain ownership until the host is dropped.
        // A second runtime must not recover processes still being cleaned up.
        if result.is_ok() {
            let root = self.storage().as_ref().cloned();
            if let Some(root) = root {
                let _drained = root.gate.lock().await;
                self.storage().take();
            }
        }
        result
    }
}
impl Engine {
    /// Native application entry point. The host supplies tools, model transport,
    /// instructions and events; this composition owns its journal and root lease.
    pub async fn open_native(
        profile: &Path,
        root_id: impl Into<String>,
        options: Options,
        host: Arc<dyn Host>,
    ) -> Result<Self, Error> {
        let root_id = root_id.into();
        let store = RootStore::acquire(profile, &root_id)?;
        Self::open_with_store(root_id, options, host, store).await
    }
    /// Construct native tools only after ownership is held. The factory receives
    /// the private root directory for artifacts; it must not start model work.
    pub async fn open_native_with<F, Fut>(
        profile: &Path,
        root_id: impl Into<String>,
        options: Options,
        create: F,
    ) -> Result<Self, Error>
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: std::future::Future<Output = Result<Arc<dyn Host>, Error>>,
    {
        let root_id = root_id.into();
        let store = RootStore::acquire(profile, &root_id)?;
        Self::open_native_owned(store, options, create).await
    }
    /// Continue from a journal inspected under its OS ownership lease. Ownership
    /// stays held through tool construction and runtime cleanup.
    pub async fn open_native_owned<F, Fut>(
        store: RootStore,
        options: Options,
        create: F,
    ) -> Result<Self, Error>
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: std::future::Future<Output = Result<Arc<dyn Host>, Error>>,
    {
        let root_id = store.root_id.clone();
        let host = create(store.directory().to_path_buf()).await?;
        Self::open_with_store(root_id, options, host, store).await
    }
    async fn open_with_store(
        root_id: String,
        options: Options,
        host: Arc<dyn Host>,
        store: RootStore,
    ) -> Result<Self, Error> {
        let host = Arc::new(NativeHost {
            inner: host,
            store: Mutex::new(Some(Arc::new(store))),
        });
        match Self::open(root_id, options, host.clone()).await {
            Ok(engine) => Ok(engine),
            Err(error) => {
                let _ = host.close().await;
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn root_ownership_blocks_other_processes_and_survives_stale_pid_stamps() {
        let directory = tempfile::tempdir().unwrap();
        let owner = RootStore::acquire(directory.path(), "root/with/untrusted/path").unwrap();
        assert!(
            owner
                .directory()
                .starts_with(directory.path().join("harness"))
        );
        assert!(matches!(
            RootStore::acquire(directory.path(), "root/with/untrusted/path"),
            Err(Error::Paused(_))
        ));
        let other = RootStore::acquire(directory.path(), "other-root").unwrap();
        drop(other);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native::tests::ownership_subprocess_probe",
                "--nocapture",
            ])
            .env("ZUSE_TEST_HARNESS_OWNERSHIP", directory.path())
            .status()
            .unwrap();
        assert!(status.success());
        let lock = owner.directory().join("owner.lock");
        drop(owner);
        std::fs::write(lock, b"999999999").unwrap();
        // PID text is stale and does not prevent the next owner from taking over.
        let recovered = RootStore::acquire(directory.path(), "root/with/untrusted/path").unwrap();
        assert!(recovered.load().unwrap().is_none());
        std::fs::write(
            recovered.directory().join("checkpoint.json"),
            b"damaged journal",
        )
        .unwrap();
        assert!(matches!(recovered.load(), Err(Error::InvalidCheckpoint)));
        assert_eq!(
            std::fs::read(recovered.directory().join("checkpoint.json")).unwrap(),
            b"damaged journal"
        );
    }
    struct ClosingHost {
        started: tokio::sync::Notify,
        finish: tokio::sync::Notify,
    }
    #[async_trait]
    impl Host for ClosingHost {
        fn tools(&self) -> Tools {
            Tools::new()
        }
        async fn load(&self) -> Result<Option<Checkpoint>, Error> {
            panic!("native journal must own storage")
        }
        async fn save(&self, _: &Checkpoint) -> Result<(), Error> {
            panic!("native journal must own storage")
        }
        async fn instructions(&self) -> Result<String, Error> {
            Ok(String::new())
        }
        async fn model(&self, _: ModelInput) -> Result<ModelOutput, Error> {
            Err(Error::AccountsUnavailable)
        }
        async fn execute(
            &self,
            _: &str,
            _: serde_json::Value,
            _: ToolContext,
        ) -> Result<ToolOutput, Error> {
            Err(Error::Tool("not used".into()))
        }
        fn notify(&self, _: Event) {}
        async fn close(&self) -> Result<(), Error> {
            self.started.notify_one();
            self.finish.notified().await;
            Ok(())
        }
    }
    #[tokio::test]
    async fn native_runtime_holds_ownership_until_host_cleanup_finishes() {
        let directory = tempfile::tempdir().unwrap();
        let host = Arc::new(ClosingHost {
            started: tokio::sync::Notify::new(),
            finish: tokio::sync::Notify::new(),
        });
        let factory_host = host.clone();
        let profile = directory.path().to_path_buf();
        let engine = Engine::open_native_with(
            directory.path(),
            "root",
            Options::new("chatgpt/fixture"),
            move |artifacts| async move {
                assert!(artifacts.starts_with(profile.join("harness")));
                assert!(matches!(
                    RootStore::acquire(&profile, "root"),
                    Err(Error::Paused(_))
                ));
                Ok(factory_host as Arc<dyn Host>)
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            RootStore::acquire(directory.path(), "root"),
            Err(Error::Paused(_))
        ));
        let close = {
            let engine = engine.clone();
            tokio::spawn(async move { engine.close().await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(3), host.started.notified())
            .await
            .unwrap();
        assert!(matches!(
            RootStore::acquire(directory.path(), "root"),
            Err(Error::Paused(_))
        ));
        host.finish.notify_one();
        close.await.unwrap().unwrap();
        let store = RootStore::acquire(directory.path(), "root").unwrap();
        let mut state = store.load().unwrap().unwrap();
        assert_eq!(state.root_id, "root");
        state.root_id = "other-root".into();
        assert!(matches!(store.save(&state), Err(Error::InvalidCheckpoint)));
        assert_eq!(store.load().unwrap().unwrap().root_id, "root");
    }
    #[test]
    fn ownership_subprocess_probe() {
        let Some(directory) = std::env::var_os("ZUSE_TEST_HARNESS_OWNERSHIP") else {
            return;
        };
        assert!(matches!(
            RootStore::acquire(Path::new(&directory), "root/with/untrusted/path"),
            Err(Error::Paused(_))
        ));
    }
}
