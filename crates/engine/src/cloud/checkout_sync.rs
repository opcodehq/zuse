//! A cloud chat's files in the user's own local checkout of the same
//! repository, one way, so the tools already running there (a dev server,
//! containers, builds) pick up what the cloud agent changes.
//!
//! Turning it on saves the checkout exactly (branch, HEAD and tracked
//! changes, pinned under hidden refs and persisted in the profile), then
//! checks out a checkpoint commit of the chat's files, detached. While on,
//! each new snapshot of the chat's checkout becomes the next checkpoint.
//! Edits made in the folder pause sync instead of being overwritten.
//! Turning it off, archiving the chat, stopping this computer's connector or
//! switching accounts restores the saved state; an interrupted restore
//! finishes after a restart. Untracked and ignored files are never touched.
//!
//! Snapshots come over the chat's existing computer connection (the same
//! transfer as "Sync to a local directory") and never wake compute.
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

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Phase {
    /// Synced, or waiting for its first checkpoint.
    On,
    /// Putting the checkout back began; it finishes even after a restart.
    Restoring,
}

/// What is persisted for one synced checkout, written before any file in
/// the checkout changes.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct State {
    version: u32,
    account_id: String,
    workspace_id: String,
    chat_id: String,
    path: PathBuf,
    phase: Phase,
    /// How the checkout was before its first checkpoint; absent until then.
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    head: Option<String>,
    #[serde(default)]
    changes: Option<String>,
    /// The checkpoint the checkout is on.
    #[serde(default)]
    checkpoint: Option<String>,
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
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    workspace_id: String,
    chat_id: String,
    enabled: bool,
    /// `off`, `preparing`, `syncing`, `synced`, `behind` (the cloud computer
    /// isn't reachable right now), `paused` (edited here), `restoring` or
    /// `error`.
    state: &'static str,
    local_path: Option<PathBuf>,
    last_synced_at: Option<i64>,
    files: usize,
    /// Why it is paused, behind or failed.
    error: Option<String>,
    /// Something to tell the user once, such as where kept edits went.
    notice: Option<String>,
}
impl Status {
    fn off(workspace: &str, chat: &str) -> Self {
        Self {
            workspace_id: workspace.into(),
            chat_id: chat.into(),
            enabled: false,
            state: "off",
            local_path: None,
            last_synced_at: None,
            files: 0,
            error: None,
            notice: None,
        }
    }
    fn on(state: &State) -> Self {
        Self {
            enabled: state.phase == Phase::On,
            state: match state.phase {
                Phase::On => "preparing",
                Phase::Restoring => "restoring",
            },
            local_path: Some(state.path.clone()),
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
    /// By checkout folder: one chat per checkout.
    records: Mutex<HashMap<PathBuf, Record>>,
    /// What the user should hear about a chat whose sync ended on its own.
    notices: Mutex<HashMap<(String, String, String), String>>,
    /// Serializes turning sync on and off.
    changes: tokio::sync::Mutex<()>,
    loaded: Mutex<Option<PathBuf>>,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Tells the cloud the chat's synced folder changed on this computer.
    reporter: Mutex<Option<Reporter>>,
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
            (state.version == 1
                && state.path.is_absolute()
                && path == state_file(profile, &state.path))
            .then_some((path, state))
        })
        .collect()
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
            && state.checkpoint.is_some())
        .then_some(state.path)
    })
}

/// Persist a chat as synced into `path` (tests of the connector's report).
#[cfg(test)]
pub(crate) fn record_synced_for_test(
    profile: &Path,
    account: &str,
    workspace: &str,
    chat: &str,
    path: &Path,
) {
    let state = State {
        version: 1,
        account_id: account.into(),
        workspace_id: workspace.into(),
        chat_id: chat.into(),
        path: path.to_owned(),
        phase: Phase::On,
        branch: None,
        head: Some("0".repeat(40)),
        changes: None,
        checkpoint: Some("1".repeat(40)),
    };
    write_state(&state_file(profile, path), &state).unwrap();
}

/// Who reports a chat's synced folder to the cloud (the computer connector).
pub fn set_reporter(report: impl Fn(String, String, String) + Send + Sync + 'static) {
    if let Ok(api) = CloudApi::shared() {
        *lock(&api.checkouts.reporter) = Some(Arc::new(report));
    }
}

/// Re-check every synced checkout now (for example after this computer's
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

