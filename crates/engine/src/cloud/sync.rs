//! Host-owned local mirror configuration and lifecycle. No cached read wakes compute.
use super::*;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use zuse_protocol::{WorkspaceTarget, methods};
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Config {
    version: u32,
    account_id: String,
    workspace_id: String,
    enabled: bool,
    local_path: PathBuf,
    session_id: String,
    #[serde(default)]
    roots: Vec<PathBuf>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    workspace_id: String,
    enabled: bool,
    state: String,
    local_path: Option<PathBuf>,
    last_synced_at: Option<i64>,
    files: usize,
    transferred_bytes: u64,
    error: Option<String>,
}
impl Status {
    fn empty(workspace: &str) -> Self {
        Self {
            workspace_id: workspace.into(),
            enabled: false,
            state: "idle".into(),
            local_path: None,
            last_synced_at: None,
            files: 0,
            transferred_bytes: 0,
            error: None,
        }
    }
    fn configured(config: &Config) -> Self {
        Self {
            enabled: config.enabled,
            local_path: Some(config.local_path.clone()),
            state: if config.enabled { "waiting" } else { "idle" }.into(),
            ..Self::empty(&config.workspace_id)
        }
    }
}
struct Record {
    config: Config,
    file: PathBuf,
    status: Status,
    cancel: CancellationToken,
    wake: Arc<Notify>,
    task: Option<tokio::task::JoinHandle<()>>,
}
#[derive(Default)]
pub(super) struct Manager {
    records: Mutex<HashMap<(String, String), Record>>,
    loaded: Mutex<HashSet<(String, PathBuf)>>,
    changes: tokio::sync::Mutex<()>,
    reconciler: Mutex<Option<super::MirrorReconciler>>,
}
fn key(account: &str, workspace: &str) -> (String, String) {
    (account.into(), workspace.into())
}
fn hashed(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn directory(auth: &Auth, account: &str) -> PathBuf {
    auth.profile_dir().join("cloud-sync").join(hashed(account))
}
fn save(file: &Path, config: &Config) -> Result<()> {
    let parent = file.parent().context("Invalid sync settings path.")?;
    fs::create_dir_all(parent)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    let bytes = serde_json::to_vec(config)?;
    ensure!(
        bytes.len() <= 1024 * 1024,
        "Cloud sync settings exceed their supported size."
    );
    temporary.write_all(&bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(file).map_err(|error| error.error)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
pub(super) fn authorized(auth: &Auth, account: &str) -> bool {
    matches!(auth.state(),crate::auth::AuthState::SignedIn{user,org_id:None} if user.id==account)
}
impl Manager {
    pub(super) fn ensure_reconciliation(&self, auth: Auth) {
        if !auth.state().is_signed_in() {
            return;
        }
        let mut task = lock(&self.reconciler);
        if task.is_none() {
            *task = Some(super::start_mirror_reconciliation(auth));
        }
    }
    pub(super) fn hydrate(&self, api: &CloudApi, auth: &Auth, account: &str) -> Result<()> {
        let profile = (account.to_owned(), auth.profile_dir().to_owned());
        let mut loaded = lock(&self.loaded);
        if loaded.contains(&profile) {
            return Ok(());
        }
        let directory = directory(auth, account);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(entries) = entries {
            for file in entries {
                let file = file?;
                if file.path().extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                ensure!(
                    file.file_type()?.is_file() && file.metadata()?.len() <= 1024 * 1024,
                    "Invalid cloud sync settings file."
                );
                let config: Config = serde_json::from_slice(&fs::read(file.path())?)?;
                ensure!(
                    config.version == 1
                        && config.account_id == account
                        && !config.workspace_id.is_empty()
                        && config.workspace_id.len() <= 512
                        && config.local_path.is_absolute()
                        && config.roots.iter().all(|p| p.is_absolute()),
                    "Invalid cloud sync configuration."
                );
                ensure!(
                    file.file_name().to_str()
                        == Some(format!("{}.json", hashed(&config.workspace_id)).as_str()),
                    "Cloud sync configuration identity changed."
                );
                for root in config
                    .roots
                    .iter()
                    .chain(std::iter::once(&config.local_path))
                {
                    api.mirrors
                        .register_root(account, &config.workspace_id, root.clone());
                }
                let mut records = lock(&self.records);
                records
                    .entry(key(account, &config.workspace_id))
                    .or_insert_with(|| Record {
                        status: Status::configured(&config),
                        config,
                        file: file.path(),
                        cancel: CancellationToken::new(),
                        wake: Arc::new(Notify::new()),
                        task: None,
                    });
            }
        }
        loaded.insert(profile);
        Ok(())
    }
    pub(super) fn local_root(&self, account: &str, workspace: &str) -> Option<PathBuf> {
        lock(&self.records)
            .get(&key(account, workspace))
            .map(|r| r.config.local_path.clone())
    }
    fn status(&self, account: &str, workspace: &str) -> Status {
        lock(&self.records)
            .get(&key(account, workspace))
            .map(|r| r.status.clone())
            .unwrap_or_else(|| Status::empty(workspace))
    }
    pub(super) fn observe(&self, account: &str, row: &Workspace) {
        if !row.archived()
            && !row.deleting()
            && !matches!(row.state.as_str(), "deleted" | "deleting")
        {
            return;
        }
        let mut records = lock(&self.records);
        let Some(record) = records.get_mut(&key(account, &row.workspace_id)) else {
            return;
        };
        record.cancel.cancel();
        record.config.enabled = false;
        record.status.enabled = false;
        record.status.state = if record.task.is_some() {
            "stopping"
        } else {
            "idle"
        }
        .into();
        if let Err(error) = save(&record.file, &record.config) {
            record.status.error = Some(format!("Could not save disabled sync settings: {error}"));
            record.status.state = "error".into();
        }
        if row.state == "deleted"
            && row
                .local_mirror_cleanup
                .as_ref()
                .is_some_and(|c| c.state == "complete")
            && record.task.is_none()
        {
            if fs::remove_file(&record.file).is_ok() {
                records.remove(&key(account, &row.workspace_id));
            }
        }
    }
    fn update(
        &self,
        account: &str,
        workspace: &str,
        cancel: &CancellationToken,
        update: impl FnOnce(&mut Record),
    ) {
        if let Some(record) = lock(&self.records).get_mut(&key(account, workspace)) {
            // Configurations are serialized and join their previous worker.
            if &record.cancel == cancel {
                update(record);
            }
        }
    }
    fn start(api: Arc<CloudApi>, auth: Auth, account: String, workspace: String) {
        if (!auth.workos_enabled() && !api.loopback) || !authorized(&auth, &account) {
            return;
        }
        let mut records = lock(&api.syncs.records);
        let Some(record) = records.get_mut(&key(&account, &workspace)) else {
            return;
        };
        if !record.config.enabled || record.task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        record.cancel = CancellationToken::new();
        let cancel = record.cancel.clone();
        let wake = record.wake.clone();
        let config = record.config.clone();
        record.status.state = "pending".into();
        record.status.error = None;
        let service = api.clone();
        record.task = Some(tokio::spawn(async move {
            run(service, auth, config, cancel, wake).await;
        }));
    }
}
async fn workspace(api: &CloudApi, auth: &Auth, account: &str, id: &str) -> Result<Value> {
    ensure!(
        auth.workos_enabled() || api.loopback,
        "Sign in to access cloud sync."
    );
    ensure!(
        authorized(auth, account),
        "The account changed. Select the workspace again."
    );
    let token = auth
        .access_token()
        .await
        .map_err(|_| anyhow::anyhow!("Sign in to access cloud sync."))?;
    ensure!(
        authorized(auth, account),
        "The account changed. Select the workspace again."
    );
    let value = api
        .request(&token, &["v1", "cloud", "workspaces", id], None)
        .await?;
    ensure!(
        authorized(auth, account),
        "The account changed. Select the workspace again."
    );
    let value = value.get("workspace").unwrap_or(&value).clone();
    ensure!(
        value["workspaceId"].as_str() == Some(id),
        "Cloud workspace identity changed."
    );
    Ok(value)
}
async fn run(
    api: Arc<CloudApi>,
    auth: Auth,
    config: Config,
    cancel: CancellationToken,
    wake: Arc<Notify>,
) {
    let mut changes = auth.watch_state();
    let watched = auth.clone();
    let owner = config.account_id.clone();
    let stop = cancel.clone();
    let watcher = tokio::spawn(async move {
        loop {
            if !authorized(&watched, &owner) {
                stop.cancel();
                return;
            }
            if changes.changed().await.is_err() {
                stop.cancel();
                return;
            }
        }
    });
    let mut retained = None;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let result = workspace(&api, &auth, &config.account_id, &config.workspace_id).await;
        let mut interval = Duration::from_secs(5);
        match result {
            Ok(value) => {
                let row = serde_json::from_value::<Workspace>(value.clone());
                match row {
                    Ok(mut row)
                        if row.archived()
                            || row.deleting()
                            || matches!(row.state.as_str(), "deleted" | "deleting") =>
                    {
                        api.local_cleanup(&config.account_id, &mut row, false);
                        break;
                    }
                    Ok(row) => {
                        if row.rpc_protocol.as_deref() != Some("zuse-rpc-v1") {
                            api.syncs.update(
                                &config.account_id,
                                &config.workspace_id,
                                &cancel,
                                |r| {
                                    r.status.state = "error".into();
                                    r.status.error = Some(
                                        "Update this cloud runtime before syncing files.".into(),
                                    );
                                },
                            );
                        } else if row.state != "ready" || row.desired_state != "ready" {
                            retained = None;
                            api.syncs.update(
                                &config.account_id,
                                &config.workspace_id,
                                &cancel,
                                |r| {
                                    r.status.state = "waiting".into();
                                    r.status.error = None;
                                },
                            );
                        } else {
                            if retained.as_ref().is_none_or(
                                |environment: &Arc<environment::Environment>| {
                                    environment.ready_client().is_none()
                                },
                            ) {
                                retained = api
                                    .environments
                                    .get(&config.account_id, &config.workspace_id);
                            }
                            if let Some(client) = retained
                                .as_ref()
                                .and_then(|environment| environment.ready_client())
                            {
                                let result = async {
                                    let _gate = api
                                        .mirrors
                                        .sync_guard(&config.account_id, &config.workspace_id)
                                        .await?;
                                    ensure!(!cancel.is_cancelled(), "Sync stopped.");
                                    api.syncs.update(
                                        &config.account_id,
                                        &config.workspace_id,
                                        &cancel,
                                        |r| {
                                            r.status.state = "syncing".into();
                                            r.status.error = None;
                                        },
                                    );
                                    mirror::sync_once_cancellable(
                                        &client,
                                        WorkspaceTarget {
                                            chat_id: Some(config.session_id.clone()),
                                            space_id: None,
                                            checkout_path: None,
                                        },
                                        config.local_path.clone(),
                                        config.workspace_id.clone(),
                                        cancel.clone(),
                                    )
                                    .await
                                }
                                .await;
                                match result {
                                    Ok(result) => api.syncs.update(
                                        &config.account_id,
                                        &config.workspace_id,
                                        &cancel,
                                        |r| {
                                            r.status.state = "synced".into();
                                            r.status.last_synced_at =
                                                Some(chrono::Utc::now().timestamp_millis());
                                            r.status.files = result.files;
                                            r.status.transferred_bytes = result.transferred_bytes;
                                            r.status.error = None;
                                        },
                                    ),
                                    Err(error) => {
                                        interval = Duration::from_secs(15);
                                        api.syncs.update(
                                            &config.account_id,
                                            &config.workspace_id,
                                            &cancel,
                                            |r| {
                                                r.status.state = "error".into();
                                                r.status.error = Some(error.to_string());
                                            },
                                        );
                                    }
                                }
                            } else {
                                api.syncs.update(&config.account_id,&config.workspace_id,&cancel,|r|{r.status.state="waiting".into();r.status.error=Some("Connect to the cloud workspace to sync files. Paused compute stays asleep.".into());});
                            }
                        }
                    }
                    Err(error) => {
                        api.syncs
                            .update(&config.account_id, &config.workspace_id, &cancel, |r| {
                                r.status.state = "error".into();
                                r.status.error =
                                    Some(format!("Invalid cloud sync workspace: {error}"));
                            })
                    }
                }
            }
            Err(error) => {
                interval = Duration::from_secs(15);
                api.syncs
                    .update(&config.account_id, &config.workspace_id, &cancel, |r| {
                        r.status.state = "error".into();
                        r.status.error = Some(error.to_string());
                    });
            }
        }
        tokio::select! {biased;_=cancel.cancelled()=>break,_=wake.notified()=>{},_=tokio::time::sleep(interval)=>{}}
    }
    watcher.abort();
    api.syncs
        .update(&config.account_id, &config.workspace_id, &cancel, |r| {
            r.status.state = if r.config.enabled { "waiting" } else { "idle" }.into();
            r.status.enabled = r.config.enabled;
            r.task = None;
        });
}
fn default_path(base: &Path, repository: &str, branch: &str) -> Result<PathBuf> {
    let name = repository
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim_end_matches(".git");
    let parts = std::iter::once(name)
        .chain(branch.split('/'))
        .collect::<Vec<_>>();
    ensure!(
        parts.iter().all(|s| !s.is_empty()
            && !matches!(*s, "." | "..")
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))),
        "Choose a local folder for this repository and branch."
    );
    Ok(parts
        .iter()
        .fold(base.to_owned(), |path, part| path.join(part)))
}
pub async fn handle(auth: Auth, method: &str, params: Value) -> Result<Value> {
    let api = CloudApi::shared()?;
    ensure!(
        auth.workos_enabled() || api.loopback,
        "Sign in to access cloud sync."
    );
    let account = auth.user_id().context("Sign in to access cloud sync.")?;
    ensure!(
        params["expectedAccountId"].as_str() == Some(&account) && authorized(&auth, &account),
        "The account changed. Select the workspace again."
    );
    let id = params["workspaceId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512)
        .context("Select a cloud workspace.")?
        .to_owned();
    api.syncs.hydrate(&api, &auth, &account)?;
    api.syncs.ensure_reconciliation(auth.clone());
    if method == methods::CLOUD_SYNC_STATUS {
        reconcile(api.clone(), auth.clone()).await?;
        return Ok(json!({"accountId":account,"status":api.syncs.status(&account,&id)}));
    }
    let _change = api.syncs.changes.lock().await;
    if method == methods::CLOUD_SYNC_RETRY {
        if let Some(record) = lock(&api.syncs.records).get(&key(&account, &id)) {
            record.wake.notify_one();
        }
        Manager::start(api.clone(), auth, account.clone(), id.clone());
        return Ok(json!({"accountId":account,"status":api.syncs.status(&account,&id)}));
    }
    let enabled = params["enabled"]
        .as_bool()
        .context("Choose whether to sync files.")?;
    let previous = lock(&api.syncs.records)
        .get(&key(&account, &id))
        .map(|r| r.config.clone());
    let value = if enabled {
        Some(workspace(&api, &auth, &account, &id).await?)
    } else {
        None
    };
    if let Some(value) = &value {
        let row: Workspace = serde_json::from_value(value.clone())?;
        ensure!(
            !row.archived()
                && !row.deleting()
                && !matches!(row.state.as_str(), "deleted" | "deleting"),
            "Unarchive this workspace before enabling local sync."
        );
        ensure!(
            row.rpc_protocol.as_deref() == Some("zuse-rpc-v1"),
            "This runtime needs an update before local sync."
        );
    }
    let requested = params["localPath"]
        .as_str()
        .filter(|s| !s.trim().is_empty());
    let path = if !enabled {
        previous
            .as_ref()
            .map(|c| c.local_path.clone())
            .or_else(|| requested.map(PathBuf::from))
    } else if let Some(path) = requested {
        Some(PathBuf::from(path))
    } else if let Some(previous) = &previous {
        Some(previous.local_path.clone())
    } else {
        let value = value.as_ref().unwrap();
        let token = auth
            .access_token()
            .await
            .map_err(|_| anyhow::anyhow!("Sign in to access cloud sync."))?;
        ensure!(
            authorized(&auth, &account),
            "The account changed. Select the workspace again."
        );
        let catalog = api
            .request(&token, &["v1", "cloud", "context"], None)
            .await?;
        ensure!(
            authorized(&auth, &account),
            "The account changed. Select the workspace again."
        );
        let project = catalog["projects"]
            .as_array()
            .and_then(|rows| rows.iter().find(|p| p["projectId"] == value["projectId"]))
            .context("Choose a local folder for this workspace.")?;
        Some(default_path(
            api.mirrors.base(),
            project["repositoryUrl"].as_str().unwrap_or_default(),
            value["branch"].as_str().unwrap_or_default(),
        )?)
    };
    let Some(path) = path else {
        return Ok(json!({"accountId":account,"status":Status::empty(&id)}));
    };
    let path = path.components().collect::<PathBuf>();
    ensure!(
        path.is_absolute()
            && path.to_string_lossy().len() <= 16 * 1024
            && path.components().all(|p| matches!(
                p,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )),
        "Choose an absolute local sync folder without parent traversal."
    );
    let session = value
        .as_ref()
        .and_then(|v| v["initialSessionId"].as_str())
        .map(str::to_owned)
        .or_else(|| previous.as_ref().map(|c| c.session_id.clone()))
        .context("Cloud conversation identity is missing.")?;
    ensure!(
        !session.is_empty(),
        "Cloud conversation identity is missing."
    );
    let mut roots = previous
        .as_ref()
        .map(|c| c.roots.clone())
        .unwrap_or_default();
    if !roots.contains(&path) {
        roots.push(path.clone());
    }
    if let Some(previous) = &previous {
        if !roots.contains(&previous.local_path) {
            roots.push(previous.local_path.clone());
        }
    }
    let config = Config {
        version: 1,
        account_id: account.clone(),
        workspace_id: id.clone(),
        enabled,
        local_path: path,
        session_id: session,
        roots,
    };
    if previous.as_ref() == Some(&config) {
        if enabled {
            Manager::start(api.clone(), auth, account.clone(), id.clone());
        }
        return Ok(json!({"accountId":account,"status":api.syncs.status(&account,&id)}));
    }
    let old = {
        let mut records = lock(&api.syncs.records);
        records.get_mut(&key(&account, &id)).and_then(|r| {
            r.cancel.cancel();
            r.status.state = "stopping".into();
            r.task.take()
        })
    };
    if let Some(task) = old {
        task.await
            .context("Previous file sync did not stop cleanly.")?;
    }
    let _registration = if enabled {
        let admission = async {
            let gate = api.mirrors.sync_guard(&account, &id).await?;
            mirror::prepare_target(&config.local_path, &id)?;
            Ok::<_, anyhow::Error>(gate)
        }
        .await;
        match admission {
            Ok(gate) => Some(gate),
            Err(error) => {
                // A rejected new destination cannot disable the previous valid mirror.
                if previous.as_ref().is_some_and(|c| c.enabled) {
                    Manager::start(api.clone(), auth, account.clone(), id.clone());
                }
                return Err(error);
            }
        }
    } else {
        None
    };
    if previous
        .as_ref()
        .is_some_and(|old| old.local_path != config.local_path)
    {
        api.local_terminals.stop_workspace(&account, &id);
    }
    let file = directory(&auth, &account).join(format!("{}.json", hashed(&id)));
    for root in &config.roots {
        api.mirrors.register_root(&account, &id, root.clone());
    }
    let status = Status::configured(&config);
    {
        let mut records = lock(&api.syncs.records);
        save(&file, &config)?;
        records.insert(
            key(&account, &id),
            Record {
                config,
                file,
                status,
                cancel: CancellationToken::new(),
                wake: Arc::new(Notify::new()),
                task: None,
            },
        );
    }
    if enabled {
        Manager::start(api.clone(), auth, account.clone(), id.clone());
    }
    Ok(json!({"accountId":account,"status":api.syncs.status(&account,&id)}))
}
pub(super) async fn reconcile(api: Arc<CloudApi>, auth: Auth) -> Result<()> {
    if !auth.workos_enabled() && !api.loopback {
        return Ok(());
    }

    let Some(account) = auth.user_id().filter(|a| authorized(&auth, a)) else {
        return Ok(());
    };
    api.syncs.hydrate(&api, &auth, &account)?;
    let Ok(_change) = api.syncs.changes.try_lock() else {
        return Ok(());
    };
    let ids = lock(&api.syncs.records)
        .values()
        .filter(|r| r.config.account_id == account && r.config.enabled)
        .map(|r| r.config.workspace_id.clone())
        .collect::<Vec<_>>();
    for id in ids {
        Manager::start(api.clone(), auth.clone(), account.clone(), id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn auth(root: &Path, origin: &str) -> Auth {
        let mut config = crate::auth::AuthConfig::new(origin, root);
        config.workos_client_id = None;
        Auth::new(config)
    }
    fn config(root: &Path, account: &str) -> Config {
        Config {
            version: 1,
            account_id: account.into(),
            workspace_id: "controller-workspace".into(),
            enabled: true,
            local_path: root.join("custom-mirror"),
            session_id: "original-session".into(),
            roots: vec![root.join("custom-mirror")],
        }
    }
    fn api(root: &Path, origin: &str) -> Arc<CloudApi> {
        let mut api = CloudApi::new(origin).unwrap();
        api.mirrors = mirror::Manager::new(root.join("default-mirrors"));
        Arc::new(api)
    }
    fn store(auth: &Auth, config: &Config) -> PathBuf {
        let file = directory(auth, &config.account_id)
            .join(format!("{}.json", hashed(&config.workspace_id)));
        save(&file, config).unwrap();
        file
    }
    #[tokio::test]
    async fn persisted_configuration_is_private_scoped_and_loaded_without_compute() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let auth = auth(&root.path().join("profile"), "http://127.0.0.1:1");
        let account = auth.user_id().unwrap();
        let config = config(root.path(), &account);
        let file = store(&auth, &config);
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let api = api(root.path(), "http://127.0.0.1:1");
        api.syncs.hydrate(&api, &auth, &account).unwrap();
        let status = api.syncs.status(&account, &config.workspace_id);
        assert!(status.enabled);
        assert_eq!(status.state, "waiting");
        assert_eq!(status.local_path, Some(config.local_path.clone()));
        assert!(
            !api.syncs
                .status("other-account", &config.workspace_id)
                .enabled
        );
        assert!(
            api.environments
                .get(&account, &config.workspace_id)
                .is_none()
        );
        assert!(
            !config.local_path.exists(),
            "Loading preferences cannot create or modify the mirror"
        );
        assert_eq!(
            default_path(
                Path::new("/home/user/.zuse/cloud"),
                "https://github.com/owner/repo.git",
                "feature/work"
            )
            .unwrap(),
            Path::new("/home/user/.zuse/cloud/repo/feature/work")
        );
        assert!(default_path(Path::new("/tmp"), "repo", "../../outside").is_err());
    }
    #[tokio::test]
    async fn enabled_sync_of_paused_compute_only_reads_metadata_and_sign_out_stops_it() {
        use axum::{Json, Router, routing::get};
        let reads = Arc::new(AtomicUsize::new(0));
        let writes = Arc::new(AtomicUsize::new(0));
        let observed = reads.clone();
        let mutations = writes.clone();
        let app=Router::new().route("/v1/cloud/workspaces/controller-workspace",get(move||{let reads=observed.clone();async move{reads.fetch_add(1,Ordering::SeqCst);Json(json!({"workspaceId":"controller-workspace","chatId":"thread","initialSessionId":"original-session","branch":"feature/work","state":"paused","desiredState":"paused","statusCode":"paused","revision":1,"rpcProtocol":"zuse-rpc-v1"}))}})).fallback(move||{let writes=mutations.clone();async move{writes.fetch_add(1,Ordering::SeqCst);axum::http::StatusCode::METHOD_NOT_ALLOWED}});
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let auth = auth(&root.path().join("profile"), &origin);
        let account = auth.user_id().unwrap();
        let config = config(root.path(), &account);
        store(&auth, &config);
        let api = api(root.path(), &origin);
        api.syncs.hydrate(&api, &auth, &account).unwrap();
        Manager::start(
            api.clone(),
            auth.clone(),
            account.clone(),
            config.workspace_id.clone(),
        );
        tokio::time::timeout(Duration::from_secs(3), async {
            while reads.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        assert!(
            api.environments
                .get(&account, &config.workspace_id)
                .is_none()
        );
        assert!(!config.local_path.exists());
        auth.sign_out();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if lock(&api.syncs.records)
                    .get(&key(&account, &config.workspace_id))
                    .unwrap()
                    .task
                    .is_none()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let before = reads.load(Ordering::SeqCst);
        reconcile(api.clone(), auth).await.unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert_eq!(before, reads.load(Ordering::SeqCst));
        server.abort();
    }
    #[tokio::test]
    async fn archive_fences_registered_custom_mirror_cleanup_and_delete_forgets_settings() {
        let root = tempfile::tempdir().unwrap();
        let auth = auth(&root.path().join("profile"), "http://127.0.0.1:1");
        let account = auth.user_id().unwrap();
        let config = config(root.path(), &account);
        let file = store(&auth, &config);
        fs::create_dir(&config.local_path).unwrap();
        fs::write(config.local_path.join("owned"), b"cloud bytes").unwrap();
        fs::write(config.local_path.join("local-only"), b"unrelated").unwrap();
        fs::write(config.local_path.join(".zuse-sync.json"),serde_json::to_vec(&json!({"version":1,"workspaceId":config.workspace_id,"files":[{"path":"owned","hash":hashed("cloud bytes"),"mode":0o644,"size":11}]})).unwrap()).unwrap();
        // File mode is part of ownership verification.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            config.local_path.join("owned"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let api = api(root.path(), "http://127.0.0.1:1");
        api.syncs.hydrate(&api, &auth, &account).unwrap();
        let gate = api
            .mirrors
            .sync_guard(&account, &config.workspace_id)
            .await
            .unwrap();
        let mut row:Workspace=serde_json::from_value(json!({"workspaceId":config.workspace_id,"chatId":"thread","initialSessionId":"original-session","branch":"feature/work","state":"archived","desiredState":"archived","statusCode":"archived","revision":2,"rpcProtocol":"zuse-rpc-v1"})).unwrap();
        api.local_cleanup(&account, &mut row, false);
        assert!(!api.syncs.status(&account, &config.workspace_id).enabled);
        assert!(config.local_path.join("owned").exists());
        assert_eq!(row.local_mirror_cleanup.as_ref().unwrap().state, "pending");
        drop(gate);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                api.local_cleanup(&account, &mut row, false);
                if row.local_mirror_cleanup.as_ref().unwrap().state == "complete" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!config.local_path.join("owned").exists());
        assert_eq!(
            fs::read(config.local_path.join("local-only")).unwrap(),
            b"unrelated"
        );
        let saved: Config = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert!(!saved.enabled);
        row.state = "deleted".into();
        row.desired_state = "deleted".into();
        api.local_cleanup(&account, &mut row, false);
        assert!(!file.exists());
        assert!(!api.syncs.status(&account, &config.workspace_id).enabled);
    }
    #[tokio::test]
    async fn reconciliation_cannot_replace_a_worker_while_configuration_is_draining() {
        let root = tempfile::tempdir().unwrap();
        let auth = auth(&root.path().join("profile"), "http://127.0.0.1:1");
        let account = auth.user_id().unwrap();
        let config = config(root.path(), &account);
        store(&auth, &config);
        let api = api(root.path(), "http://127.0.0.1:1");
        api.syncs.hydrate(&api, &auth, &account).unwrap();
        let gate = api.syncs.changes.lock().await;
        reconcile(api.clone(), auth.clone()).await.unwrap();
        assert!(
            lock(&api.syncs.records)
                .get(&key(&account, &config.workspace_id))
                .unwrap()
                .task
                .is_none()
        );
        drop(gate);
        let old = CancellationToken::new();
        let current = CancellationToken::new();
        {
            let mut records = lock(&api.syncs.records);
            records
                .get_mut(&key(&account, &config.workspace_id))
                .unwrap()
                .cancel = current;
        }
        old.cancel();
        api.syncs.update(&account, &config.workspace_id, &old, |r| {
            r.status.state = "old-worker".into()
        });
        assert_ne!(
            api.syncs.status(&account, &config.workspace_id).state,
            "old-worker"
        );
    }
    #[tokio::test]
    async fn development_preferences_cannot_start_production_sync_workers() {
        let root = tempfile::tempdir().unwrap();
        let auth = auth(&root.path().join("profile"), "http://127.0.0.1:1");
        let account = auth.user_id().unwrap();
        let config = config(root.path(), &account);
        store(&auth, &config);
        let api = api(root.path(), "https://api-rust.zuse.sh");
        reconcile(api.clone(), auth.clone()).await.unwrap();
        assert!(
            lock(&api.syncs.records).is_empty(),
            "Production background work must not load development preferences or send a synthetic bearer"
        );
        api.syncs.hydrate(&api, &auth, &account).unwrap();
        Manager::start(
            api.clone(),
            auth,
            account.clone(),
            config.workspace_id.clone(),
        );
        assert!(
            lock(&api.syncs.records)
                .get(&key(&account, &config.workspace_id))
                .unwrap()
                .task
                .is_none()
        );
    }
}
