//! A cloud chat's files in a folder of its own on this computer, one way, so
//! a dev server, containers or builds can run against what the cloud agent
//! changes without ever touching the user's own checkout.
//!
//! Turning it on makes `~/zuse/cloud/<owner>-<repo>/<chat>`: a git worktree
//! of the user's clone of the repository when Zuse knows one (cheap: it
//! shares that clone's objects), otherwise a fresh shallow clone. Each new
//! snapshot of the chat's checkout becomes a checkpoint commit the folder
//! moves to, detached. Edits made in the folder pause sync instead of being
//! overwritten; the user discards them to resume or stops syncing to keep
//! editing. Turning it off keeps the folder; deleting it, or archiving the
//! chat while the folder holds nothing but the chat's files, removes it.
//! Untracked and ignored files are never touched.
//!
//! Earlier versions synced into the user's own checkout. Such a sync is put
//! back exactly as saved (branch, HEAD and tracked changes; crash-safe), then
//! the chat moves on to a folder of its own.
//!
//! Snapshots come over the chat's existing computer connection (the same
//! transfer as "Sync to a local directory") and never wake compute. A
//! snapshot is verified completely before anything in the folder changes.
use super::*;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use zuse_protocol::{WorkspaceTarget, methods};

pub(crate) mod repo;
#[cfg(test)]
mod tests;

/// The fastest and slowest re-check of an unchanged chat.
const MIN_DELAY: Duration = Duration::from_secs(3);
const MAX_DELAY: Duration = Duration::from_secs(30);
/// How often the chat itself is checked for being archived.
const CHAT_CHECK: Duration = Duration::from_secs(30);
/// Snapshot attempts in one step when the connection drops part way; each
/// one keeps what arrived.
const SNAPSHOT_ATTEMPTS: usize = 3;
/// State file versions: 1 synced the user's own checkout, 2 a folder of the
/// chat's own (older versions of the app skip those).
const VERSION_CHECKOUT: u32 = 1;
const VERSION_FOLDER: u32 = 2;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Mode {
    /// The user's own checkout, saved and put back (earlier versions).
    #[default]
    Checkout,
    /// A folder made for this chat.
    Folder,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Phase {
    /// Synced, or waiting for its first checkpoint.
    On,
    /// Putting the checkout back began; it finishes even after a restart.
    Restoring,
    /// Not syncing; the chat's folder stays.
    Stopped,
}

/// What is persisted for one synced folder, written before any file in it
/// changes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct State {
    version: u32,
    #[serde(default)]
    mode: Mode,
    account_id: String,
    workspace_id: String,
    chat_id: String,
    path: PathBuf,
    phase: Phase,
    /// How the folder was before its first checkpoint; absent until then.
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    head: Option<String>,
    #[serde(default)]
    changes: Option<String>,
    /// The checkpoint the folder is on.
    #[serde(default)]
    checkpoint: Option<String>,
    /// The checkpoint a move began toward, recorded before any file moves:
    /// if HEAD is there, the move finished.
    #[serde(default)]
    pending: Option<String>,
    /// A chat's folder: the user's clone it is a worktree of, when it is one.
    #[serde(default)]
    source: Option<PathBuf>,
    /// A chat's folder: the repository (`host/owner/repo`) and where to
    /// clone it from when there is no clone to make a worktree of.
    #[serde(default)]
    repository: Option<String>,
    #[serde(default)]
    url: Option<String>,
    /// An own-checkout sync being put back so the chat can move to a folder
    /// of its own; remembered across a restart.
    #[serde(default)]
    migrating: bool,
}
impl State {
    fn saved(&self) -> Option<repo::Saved> {
        Some(repo::Saved {
            branch: self.branch.clone(),
            head: self.head.clone()?,
            changes: self.changes.clone(),
        })
    }
    fn chat(&self) -> (String, String, String) {
        (
            self.account_id.clone(),
            self.workspace_id.clone(),
            self.chat_id.clone(),
        )
    }
    fn folder(
        account: &str,
        workspace: &str,
        chat: &str,
        path: PathBuf,
        repository: &str,
        source: Option<PathBuf>,
        url: Option<String>,
    ) -> Self {
        Self {
            version: VERSION_FOLDER,
            mode: Mode::Folder,
            account_id: account.into(),
            workspace_id: workspace.into(),
            chat_id: chat.into(),
            path,
            phase: Phase::On,
            branch: None,
            head: None,
            changes: None,
            checkpoint: None,
            pending: None,
            source,
            repository: Some(repository.into()),
            url,
            migrating: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    workspace_id: String,
    chat_id: String,
    enabled: bool,
    /// `off`, `preparing`, `syncing`, `synced`, `behind` (the cloud computer
    /// isn't reachable right now), `paused` (edited here), `restoring` (an
    /// earlier version's sync into the user's checkout going back) or
    /// `error`.
    state: &'static str,
    /// The chat's folder, also while off when it was kept.
    local_path: Option<PathBuf>,
    /// The user's clone the folder is a worktree of.
    source_path: Option<PathBuf>,
    last_synced_at: Option<i64>,
    files: usize,
    /// Why it is paused, behind or failed.
    error: Option<String>,
    /// Something to tell the user once, such as where kept edits went.
    notice: Option<String>,
    /// The cloud computer must be updated before it can sync.
    update_required: bool,
}
impl Status {
    fn off(workspace: &str, chat: &str) -> Self {
        Self {
            workspace_id: workspace.into(),
            chat_id: chat.into(),
            enabled: false,
            state: "off",
            local_path: None,
            source_path: None,
            last_synced_at: None,
            files: 0,
            error: None,
            notice: None,
            update_required: false,
        }
    }
    fn on(state: &State) -> Self {
        Self {
            enabled: state.phase == Phase::On,
            state: match state.phase {
                Phase::On => "preparing",
                Phase::Restoring => "restoring",
                Phase::Stopped => "off",
            },
            local_path: Some(state.path.clone()),
            source_path: state.source.clone(),
            ..Self::off(&state.workspace_id, &state.chat_id)
        }
    }
}

struct Record {
    state: State,
    file: PathBuf,
    status: Status,
    cancel: CancellationToken,
    wake: Arc<Notify>,
    task: Option<tokio::task::JoinHandle<()>>,
}

type Reporter = Arc<dyn Fn(String, String, String) + Send + Sync>;

#[derive(Default)]
pub(super) struct Manager {
    /// By folder: one chat per folder.
    records: Mutex<HashMap<PathBuf, Record>>,
    /// What the user should hear about a chat whose sync ended on its own.
    notices: Mutex<HashMap<(String, String, String), String>>,
    /// Serializes turning sync on and off.
    changes: tokio::sync::Mutex<()>,
    loaded: Mutex<Option<PathBuf>>,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Tells the cloud the chat's synced folder changed on this computer.
    reporter: Mutex<Option<Reporter>>,
    /// Checkouts being put back right now: one restore per checkout.
    restoring: Mutex<std::collections::HashSet<PathBuf>>,
    /// Kept folders of stopped syncs by how many checks in a row found their
    /// chat archived.
    archived: Mutex<HashMap<PathBuf, u8>>,
}

/// Holds a checkout's restore slot until dropped.
struct RestoreSlot<'a>(&'a Manager, PathBuf);
impl Drop for RestoreSlot<'_> {
    fn drop(&mut self) {
        lock(&self.0.restoring).remove(&self.1);
    }
}