impl Manager {
    fn report(&self, chat: (String, String, String)) {
        let reporter = lock(&self.reporter).clone();
        if let Some(report) = reporter {
            report(chat.0, chat.1, chat.2);
        }
    }

    /// Load persisted checkouts once per profile.
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
        lock(&self.records)
            .iter()
            .find(|(_, r)| {
                r.state.account_id == account
                    && r.state.workspace_id == workspace
                    && r.state.chat_id == chat
            })
            .map(|(path, _)| path.clone())
    }

    fn status(&self, account: &str, workspace: &str, chat: &str) -> Status {
        let records = lock(&self.records);
        let found = records.values().find(|r| {
            r.state.account_id == account
                && r.state.workspace_id == workspace
                && r.state.chat_id == chat
        });
        match found {
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

    fn start(api: &Arc<CloudApi>, auth: &Auth, key: &Path) {
        let mut records = lock(&api.checkouts.records);
        let Some(record) = records.get_mut(key) else {
            return;
        };
        if record.state.phase != Phase::On
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

    /// Stop syncing `key` (waiting for its worker) and put the checkout back.
    /// `notice` is what the user hears when this wasn't their own choice.
    async fn end(&self, key: &Path, notice: Option<String>) -> Result<Status> {
        let chat = self.state(key).map(|state| state.chat());
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
        match (self.state(key), chat) {
            (Some(_), _) => self.restore(key, notice).await,
            // Its worker already put it back.
            (None, Some(chat)) => Ok(self.status(&chat.0, &chat.1, &chat.2)),
            (None, None) => anyhow::bail!("This chat isn't synced to a folder here."),
        }
    }

    /// Put the checkout back; the record goes once it is. The caller has
    /// stopped its worker (or is it).
    async fn restore(&self, key: &Path, notice: Option<String>) -> Result<Status> {
        let Some(mut state) = self.state(key) else {
            anyhow::bail!("This chat isn't synced to a folder here.");
        };
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
            Some(saved) if path.exists() => repo::restore(&path, &saved).map(Some),
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
}

/// Keep persisted checkouts in step with the signed-in account: finish
/// interrupted restores, restore after an account switch, resume the rest.
/// Started with the engine; idempotent.
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
        if state.phase == Phase::Restoring {
            let _ = api.checkouts.restore(&key, None).await;
        } else if current.as_deref() != Some(&state.account_id) {
            let _ = api
                .checkouts
                .end(
                    &key,
                    Some(format!(
                        "You switched accounts, so {} went back to how you left it.",
                        state.path.display()
                    )),
                )
                .await;
        } else {
            Manager::start(api, auth, &key);
        }
    }
}

enum Cycle {
    /// The checkout moved to a new checkpoint of this many files.
    Changed(usize),
    /// The chat's files are what the checkout already has.
    Same,
    /// Waiting for the cloud computer, with why.
    Behind(String),
    /// Edited here: waiting for the user.
    Paused(String),
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
        if let Some(reason) = ended(
            &api,
            &auth,
            &state,
            &row,
            &mut chat_checked,
            &mut chat_missing,
        )
        .await
        {
            if !cancel.is_cancelled() {
                let _ = api.checkouts.restore(&key, Some(reason)).await;
            }
            return;
        }
        api.checkouts.set_status(&key, &cancel, |s| {
            if s.state != "synced" {
                s.state = "syncing";
            }
        });
        match cycle(&api, &key, &state, row, &mut known, &cancel).await {
            Ok(Cycle::Changed(files)) => {
                delay = MIN_DELAY;
                api.checkouts.set_status(&key, &cancel, |s| {
                    s.state = "synced";
                    s.error = None;
                    s.files = files;
                    s.last_synced_at = Some(chrono::Utc::now().timestamp_millis());
                });
            }
            Ok(Cycle::Same) => {
                delay = (delay * 2).min(MAX_DELAY);
                api.checkouts.set_status(&key, &cancel, |s| {
                    s.state = "synced";
                    s.error = None;
                    s.last_synced_at = Some(chrono::Utc::now().timestamp_millis());
                });
            }
            Ok(Cycle::Behind(why)) => {
                delay = (delay * 2).min(MAX_DELAY);
                api.checkouts.set_status(&key, &cancel, |s| {
                    s.state = "behind";
                    s.error = Some(why);
                });
            }
            Ok(Cycle::Paused(why)) => {
                delay = MIN_DELAY.max(Duration::from_secs(5));
                api.checkouts.set_status(&key, &cancel, |s| {
                    s.state = "paused";
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
    chat_missing: &mut u8,
) -> Option<String> {
    let folder = state.path.display();
    if !authorized(auth, &state.account_id) {
        return Some(format!(
            "You switched accounts, so {folder} went back to how you left it."
        ));
    }
    if !crate::computer_bridge::enabled_for(auth.profile_dir(), &state.account_id) {
        return Some(format!(
            "This computer stopped running cloud chat commands, so {folder} went back to how you left it."
        ));
    }
    if let Ok(row) = row
        && (row.archived()
            || row.deleting()
            || matches!(row.state.as_str(), "deleted" | "deleting"))
    {
        return Some(format!(
            "This chat was archived, so {folder} went back to how you left it."
        ));
    }
    let now = tokio::time::Instant::now();
    if chat_checked.is_none_or(|at| now.duration_since(at) >= CHAT_CHECK) {
        *chat_checked = Some(now);
        match chat_active(api, auth, state).await {
            Some(false) => *chat_missing += 1,
            Some(true) => *chat_missing = 0,
            None => {}
        }
        // Twice in a row, so one odd listing never ends a sync.
        if *chat_missing >= 2 {
            return Some(format!(
                "This chat was archived, so {folder} went back to how you left it."
            ));
        }
    }
    None
}

/// Whether the chat is still active: `None` when that can't be told.
async fn chat_active(api: &CloudApi, auth: &Auth, state: &State) -> Option<bool> {
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
    (!items.is_empty() && listing["nextCursor"].is_null()).then_some(false)
}

/// One sync step: take a snapshot of the chat's files and move the checkout
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
        return Ok(Cycle::Behind(
            "Update this cloud computer to sync its files.".into(),
        ));
    }
    if row.state != "ready" || row.desired_state != "ready" {
        return Ok(Cycle::Behind(
            "The cloud computer is asleep. Its changes arrive when it wakes.".into(),
        ));
    }
    let path = state.path.clone();
    ensure!(
        path.is_dir(),
        "{} isn't on this computer anymore.",
        path.display()
    );
    if let Some(checkpoint) = state.checkpoint.clone() {
        let dir = path.clone();
        if let Some(reason) =
            tokio::task::spawn_blocking(move || repo::local_edits(&dir, &checkpoint)).await??
        {
            return Ok(Cycle::Paused(reason.into()));
        }
    }
    let Some(client) = api
        .environments
        .get(&state.account_id, &state.workspace_id)
        .and_then(|environment| environment.ready_client())
    else {
        return Ok(Cycle::Behind(
            "Waiting for the cloud computer to connect.".into(),
        ));
    };
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
    let objects = objects_dir(&profile, key);
    let _ = fs::remove_dir_all(&objects);
    fs::create_dir_all(&objects)?;
    let have = known
        .as_ref()
        .map(|known| known.keys().take(90_000).cloned().collect())
        .unwrap_or_default();
    let files = mirror::receive_snapshot(
        &client,
        WorkspaceTarget {
            chat_id: Some(state.chat_id.clone()),
            space_id: None,
            checkout_path: None,
        },
        &state.workspace_id,
        &objects,
        have,
        cancel.clone(),
    )
    .await?;
    ensure!(!cancel.is_cancelled(), "Sync stopped.");
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
            // First checkpoint: save the checkout as it is right now, persist
            // that, then move it.
            let dir = path.clone();
            let saved = tokio::task::spawn_blocking(move || repo::save(&dir)).await??;
            let mut next = current.clone();
            next.branch = saved.branch.clone();
            next.head = Some(saved.head.clone());
            next.changes = saved.changes.clone();
            api.checkouts.save_state(key, &next)?;
            let dir = path.clone();
            let to = commit.clone();
            let entered = {
                let saved = saved.clone();
                tokio::task::spawn_blocking(move || repo::enter(&dir, &saved, &to)).await?
            };
            if let Err(error) = entered {
                // Nothing moved; the saved changes are back in the folder.
                let mut back = current.clone();
                back.branch = None;
                back.head = None;
                back.changes = None;
                api.checkouts.save_state(key, &back)?;
                return Err(error);
            }
            next.checkpoint = Some(commit);
            api.checkouts.save_state(key, &next)?;
            api.checkouts.report(next.chat());
        }
        Some(from) => {
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
            api.checkouts.save_state(key, &next)?;
        }
    }
    Ok(Cycle::Changed(count))
}

/// The connected repository a cloud workspace was started from, as
/// `host/owner/repo` (lowercase).
async fn repository(api: &CloudApi, auth: &Auth, account: &str, row: &Value) -> Result<String> {
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
    Ok(normalize(identity))
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
    if method == methods::CLOUD_CHECKOUT_SYNC_RESUME {
        let key = api
            .checkouts
            .by_chat(&account, &workspace, &chat)
            .context("This chat isn't synced to a folder here.")?;
        let state = api.checkouts.state(&key).context("Sync stopped.")?;
        if state.phase == Phase::Restoring {
            api.checkouts.restore(&key, None).await?;
            return Ok(reply(api.checkouts.status(&account, &workspace, &chat)));
        }
        if let Some(checkpoint) = state.checkpoint.clone() {
            let dir = state.path.clone();
            let kept = tokio::task::spawn_blocking(move || {
                let kept = repo::keep_in_stash_list(
                    &dir,
                    "Zuse: edits made while synced to a cloud chat",
                )?;
                repo::return_to(&dir, &checkpoint)?;
                Ok::<_, anyhow::Error>(kept)
            })
            .await??;
            if let Some(record) = lock(&api.checkouts.records).get_mut(&key) {
                record.status.state = "syncing";
                record.status.error = None;
                record.status.notice = kept
                    .then(|| "Your edits are in the folder's stash list (git stash list).".into());
                record.wake.notify_one();
            }
        }
        Manager::start(&api, &auth, &key);
        return Ok(reply(api.checkouts.status(&account, &workspace, &chat)));
    }
    ensure!(
        method == methods::CLOUD_CHECKOUT_SYNC_CONFIGURE,
        "Unknown sync request."
    );
    let enabled = params["enabled"]
        .as_bool()
        .context("Choose whether to sync this chat.")?;
    let existing = api.checkouts.by_chat(&account, &workspace, &chat);
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
    let requested = params["localPath"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 4096)
        .map(PathBuf::from)
        .context("Choose this repository's folder on this computer.")?;
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
    ensure!(
        !row.archived() && !row.deleting() && !matches!(row.state.as_str(), "deleted" | "deleting"),
        "Unarchive this chat before syncing it."
    );
    ensure!(
        row.rpc_protocol.as_deref() == Some("zuse-rpc-v1"),
        "Update this cloud computer before syncing its files."
    );
    let expected = repository(&api, &auth, &account, &value).await?;
    let checked = requested.clone();
    let path = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        let root = repo::checkout_root(&checked)?;
        let remote = repo::remote(&root).map(|r| normalize(&r));
        ensure!(
            remote.as_deref() == Some(expected.as_str()),
            "{} is a checkout of {}, not {expected}.",
            root.display(),
            remote.as_deref().unwrap_or("another repository")
        );
        Ok(root)
    })
    .await??;
    if let Some(key) = &existing {
        if key == &path {
            Manager::start(&api, &auth, key);
            return Ok(reply(api.checkouts.status(&account, &workspace, &chat)));
        }
        api.checkouts.end(key, None).await?;
    }
    // Switching: the chat synced here before goes back first.
    let occupied = lock(&api.checkouts.records).contains_key(&path);
    if occupied {
        api.checkouts.end(&path, None).await?;
    }
    lock(&api.checkouts.notices).remove(&(account.clone(), workspace.clone(), chat.clone()));
    let state = State {
        version: 1,
        account_id: account.clone(),
        workspace_id: workspace.clone(),
        chat_id: chat.clone(),
        path: path.clone(),
        phase: Phase::On,
        branch: None,
        head: None,
        changes: None,
        checkpoint: None,
    };
    let file = state_file(&profile, &path);
    write_state(&file, &state)?;
    lock(&api.checkouts.records).insert(
        path.clone(),
        Record {
            status: Status::on(&state),
            state,
            file,
            cancel: CancellationToken::new(),
            wake: Arc::new(Notify::new()),
            task: None,
        },
    );
    Manager::start(&api, &auth, &path);
    Ok(reply(api.checkouts.status(&account, &workspace, &chat)))
}