fn hashed(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn directory(profile: &Path) -> PathBuf {
    profile.join("checkout-sync")
}
fn state_file(profile: &Path, checkout: &Path) -> PathBuf {
    directory(profile).join(format!("{}.json", hashed(&checkout.to_string_lossy())))
}
fn objects_dir(profile: &Path, checkout: &Path) -> PathBuf {
    directory(profile).join(format!("{}.objects", hashed(&checkout.to_string_lossy())))
}
fn write_state(file: &Path, state: &State) -> Result<()> {
    let parent = file.parent().context("Invalid sync settings path.")?;
    fs::create_dir_all(parent)?;
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec(state)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(file).map_err(|error| error.error)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
fn read_states(profile: &Path) -> Vec<(PathBuf, State)> {
    let Ok(entries) = fs::read_dir(directory(profile)) else {
        return vec![];
    };
    entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "json" {
                return None;
            }
            let meta = fs::symlink_metadata(&path).ok()?;
            if !meta.is_file() || meta.len() > 64 * 1024 {
                return None;
            }
            let state: State = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
            let known = match state.mode {
                Mode::Checkout => state.version == VERSION_CHECKOUT,
                Mode::Folder => state.version == VERSION_FOLDER,
            };
            (known && state.path.is_absolute() && path == state_file(profile, &state.path))
                .then_some((path, state))
        })
        .collect()
}

/// Where chats' folders go: `ZUSE_CLOUD_SYNC_ROOT` when set, otherwise
/// `~/zuse/cloud`.
fn folders_root() -> Result<PathBuf> {
    if let Some(root) = std::env::var_os("ZUSE_CLOUD_SYNC_ROOT").map(PathBuf::from)
        && root.is_absolute()
    {
        return Ok(root);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .context("Couldn't find your home folder.")?;
    Ok(home.join("zuse").join("cloud"))
}

/// One path segment from free text: lowercase letters, digits, `.`, `_`
/// and `-`.
fn segment_name(value: &str) -> String {
    let name = value
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let name = name.trim_matches(|c| c == '-' || c == '.').to_owned();
    if name.is_empty() { "chat".into() } else { name }
}

/// The folder for a chat of `repository` (`host/owner/repo`) under `root`:
/// `<owner>-<repo>/<the chat id's last 8 letters and digits>`, with a number
/// added when something else is already there.
fn folder_for(root: &Path, repository: &str, chat: &str, taken: &dyn Fn(&Path) -> bool) -> PathBuf {
    let mut parts = repository.rsplit('/');
    let repo = parts.next().unwrap_or("repository");
    let owner = parts.next();
    let group = segment_name(&match owner {
        Some(owner) => format!("{owner}-{repo}"),
        None => repo.to_owned(),
    });
    let letters = chat
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<Vec<_>>();
    let short = segment_name(
        &letters[letters.len().saturating_sub(8)..]
            .iter()
            .collect::<String>(),
    );
    let base = root.join(group);
    let mut path = base.join(&short);
    let mut n = 2;
    while taken(&path) {
        path = base.join(format!("{short}-{n}"));
        n += 1;
    }
    path
}

/// The folder a chat is synced into on this computer, once it holds the
/// chat's files. The connector reports it so the chat's commands here start
/// in that folder.
pub(crate) fn synced_path(
    profile: &Path,
    account: &str,
    workspace: &str,
    chat: &str,
) -> Option<PathBuf> {
    read_states(profile).into_iter().find_map(|(_, state)| {
        (state.account_id == account
            && state.workspace_id == workspace
            && state.chat_id == chat
            && state.phase == Phase::On
            && state
                .checkpoint
                .as_deref()
                .is_some_and(|checkpoint| repo::holds(&state.path, checkpoint)))
        .then_some(state.path)
    })
}

/// Make `path` a folder synced to a chat (tests of the connector's report).
/// The returned closure moves it onto a branch, as a user would.
#[cfg(test)]
pub(crate) fn record_synced_for_test(
    profile: &Path,
    account: &str,
    workspace: &str,
    chat: &str,
    path: &Path,
) -> impl Fn() {
    let run = |args: &[&str]| {
        let mut command = std::process::Command::new("git");
        for name in repo::REPOSITORY_VARIABLES {
            command.env_remove(name);
        }
        assert!(
            command
                .arg("-C")
                .arg(path)
                .args(args)
                .env("GIT_AUTHOR_NAME", "Test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "Test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .status()
                .unwrap()
                .success()
        );
    };
    run(&["init", "-q"]);
    run(&["commit", "-q", "--allow-empty", "-m", "base"]);
    run(&["checkout", "-q", "--detach"]);
    let checkpoint = repo::head(path).unwrap();
    let mut state = State::folder(
        account,
        workspace,
        chat,
        path.to_owned(),
        "github.com/example/repo",
        None,
        None,
    );
    state.head = Some(checkpoint.clone());
    state.checkpoint = Some(checkpoint);
    write_state(&state_file(profile, path), &state).unwrap();
    let path = path.to_owned();
    move || {
        let mut command = std::process::Command::new("git");
        for name in repo::REPOSITORY_VARIABLES {
            command.env_remove(name);
        }
        assert!(
            command
                .arg("-C")
                .arg(&path)
                .args(["switch", "-q", "-c", "elsewhere"])
                .status()
                .unwrap()
                .success()
        );
    }
}

/// Who reports a chat's synced folder to the cloud (the computer connector).
pub fn set_reporter(report: impl Fn(String, String, String) + Send + Sync + 'static) {
    if let Ok(api) = CloudApi::shared() {
        *lock(&api.checkouts.reporter) = Some(Arc::new(report));
    }
}

/// Re-check every synced folder now (for example after this computer's
/// connector stopped).
pub fn nudge() {
    if let Ok(api) = CloudApi::shared() {
        for record in lock(&api.checkouts.records).values() {
            record.wake.notify_one();
        }
    }
}

fn authorized(auth: &Auth, account: &str) -> bool {
    super::sync::authorized(auth, account)
}

/// Why a sync must end on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    Account,
    Connector,
    Archived,
}
impl Ending {
    /// What the user hears, for a sync of `state`'s kind.
    fn notice(self, state: &State) -> String {
        let folder = state.path.display();
        match (state.mode, self) {
            (Mode::Checkout, Ending::Account) => {
                format!("You switched accounts, so {folder} went back to how you left it.")
            }
            (Mode::Checkout, Ending::Connector) => format!(
                "This computer stopped running cloud chat commands, so {folder} went back to how you left it."
            ),
            (Mode::Checkout, Ending::Archived) => {
                format!("This chat was archived, so {folder} went back to how you left it.")
            }
            (Mode::Folder, Ending::Account) => {
                format!("You switched accounts, so sync stopped. The folder stays at {folder}.")
            }
            (Mode::Folder, Ending::Connector) => format!(
                "This computer stopped running cloud chat commands, so sync stopped. The folder stays at {folder}."
            ),
            (Mode::Folder, Ending::Archived) => format!(
                "This chat was archived. Its folder has changes made here, so it stays at {folder}."
            ),
        }
    }
}

impl Manager {
    fn report(&self, chat: (String, String, String)) {
        let reporter = lock(&self.reporter).clone();
        if let Some(report) = reporter {
            report(chat.0, chat.1, chat.2);
        }
    }

    /// Load persisted syncs once per profile.
    fn hydrate(&self, profile: &Path) {
        let mut loaded = lock(&self.loaded);
        if loaded.as_deref() == Some(profile) {
            return;
        }
        let mut records = lock(&self.records);
        for (file, state) in read_states(profile) {
            records.entry(state.path.clone()).or_insert_with(|| Record {
                status: Status::on(&state),
                state,
                file,
                cancel: CancellationToken::new(),
                wake: Arc::new(Notify::new()),
                task: None,
            });
        }
        *loaded = Some(profile.to_owned());
    }

    fn by_chat(&self, account: &str, workspace: &str, chat: &str) -> Option<PathBuf> {
        let records = lock(&self.records);
        let matching = |r: &&Record| {
            r.state.account_id == account
                && r.state.workspace_id == workspace
                && r.state.chat_id == chat
        };
        // An earlier version's sync of the user's checkout comes first: it
        // must go back before anything else happens for this chat.
        records
            .iter()
            .filter(|(_, r)| matching(r))
            .min_by_key(|(_, r)| r.state.mode == Mode::Folder)
            .map(|(path, _)| path.clone())
    }

    fn status(&self, account: &str, workspace: &str, chat: &str) -> Status {
        let key = self.by_chat(account, workspace, chat);
        let records = lock(&self.records);
        match key.and_then(|key| records.get(&key)) {
            Some(record) => record.status.clone(),
            None => {
                let mut status = Status::off(workspace, chat);
                status.notice = lock(&self.notices)
                    .get(&(account.into(), workspace.into(), chat.into()))
                    .cloned();
                status
            }
        }
    }

    fn state(&self, key: &Path) -> Option<State> {
        lock(&self.records).get(key).map(|r| r.state.clone())
    }

    /// Change a record's status, when its sync is still the one `cancel`
    /// belongs to.
    fn set_status(&self, key: &Path, cancel: &CancellationToken, update: impl FnOnce(&mut Status)) {
        if let Some(record) = lock(&self.records).get_mut(key)
            && &record.cancel == cancel
        {
            update(&mut record.status);
        }
    }

    /// Persist a record's new state before acting on it.
    fn save_state(&self, key: &Path, state: &State) -> Result<()> {
        let file = {
            let records = lock(&self.records);
            records.get(key).map(|r| r.file.clone())
        };
        let file = file.context("Sync stopped.")?;
        write_state(&file, state)?;
        if let Some(record) = lock(&self.records).get_mut(key) {
            record.state = state.clone();
        }
        Ok(())
    }

    /// Record a new chat folder (not made yet: its worker makes it).
    fn insert(&self, profile: &Path, state: State) -> Result<PathBuf> {
        let key = state.path.clone();
        let file = state_file(profile, &key);
        write_state(&file, &state)?;
        lock(&self.records).insert(
            key.clone(),
            Record {
                status: Status::on(&state),
                state,
                file,
                cancel: CancellationToken::new(),
                wake: Arc::new(Notify::new()),
                task: None,
            },
        );
        Ok(key)
    }

    /// A free folder for a new chat folder under `root`.
    fn new_folder(&self, root: &Path, repository: &str, chat: &str) -> Result<PathBuf> {
        fs::create_dir_all(root).with_context(|| format!("Couldn't create {}.", root.display()))?;
        let root = root.canonicalize()?;
        let records = lock(&self.records);
        Ok(folder_for(&root, repository, chat, &|path| {
            records.contains_key(path) || path.exists()
        }))
    }

    fn start(api: &Arc<CloudApi>, auth: &Auth, key: &Path) {
        let mut records = lock(&api.checkouts.records);
        let Some(record) = records.get_mut(key) else {
            return;
        };
        if record.state.mode != Mode::Folder
            || record.state.phase != Phase::On
            || record.task.as_ref().is_some_and(|task| !task.is_finished())
            || !authorized(auth, &record.state.account_id)
        {
            return;
        }
        record.cancel = CancellationToken::new();
        let cancel = record.cancel.clone();
        let wake = record.wake.clone();
        let (api, auth, key) = (api.clone(), auth.clone(), key.to_owned());
        record.task = Some(tokio::spawn(async move {
            run(api, auth, key, cancel, wake).await;
        }));
    }

    /// Stop `key`'s worker and wait for it.
    async fn halt_worker(&self, key: &Path) {
        let task = {
            let mut records = lock(&self.records);
            records.get_mut(key).and_then(|record| {
                record.cancel.cancel();
                record.task.take()
            })
        };
        if let Some(task) = task
            && task.await.is_err()
        {
            tracing::warn!("checkout sync: worker ended abnormally");
        }
    }

    /// Turn sync off. A chat's folder stays; the user's own checkout (from
    /// an earlier version) goes back. `notice` is what the user hears when
    /// this wasn't their own choice.
    async fn end(&self, key: &Path, notice: Option<String>) -> Result<Status> {
        let chat = self.state(key).map(|state| state.chat());
        self.halt_worker(key).await;
        match (self.state(key), chat) {
            (Some(state), _) if state.mode == Mode::Folder => self.stopped(key, notice),
            (Some(_), _) => self.restore(key, notice).await,
            // Its worker already put it back.
            (None, Some(chat)) => Ok(self.status(&chat.0, &chat.1, &chat.2)),
            (None, None) => anyhow::bail!("This chat isn't synced to a folder here."),
        }
    }

    /// Mark a chat's folder as no longer syncing (its worker is stopped, or
    /// is the caller).
    fn stopped(&self, key: &Path, notice: Option<String>) -> Result<Status> {
        let mut state = self.state(key).context("Sync stopped.")?;
        if state.phase != Phase::Stopped {
            state.phase = Phase::Stopped;
            state.pending = None;
            self.save_state(key, &state)?;
        }
        let status = {
            let mut records = lock(&self.records);
            let record = records.get_mut(key).context("Sync stopped.")?;
            record.status.enabled = false;
            record.status.state = "off";
            record.status.error = None;
            record.status.update_required = false;
            record.status.notice = notice;
            record.status.clone()
        };
        self.report(state.chat());
        Ok(status)
    }

    /// Forget a chat's folder record (the folder itself is already gone or
    /// stays as the user's).
    fn forget(&self, key: &Path) {
        let record = lock(&self.records).remove(key);
        lock(&self.archived).remove(key);
        if let Some(record) = record {
            let _ = fs::remove_file(&record.file);
            if let Some(profile) = lock(&self.loaded).clone() {
                let _ = fs::remove_dir_all(objects_dir(&profile, key));
            }
            self.report(record.state.chat());
        }
    }

    /// Stop syncing and remove the chat's folder.
    async fn delete(&self, key: &Path) -> Result<Status> {
        let state = self
            .state(key)
            .context("This chat isn't synced to a folder here.")?;
        ensure!(
            state.mode == Mode::Folder,
            "This chat's sync uses your own checkout; it can't be deleted."
        );
        self.halt_worker(key).await;
        let path = state.path.clone();
        tokio::task::spawn_blocking(move || repo::remove_folder(&path))
            .await
            .context("Deleting the folder was interrupted.")??;
        self.forget(key);
        lock(&self.notices).remove(&state.chat());
        Ok(Status::off(&state.workspace_id, &state.chat_id))
    }

    /// The chat was archived: remove its folder when it holds nothing but
    /// the chat's files, otherwise keep it, stopped, and say so.
    async fn archived(&self, key: &Path) -> Result<()> {
        let state = self.state(key).context("Sync stopped.")?;
        let path = state.path.clone();
        let checkpoint = state.checkpoint.clone();
        let untouched = tokio::task::spawn_blocking(move || {
            !path.exists()
                || checkpoint.is_some_and(|checkpoint| repo::untouched(&path, &checkpoint))
        })
        .await
        .unwrap_or(false);
        if untouched {
            let path = state.path.clone();
            tokio::task::spawn_blocking(move || repo::remove_folder(&path)).await??;
            self.forget(key);
        } else {
            self.stopped(key, Some(Ending::Archived.notice(&state)))?;
        }
        Ok(())
    }

    /// Put the user's own checkout back (a sync from an earlier version);
    /// the record goes once it is. The caller has stopped its worker.
    async fn restore(&self, key: &Path, notice: Option<String>) -> Result<Status> {
        ensure!(
            lock(&self.restoring).insert(key.to_owned()),
            "The folder is being put back already."
        );
        let _slot = RestoreSlot(self, key.to_owned());
        let Some(mut state) = self.state(key) else {
            anyhow::bail!("This chat isn't synced to a folder here.");
        };
        ensure!(
            state.mode == Mode::Checkout,
            "This chat's folder is its own."
        );
        if state.phase != Phase::Restoring {
            state.phase = Phase::Restoring;
            self.save_state(key, &state)?;
        }
        if let Some(record) = lock(&self.records).get_mut(key) {
            record.status.enabled = false;
            record.status.state = "restoring";
            record.status.error = None;
        }
        let path = state.path.clone();
        let saved = state.saved();
        let restored = tokio::task::spawn_blocking(move || match saved {
            Some(saved) if path.exists() => repo::restore(&path, &saved, true).map(Some),
            // Never checked out, or the folder is gone: nothing to put back.
            _ => Ok(None),
        })
        .await
        .context("Restoring the folder was interrupted.")?;
        match restored {
            Ok(restored) => {
                let file = lock(&self.records).remove(key).map(|r| r.file);
                if let Some(file) = file {
                    let _ = fs::remove_file(&file);
                }
                if let Some(profile) = lock(&self.loaded).clone() {
                    let _ = fs::remove_dir_all(objects_dir(&profile, key));
                }
                let mut said = notice.into_iter().collect::<Vec<_>>();
                if restored.as_ref().is_some_and(|r| r.kept_edits) {
                    said.push(
                        "Edits made in the folder while synced are in its stash list (git stash list)."
                            .into(),
                    );
                }
                if restored.as_ref().is_some_and(|r| r.changes_in_stash_list) {
                    said.push("Your branch moved while synced, so your earlier changes are in the stash list (git stash list).".into());
                }
                let chat = state.chat();
                let mut notices = lock(&self.notices);
                if said.is_empty() {
                    notices.remove(&chat);
                } else {
                    notices.insert(chat.clone(), said.join(" "));
                }
                drop(notices);
                let status = self.status(&chat.0, &chat.1, &chat.2);
                self.report(chat);
                Ok(status)
            }
            Err(error) => {
                let message = format!("Couldn't put {} back yet: {error}", state.path.display());
                if let Some(record) = lock(&self.records).get_mut(key) {
                    record.status.state = "error";
                    record.status.error = Some(message.clone());
                }
                Err(anyhow::anyhow!(message))
            }
        }
    }

    /// Move a chat synced into the user's own checkout (an earlier version)
    /// to a folder of its own: put the checkout back exactly first, then
    /// make the chat's folder from it. Returns the new folder's key.
    async fn migrate(&self, profile: &Path, root: &Path, key: &Path) -> Result<PathBuf> {
        self.halt_worker(key).await;
        let mut state = self.state(key).context("Sync stopped.")?;
        ensure!(
            state.mode == Mode::Checkout,
            "This chat's folder is its own."
        );
        if !state.migrating {
            state.migrating = true;
            self.save_state(key, &state)?;
        }
        let checkout = state.path.clone();
        self.restore(key, None).await?;
        let chat = state.chat();
        let earlier = lock(&self.notices).remove(&chat);
        let source = checkout.clone();
        let repository = tokio::task::spawn_blocking(move || repo::remote(&source))
            .await
            .ok()
            .flatten()
            .map(|remote| normalize(&remote))
            .unwrap_or_else(|| "repository".into());
        let path = self.new_folder(root, &repository, &chat.2)?;
        let folder = State::folder(
            &chat.0,
            &chat.1,
            &chat.2,
            path,
            &repository,
            checkout.is_dir().then(|| checkout.clone()),
            None,
        );
        let key = self.insert(profile, folder)?;
        let mut notice = format!(
            "Sync now uses a folder of its own, so {} went back to how you left it.",
            checkout.display()
        );
        if let Some(earlier) = earlier {
            notice = format!("{notice} {earlier}");
        }
        if let Some(record) = lock(&self.records).get_mut(&key) {
            record.status.notice = Some(notice);
        }
        Ok(key)
    }
}

/// Keep persisted syncs in step with the signed-in account: finish
/// interrupted restores, move syncs of the user's own checkout to folders of
/// their own, stop after an account switch, resume the rest. Started with
/// the engine; idempotent.
pub(super) fn supervise(auth: Auth) {
    let Ok(api) = CloudApi::shared() else {
        return;
    };
    if api.hosted {
        return;
    }
    let profile = auth.profile_dir().to_owned();
    if profile.as_os_str().is_empty() {
        return;
    }
    let shared = api.clone();
    let mut slot = lock(&shared.checkouts.supervisor);
    if slot.as_ref().is_some_and(|task| !task.is_finished()) {
        return;
    }
    *slot = Some(tokio::spawn(async move {
        let mut changes = auth.watch_state();
        loop {
            reconcile(&api, &auth, &profile).await;
            tokio::select! {
                changed = changes.changed() => if changed.is_err() { break },
                _ = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
        }
    }));
}

async fn reconcile(api: &Arc<CloudApi>, auth: &Auth, profile: &Path) {
    api.checkouts.hydrate(profile);
    let Ok(_change) = api.checkouts.changes.try_lock() else {
        return;
    };
    let current = auth.user_id().filter(|account| authorized(auth, account));
    let records = lock(&api.checkouts.records)
        .iter()
        .map(|(key, r)| (key.clone(), r.state.clone()))
        .collect::<Vec<_>>();
    for (key, state) in records {
        if lock(&api.checkouts.restoring).contains(&key) {
            continue;
        }
        let ours = current.as_deref() == Some(&state.account_id);
        match (state.mode, state.phase) {
            // Put back first, whatever else is going on: never left half way.
            (Mode::Checkout, Phase::Restoring) if !(state.migrating && ours) => {
                let _ = api.checkouts.restore(&key, None).await;
            }
            (Mode::Checkout, _) if !ours => {
                let _ = api
                    .checkouts
                    .end(&key, Some(Ending::Account.notice(&state)))
                    .await;
            }
            (Mode::Checkout, _) => {
                let migrated = match folders_root() {
                    Ok(root) => api.checkouts.migrate(profile, &root, &key).await,
                    Err(error) => Err(error),
                };
                match migrated {
                    Ok(folder) => Manager::start(api, auth, &folder),
                    Err(error) => tracing::info!(%error, "checkout sync: not moved yet"),
                }
            }
            (Mode::Folder, Phase::On) if !ours => {
                let _ = api
                    .checkouts
                    .end(&key, Some(Ending::Account.notice(&state)))
                    .await;
            }
            (Mode::Folder, Phase::On) => Manager::start(api, auth, &key),
            (Mode::Folder, _) if ours => {
                // A kept folder goes with its chat once that is archived.
                if chat_archived(api, auth, &state).await {
                    let misses = {
                        let mut archived = lock(&api.checkouts.archived);
                        let misses = archived.entry(key.clone()).or_default();
                        *misses += 1;
                        *misses
                    };
                    if misses >= 2 {
                        let _ = api.checkouts.archived(&key).await;
                    }
                } else {
                    lock(&api.checkouts.archived).remove(&key);
                }
            }
            (Mode::Folder, _) => {}
        }
    }
}

/// Whether a stopped sync's chat is archived, as far as can be told now.
async fn chat_archived(api: &CloudApi, auth: &Auth, state: &State) -> bool {
    match workspace_row(api, auth, state).await {
        Ok(row) if gone(&row) => true,
        Ok(_) => chat_active(api, auth, state, false).await == Some(false),
        Err(_) => false,
    }
}

fn gone(row: &Workspace) -> bool {
    row.archived() || row.deleting() || matches!(row.state.as_str(), "deleted" | "deleting")
}

enum Cycle {
    /// The folder moved to a new checkpoint of this many files.
    Changed(usize),
    /// The chat's files are what the folder already has.
    Same,
    /// Waiting for the cloud computer, with why.
    Behind(String),
    /// Edited here: waiting for the user.
    Paused(String),
    /// The cloud computer must be updated first.
    Update(String),
}

async fn run(
    api: Arc<CloudApi>,
    auth: Auth,
    key: PathBuf,
    cancel: CancellationToken,
    wake: Arc<Notify>,
) {
    let mut known: Option<HashMap<String, String>> = None;
    let mut delay = MIN_DELAY;
    let mut chat_checked: Option<tokio::time::Instant> = None;
    let mut chat_missing = 0u8;
    let mut chat_listed = false;
    // Whether the folder held the chat's files when last reported.
    let mut held: Option<bool> = None;
    loop {
        if cancel.is_cancelled() {
            break;
        }
        let Some(state) = api.checkouts.state(&key) else {
            break;
        };
        if state.phase != Phase::On {
            break;
        }
        let row = workspace_row(&api, &auth, &state).await;
        if let Some(ending) = ended(
            &api,
            &auth,
            &state,
            &row,
            &mut chat_checked,
            (&mut chat_missing, &mut chat_listed),
        )
        .await
        {
            if !cancel.is_cancelled() {
                let _ = match ending {
                    Ending::Archived => api.checkouts.archived(&key).await,
                    _ => api
                        .checkouts
                        .stopped(&key, Some(ending.notice(&state)))
                        .map(|_| ()),
                };
            }
            return;
        }
        api.checkouts.set_status(&key, &cancel, |s| {
            if s.state != "synced" {
                s.state = "syncing";
            }
        });
        let outcome = cycle(&api, &key, &state, row, &mut known, &cancel).await;
        // Commands start in the folder only while it holds the chat's files.
        if let Some(now) = api.checkouts.state(&key) {
            let holding = match now.checkpoint.clone() {
                Some(checkpoint) => {
                    let dir = now.path.clone();
                    tokio::task::spawn_blocking(move || repo::holds(&dir, &checkpoint))
                        .await
                        .unwrap_or(false)
                }
                None => false,
            };
            if held.is_some_and(|before| before != holding) {
                api.checkouts.report(now.chat());
            }
            held = Some(holding);
        }
        match outcome {
            Ok(Cycle::Changed(files)) => {
                delay = MIN_DELAY;
                api.checkouts.set_status(&key, &cancel, |s| {
                    s.state = "synced";
                    s.error = None;
                    s.update_required = false;
                    s.files = files;
                    s.last_synced_at = Some(chrono::Utc::now().timestamp_millis());
                });
            }
            Ok(Cycle::Same) => {
                delay = (delay * 2).min(MAX_DELAY);
                api.checkouts.set_status(&key, &cancel, |s| {
                    s.state = "synced";
                    s.error = None;
                    s.update_required = false;
                    s.last_synced_at = Some(chrono::Utc::now().timestamp_millis());
                });
            }
            Ok(Cycle::Behind(why)) => {
                delay = (delay * 2).min(MAX_DELAY);
                api.checkouts.set_status(&key, &cancel, |s| {
                    s.state = "behind";
                    s.update_required = false;
                    s.error = Some(why);
                });
            }
            Ok(Cycle::Paused(why)) => {
                delay = MIN_DELAY.max(Duration::from_secs(5));
                api.checkouts.set_status(&key, &cancel, |s| {
                    s.state = "paused";
                    s.update_required = false;
                    s.error = Some(why);
                });
            }
            Ok(Cycle::Update(why)) => {
                delay = MAX_DELAY;
                api.checkouts.set_status(&key, &cancel, |s| {
                    s.state = "error";
                    s.update_required = true;
                    s.error = Some(why);
                });
            }
            Err(error) => {
                if cancel.is_cancelled() {
                    break;
                }
                delay = Duration::from_secs(15);
                api.checkouts.set_status(&key, &cancel, |s| {
                    s.state = "error";
                    s.update_required = false;
                    s.error = Some(error.to_string());
                });
            }
        }
        tokio::select! {biased;
            _ = cancel.cancelled() => break,
            _ = wake.notified() => delay = MIN_DELAY,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

/// The chat's workspace row, or why it couldn't be read.
async fn workspace_row(
    api: &CloudApi,
    auth: &Auth,
    state: &State,
) -> std::result::Result<Workspace, String> {
    let token = auth
        .access_token()
        .await
        .map_err(|_| "Sign in to keep syncing.".to_owned())?;
    let value = api
        .request(
            &token,
            &["v1", "cloud", "workspaces", &state.workspace_id],
            None,
        )
        .await
        .map_err(|error| error.to_string())?;
    let value = value.get("workspace").unwrap_or(&value).clone();
    let row: Workspace = serde_json::from_value(value).map_err(|error| error.to_string())?;
    if row.workspace_id != state.workspace_id {
        return Err("Cloud workspace identity changed.".into());
    }
    Ok(row)
}

/// Why sync must end now on its own, if it must.
async fn ended(
    api: &CloudApi,
    auth: &Auth,
    state: &State,
    row: &std::result::Result<Workspace, String>,
    chat_checked: &mut Option<tokio::time::Instant>,
    (chat_missing, chat_listed): (&mut u8, &mut bool),
) -> Option<Ending> {
    if !authorized(auth, &state.account_id) {
        return Some(Ending::Account);
    }
    if !crate::computer_bridge::enabled_for(auth.profile_dir(), &state.account_id) {
        return Some(Ending::Connector);
    }
    if let Ok(row) = row
        && gone(row)
    {
        return Some(Ending::Archived);
    }
    let now = tokio::time::Instant::now();
    if chat_checked.is_none_or(|at| now.duration_since(at) >= CHAT_CHECK) {
        *chat_checked = Some(now);
        match chat_active(api, auth, state, *chat_listed).await {
            Some(false) => *chat_missing += 1,
            Some(true) => {
                *chat_missing = 0;
                *chat_listed = true;
            }
            None => {}
        }
        // Twice in a row, so one odd listing never ends a sync.
        if *chat_missing >= 2 {
            return Some(Ending::Archived);
        }
    }
    None
}

/// Whether the chat is still active: `None` when that can't be told.
/// `listed`: an earlier listing showed this chat, so this computer records
/// its chats and an empty listing means its last chat was archived.
async fn chat_active(api: &CloudApi, auth: &Auth, state: &State, listed: bool) -> Option<bool> {
    let token = auth.access_token().await.ok()?;
    let listing = api
        .request(
            &token,
            &["v1", "cloud", "workspaces", &state.workspace_id, "sessions"],
            None,
        )
        .await
        .ok()?;
    let items = listing["items"].as_array()?;
    if items.iter().any(|chat| {
        chat["sessionId"] == state.chat_id.as_str()
            || chat["session_id"] == state.chat_id.as_str()
            || chat["id"] == state.chat_id.as_str()
    }) {
        return Some(true);
    }
    // Only a complete listing from a computer that records its chats can
    // tell a chat is gone.
    ((listed || !items.is_empty()) && listing["nextCursor"].is_null()).then_some(false)
}

/// Make the chat's folder when it isn't there (first time, or the user
/// removed it): a worktree of the user's clone when there is a usable one,
/// otherwise a clone. Whatever was recorded about the old folder goes.
async fn prepare_folder(api: &CloudApi, key: &Path, state: &State) -> Result<State> {
    let path = state.path.clone();
    let made = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || repo::is_folder(&path)).await?
    };
    if made {
        return Ok(state.clone());
    }
    let mut fresh = state.clone();
    fresh.branch = None;
    fresh.head = None;
    fresh.changes = None;
    fresh.checkpoint = None;
    fresh.pending = None;
    api.checkouts.save_state(key, &fresh)?;
    let repository = state
        .repository
        .clone()
        .context("This chat's repository isn't known.")?;
    let url = state
        .url
        .clone()
        .unwrap_or_else(|| format!("https://{repository}.git"));
    let source = state.source.clone();
    let how = tokio::task::spawn_blocking(move || {
        // Only a clone of this very repository can lend its objects.
        let source = source
            .and_then(|source| repo::clone_root(&source))
            .filter(|source| {
                repo::remote(source).map(|remote| normalize(&remote)) == Some(repository.clone())
            });
        repo::create_folder(&path, source.as_deref(), &url)
    })
    .await??;
    if how != repo::Made::Worktree && fresh.source.is_some() {
        fresh.source = None;
        api.checkouts.save_state(key, &fresh)?;
    }
    if let Some(record) = lock(&api.checkouts.records).get_mut(key) {
        record.status.source_path = fresh.source.clone();
    }
    Ok(fresh)
}

/// One snapshot of the chat's files into `objects`, verified completely.
/// A dropped connection keeps what arrived and tries again.
async fn snapshot(
    api: &CloudApi,
    state: &State,
    objects: &Path,
    have: Vec<String>,
    cancel: &CancellationToken,
) -> Result<std::result::Result<Vec<mirror::ReceivedFile>, Cycle>> {
    for attempt in 1..=SNAPSHOT_ATTEMPTS {
        // The environment stays held for the whole transfer: letting it go
        // (the chat closed meanwhile) would close the connection mid-way.
        let environment = api.environments.get(&state.account_id, &state.workspace_id);
        let Some(client) = environment
            .as_ref()
            .and_then(|environment| environment.ready_client())
        else {
            return Ok(Err(Cycle::Behind(
                // Like "Sync to a local directory", sync rides the connection
                // an open chat holds; it never connects (or wakes) on its own.
                "Open this chat to keep syncing; its changes arrive once it connects.".into(),
            )));
        };
        let received = mirror::receive_snapshot(
            &client,
            WorkspaceTarget {
                chat_id: Some(state.chat_id.clone()),
                space_id: None,
                checkout_path: None,
            },
            &state.workspace_id,
            objects,
            have.clone(),
            cancel.clone(),
        )
        .await;
        drop(environment);
        match received {
            Ok(files) => return Ok(Ok(files)),
            Err(error) if error.downcast_ref::<mirror::UpdateRequired>().is_some() => {
                return Ok(Err(Cycle::Update(error.to_string())));
            }
            Err(error) if error.downcast_ref::<mirror::Interrupted>().is_some() => {
                ensure!(!cancel.is_cancelled(), "Sync stopped.");
                tracing::info!(attempt, "checkout sync: snapshot interrupted, resuming");
            }
            Err(error) => return Err(error),
        }
    }
    Ok(Err(Cycle::Behind(
        "The connection to the cloud computer dropped while sending files. Sync continues where it stopped."
            .into(),
    )))
}

/// One sync step: take a snapshot of the chat's files and move the folder
/// to it, unless it was edited here.
async fn cycle(
    api: &Arc<CloudApi>,
    key: &Path,
    state: &State,
    row: std::result::Result<Workspace, String>,
    known: &mut Option<HashMap<String, String>>,
    cancel: &CancellationToken,
) -> Result<Cycle> {
    let row = match row {
        Ok(row) => row,
        Err(error) => return Ok(Cycle::Behind(error)),
    };
    if row.rpc_protocol.as_deref() != Some("zuse-rpc-v1") {
        return Ok(Cycle::Update(
            "Update this cloud computer to sync its files.".into(),
        ));
    }
    if row.state != "ready" || row.desired_state != "ready" {
        return Ok(Cycle::Behind(
            "The cloud computer is asleep. Its changes arrive when it wakes.".into(),
        ));
    }
    let mut state = state.clone();
    if state.mode == Mode::Folder {
        if !repo::is_folder(&state.path) {
            api.checkouts
                .set_status(key, cancel, |s| s.state = "preparing");
            *known = None;
        }
        state = prepare_folder(api, key, &state).await?;
    }
    let path = state.path.clone();
    ensure!(
        path.is_dir(),
        "{} isn't on this computer anymore.",
        path.display()
    );
    // A move recorded but not confirmed: finished if HEAD got there.
    if let Some(pending) = state.pending.clone() {
        let dir = path.clone();
        let target = pending.clone();
        let arrived = tokio::task::spawn_blocking(move || repo::holds(&dir, &target)).await?;
        if arrived {
            state.checkpoint = Some(pending);
        }
        state.pending = None;
        api.checkouts.save_state(key, &state)?;
    }
    let state = &state;
    if state.checkpoint.is_none() && state.head.is_some() {
        // Saved, then stopped before the first checkpoint was recorded: put
        // the folder back to what was saved before saving it again.
        let dir = path.clone();
        let saved = state.saved();
        tokio::task::spawn_blocking(move || match saved {
            Some(saved) => repo::restore(&dir, &saved, false).map(|_| ()),
            None => Ok(()),
        })
        .await??;
        let mut fresh = state.clone();
        fresh.branch = None;
        fresh.head = None;
        fresh.changes = None;
        api.checkouts.save_state(key, &fresh)?;
        return Box::pin(cycle(api, key, &fresh, Ok(row), known, cancel)).await;
    }
    if let Some(checkpoint) = state.checkpoint.clone() {
        let dir = path.clone();
        if let Some(reason) =
            tokio::task::spawn_blocking(move || repo::local_edits(&dir, &checkpoint)).await??
        {
            return Ok(Cycle::Paused(reason.into()));
        }
    }
    if known.is_none() {
        let dir = path.clone();
        let base = state.head.clone().unwrap_or_else(|| "HEAD".into());
        *known = Some(
            tokio::task::spawn_blocking(move || repo::known_blobs(&dir, &base))
                .await?
                .unwrap_or_default(),
        );
    }
    let profile = lock(&api.checkouts.loaded)
        .clone()
        .context("Sync stopped.")?;
    // Kept across attempts: what an interrupted snapshot verified is not
    // sent again.
    let objects = objects_dir(&profile, key);
    fs::create_dir_all(&objects)?;
    let have: Vec<String> = known
        .as_ref()
        .map(|known| known.keys().cloned().collect())
        .unwrap_or_default();
    let files = match snapshot(api, state, &objects, have, cancel).await? {
        Ok(files) => files,
        Err(waiting) => return Ok(waiting),
    };
    ensure!(!cancel.is_cancelled(), "Sync stopped.");
    // Verified completely; only now does anything in the folder change.
    let count = files.len();
    let mut blobs = known.take().unwrap_or_default();
    let state = state.clone();
    let dir = path.clone();
    let built = tokio::task::spawn_blocking(move || {
        let parent = match &state.head {
            Some(head) => head.clone(),
            None => repo::head(&dir)?,
        };
        let commit = repo::checkpoint(
            &dir,
            &files,
            &objects,
            &mut blobs,
            &parent,
            "Cloud chat files (synced by Zuse)",
        );
        let _ = fs::remove_dir_all(&objects);
        let commit = commit?;
        let same = match &state.checkpoint {
            Some(current) => repo::tree(&dir, current)? == repo::tree(&dir, &commit)?,
            None => false,
        };
        Ok::<_, anyhow::Error>((commit, same, blobs))
    })
    .await?;
    let (commit, same, blobs) = built?;
    *known = Some(blobs);
    if same {
        return Ok(Cycle::Same);
    }
    ensure!(!cancel.is_cancelled(), "Sync stopped.");
    let current = api.checkouts.state(key).context("Sync stopped.")?;
    match current.checkpoint.clone() {
        None => {
            // First checkpoint: save the folder as it is right now, persist
            // that, then move it.
            let dir = path.clone();
            let saved = tokio::task::spawn_blocking(move || repo::save(&dir)).await??;
            let mut next = current.clone();
            next.branch = saved.branch.clone();
            next.head = Some(saved.head.clone());
            next.changes = saved.changes.clone();
            next.pending = Some(commit.clone());
            api.checkouts.save_state(key, &next)?;
            let dir = path.clone();
            let to = commit.clone();
            let entered = {
                let saved = saved.clone();
                tokio::task::spawn_blocking(move || repo::enter(&dir, &saved, &to)).await?
            };
            let saved = saved.clone();
            if let Err(error) = entered {
                // Put the folder back from wherever entering stopped; the
                // saved state stays recorded until that succeeded.
                let dir = path.clone();
                tokio::task::spawn_blocking(move || repo::restore(&dir, &saved, false)).await??;
                let mut back = current.clone();
                back.branch = None;
                back.head = None;
                back.changes = None;
                back.pending = None;
                api.checkouts.save_state(key, &back)?;
                return Err(error);
            }
            next.checkpoint = Some(commit);
            next.pending = None;
            api.checkouts.save_state(key, &next)?;
            api.checkouts.report(next.chat());
        }
        Some(from) => {
            let mut moving = current.clone();
            moving.pending = Some(commit.clone());
            api.checkouts.save_state(key, &moving)?;
            let dir = path.clone();
            let to = commit.clone();
            let advanced =
                tokio::task::spawn_blocking(move || repo::advance(&dir, &from, &to)).await?;
            if let Err(error) = advanced {
                let dir = path.clone();
                let at = current.checkpoint.clone().unwrap_or_default();
                if let Ok(Ok(Some(reason))) =
                    tokio::task::spawn_blocking(move || repo::local_edits(&dir, &at)).await
                {
                    return Ok(Cycle::Paused(reason.into()));
                }
                return Err(error);
            }
            let mut next = current.clone();
            next.checkpoint = Some(commit);
            next.pending = None;
            api.checkouts.save_state(key, &next)?;
        }
    }
    Ok(Cycle::Changed(count))
}

/// The connected repository a cloud workspace was started from, as
/// `host/owner/repo` (lowercase), and its clone URL when Cloud knows one.
async fn repository(
    api: &CloudApi,
    auth: &Auth,
    account: &str,
    row: &Value,
) -> Result<(String, Option<String>)> {
    let token = auth
        .access_token()
        .await
        .map_err(|_| anyhow::anyhow!("Sign in to sync this chat."))?;
    ensure!(
        authorized(auth, account),
        "The account changed. Select the chat again."
    );
    let catalog = api
        .request(&token, &["v1", "cloud", "context"], None)
        .await?;
    let project = catalog["projects"]
        .as_array()
        .and_then(|rows| rows.iter().find(|p| p["projectId"] == row["projectId"]))
        .context("This chat's repository isn't connected to Zuse Cloud.")?;
    let identity = project["repositoryIdentity"]
        .as_str()
        .or_else(|| project["repositoryUrl"].as_str())
        .context("This chat's repository isn't connected to Zuse Cloud.")?;
    let url = project["repositoryUrl"]
        .as_str()
        .filter(|url| url.starts_with("https://") || url.starts_with("git@"))
        .map(str::to_owned);
    Ok((normalize(identity), url))
}

/// One spelling for a repository: lowercase `host/owner/repo`.
fn normalize(identity: &str) -> String {
    let mut value = identity.trim().to_ascii_lowercase();
    for scheme in ["https://", "http://", "ssh://", "git://"] {
        if let Some(rest) = value.strip_prefix(scheme) {
            value = rest.to_owned();
        }
    }
    if let Some(rest) = value.strip_prefix("git@") {
        value = rest.replacen(':', "/", 1);
    }
    if let Some((_, rest)) = value.split_once('@') {
        value = rest.to_owned();
    }
    let value = value.trim_end_matches('/');
    value.strip_suffix(".git").unwrap_or(value).to_owned()
}

fn segment<'a>(params: &'a Value, name: &str, what: &str) -> Result<&'a str> {
    params[name]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 512
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
        })
        .with_context(|| format!("Select a {what}."))
}

pub async fn handle(auth: Auth, method: &str, params: Value) -> Result<Value> {
    let api = CloudApi::shared()?;
    ensure!(
        !api.hosted && (auth.workos_enabled() || api.loopback),
        "Sign in to sync a cloud chat."
    );
    let account = auth.user_id().context("Sign in to sync a cloud chat.")?;
    ensure!(
        params["expectedAccountId"].as_str() == Some(&account) && authorized(&auth, &account),
        "The account changed. Select the chat again."
    );
    let workspace = segment(&params, "workspaceId", "cloud chat")?.to_owned();
    let chat = segment(&params, "chatId", "cloud chat")?.to_owned();
    let profile = auth.profile_dir().to_owned();
    supervise(auth.clone());
    api.checkouts.hydrate(&profile);
    let reply = |status: Status| json!({"accountId":account,"status":status});
    if method == methods::CLOUD_CHECKOUT_SYNC_STATUS {
        return Ok(reply(api.checkouts.status(&account, &workspace, &chat)));
    }
    let _change = api.checkouts.changes.lock().await;
    let existing = api.checkouts.by_chat(&account, &workspace, &chat);
    let status = |api: &CloudApi| api.checkouts.status(&account, &workspace, &chat);
    if method == methods::CLOUD_CHECKOUT_SYNC_DELETE_FOLDER {
        let key = existing.context("This chat has no folder here.")?;
        return Ok(reply(api.checkouts.delete(&key).await?));
    }
    if method == methods::CLOUD_CHECKOUT_SYNC_RESUME {
        let key = existing.context("This chat isn't synced to a folder here.")?;
        let state = api.checkouts.state(&key).context("Sync stopped.")?;
        if state.mode == Mode::Checkout {
            // An earlier version's sync of the user's checkout: put it back,
            // then on to a folder of its own unless it was being turned off.
            if state.phase == Phase::Restoring && !state.migrating {
                api.checkouts.restore(&key, None).await?;
            } else {
                let folder = api
                    .checkouts
                    .migrate(&profile, &folders_root()?, &key)
                    .await?;
                Manager::start(&api, &auth, &folder);
            }
            return Ok(reply(status(&api)));
        }
        if state.phase == Phase::On
            && let Some(checkpoint) = state.checkpoint.clone()
        {
            // Paused by edits made in the folder: the user chose to discard
            // them. The folder is the chat's own, so they don't go to the
            // stash list (which the user's clone shares).
            api.checkouts.halt_worker(&key).await;
            let dir = state.path.clone();
            let paused = {
                let dir = dir.clone();
                let checkpoint = checkpoint.clone();
                tokio::task::spawn_blocking(move || repo::local_edits(&dir, &checkpoint)).await??
            };
            if paused.is_some() {
                tokio::task::spawn_blocking(move || repo::return_to(&dir, &checkpoint)).await??;
            }
        }
        if let Some(record) = lock(&api.checkouts.records).get_mut(&key) {
            record.status.state = "syncing";
            record.status.error = None;
            record.status.notice = None;
            record.wake.notify_one();
        }
        Manager::start(&api, &auth, &key);
        return Ok(reply(status(&api)));
    }
    ensure!(
        method == methods::CLOUD_CHECKOUT_SYNC_CONFIGURE,
        "Unknown sync request."
    );
    let enabled = params["enabled"]
        .as_bool()
        .context("Choose whether to sync this chat.")?;
    if !enabled {
        let Some(key) = existing else {
            lock(&api.checkouts.notices).remove(&(
                account.clone(),
                workspace.clone(),
                chat.clone(),
            ));
            return Ok(reply(Status::off(&workspace, &chat)));
        };
        return Ok(reply(api.checkouts.end(&key, None).await?));
    }
    ensure!(
        crate::computer_bridge::enabled_for(&profile, &account),
        "Allow cloud chats to use this computer first (Settings → Cloud computer → Your computers)."
    );
    lock(&api.checkouts.notices).remove(&(account.clone(), workspace.clone(), chat.clone()));
    if let Some(key) = existing {
        let state = api.checkouts.state(&key).context("Sync stopped.")?;
        let key = match state.mode {
            Mode::Checkout => {
                api.checkouts
                    .migrate(&profile, &folders_root()?, &key)
                    .await?
            }
            Mode::Folder => {
                if state.phase != Phase::On {
                    let mut on = state.clone();
                    on.phase = Phase::On;
                    api.checkouts.save_state(&key, &on)?;
                    if let Some(record) = lock(&api.checkouts.records).get_mut(&key) {
                        record.status = Status::on(&on);
                    }
                }
                key
            }
        };
        Manager::start(&api, &auth, &key);
        return Ok(reply(status(&api)));
    }
    let value = {
        let token = auth
            .access_token()
            .await
            .map_err(|_| anyhow::anyhow!("Sign in to sync this chat."))?;
        let value = api
            .request(&token, &["v1", "cloud", "workspaces", &workspace], None)
            .await?;
        value.get("workspace").unwrap_or(&value).clone()
    };
    let row: Workspace = serde_json::from_value(value.clone())?;
    ensure!(
        row.workspace_id == workspace,
        "Cloud workspace identity changed."
    );
    ensure!(!gone(&row), "Unarchive this chat before syncing it.");
    ensure!(
        row.rpc_protocol.as_deref() == Some("zuse-rpc-v1"),
        "Update this cloud computer before syncing its files."
    );
    let (repository, url) = repository(&api, &auth, &account, &value).await?;
    // The user's clone of the repository lends its objects to the chat's
    // folder; it is checked again when the folder is made.
    let source = params["sourcePath"]
        .as_str()
        .or_else(|| params["localPath"].as_str())
        .filter(|s| !s.trim().is_empty() && s.len() <= 4096)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let source = match source {
        Some(source) => {
            let expected = repository.clone();
            tokio::task::spawn_blocking(move || {
                let root = repo::clone_root(&source)?;
                (repo::remote(&root).map(|r| normalize(&r)) == Some(expected)).then_some(root)
            })
            .await?
        }
        None => None,
    };
    let path = api
        .checkouts
        .new_folder(&folders_root()?, &repository, &chat)?;
    let state = State::folder(&account, &workspace, &chat, path, &repository, source, url);
    let key = api.checkouts.insert(&profile, state)?;
    Manager::start(&api, &auth, &key);
    Ok(reply(status(&api)))
}
