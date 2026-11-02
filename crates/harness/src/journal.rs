//! Sequenced durable transitions with bounded snapshot rotation. History cursors
//! encode append operations without diffing or serializing prior transcripts.
use crate::artifact::{ArchivedValue, Artifacts};
use crate::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
const SNAPSHOT_BYTES: u64 = 4 * 1024 * 1024;
const SNAPSHOT_COMMITS: u64 = 1000;
const MAX_RECORD: usize = 128 * 1024 * 1024;
const HEADER: usize = 4 + 8 + 32;
const FOOTER: usize = 16;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentHead {
    id: String,
    name: String,
    parent_id: Option<String>,
    parent_item_id: Option<String>,
    depth: u8,
    read_only: bool,
    mailbox: Vec<Mail>,
    status: Status,
    turns: u64,
    summary: String,
    pending_calls: Vec<Call>,
    completed_delivery: bool,
    checkpoint: Option<Compaction>,
}
impl From<&Agent> for AgentHead {
    fn from(a: &Agent) -> Self {
        Self {
            id: a.id.clone(),
            name: a.name.clone(),
            parent_id: a.parent_id.clone(),
            parent_item_id: a.parent_item_id.clone(),
            depth: a.depth,
            read_only: a.read_only,
            mailbox: a.mailbox.clone(),
            status: a.status,
            turns: a.turns,
            summary: a.summary.clone(),
            pending_calls: a.pending_calls.clone(),
            completed_delivery: a.completed_delivery,
            checkpoint: a.checkpoint.clone(),
        }
    }
}
impl AgentHead {
    fn apply(self, history: Vec<Message>) -> Agent {
        Agent {
            id: self.id,
            name: self.name,
            parent_id: self.parent_id,
            parent_item_id: self.parent_item_id,
            depth: self.depth,
            read_only: self.read_only,
            mailbox: self.mailbox,
            status: self.status,
            turns: self.turns,
            summary: self.summary,
            pending_calls: self.pending_calls,
            completed_delivery: self.completed_delivery,
            checkpoint: self.checkpoint,
            history,
        }
    }
}
#[derive(Serialize, Deserialize)]
struct AgentChange {
    head: AgentHead,
    from: usize,
    append: Vec<Message>,
}
#[derive(Default, Serialize, Deserialize)]
struct SetChange {
    add: Vec<String>,
    remove: Vec<String>,
}
impl SetChange {
    fn between(old: &BTreeSet<String>, new: &BTreeSet<String>) -> Self {
        Self {
            add: new.difference(old).cloned().collect(),
            remove: old.difference(new).cloned().collect(),
        }
    }
    fn apply(self, set: &mut BTreeSet<String>) {
        for id in self.remove {
            set.remove(&id);
        }
        set.extend(self.add);
    }
}
#[derive(Serialize, Deserialize)]
struct Commit {
    model: String,
    reasoning: Option<String>,
    requests: u64,
    agents: Vec<AgentChange>,
    receipts: Vec<(String, Option<Receipt>)>,
    notifications: SetChange,
    cancelled: SetChange,
    consumed: Vec<(String, Option<String>)>,
    reviews: Vec<RecoveryReview>,
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    format: u8,
    sequence: u64,
    state: ArchivedValue,
}
#[derive(Clone)]
struct AgentCursor {
    length: usize,
    head: [u8; 32],
}
#[derive(Clone)]
struct ReceiptCursor {
    started: bool,
    signature: Option<String>,
    output: Option<Shared<ToolOutput>>,
}
#[derive(Default, Clone)]
struct Cursor {
    root_id: String,
    version: u8,
    model: String,
    reasoning: Option<String>,
    requests: u64,
    sequence: u64,
    bytes: u64,
    commits: u64,
    agents: BTreeMap<String, AgentCursor>,
    receipts: BTreeMap<String, ReceiptCursor>,
    notifications: BTreeSet<String>,
    cancelled: BTreeSet<String>,
    consumed: BTreeMap<String, String>,
    reviews: usize,
}
impl Cursor {
    fn state(state: &Checkpoint, sequence: u64, bytes: u64, commits: u64) -> Result<Self, Error> {
        Ok(Self {
            root_id: state.root_id.clone(),
            version: state.version,
            model: state.model.clone(),
            reasoning: state.reasoning.clone(),
            requests: state.requests,
            sequence,
            bytes,
            commits,
            agents: state
                .agents
                .iter()
                .map(|a| {
                    Ok((
                        a.id.clone(),
                        AgentCursor {
                            length: a.history.len(),
                            head: head_fingerprint(a)?,
                        },
                    ))
                })
                .collect::<Result<_, Error>>()?,
            receipts: state
                .tools
                .iter()
                .map(|(id, r)| {
                    (
                        id.clone(),
                        ReceiptCursor {
                            started: matches!(r.status, ReceiptStatus::Started),
                            signature: r.signature.clone(),
                            output: r.output.clone(),
                        },
                    )
                })
                .collect(),
            notifications: state.notifications.clone(),
            cancelled: state.cancelled_notifications.clone(),
            consumed: state.consumed_notifications.clone(),
            reviews: state.reviews.len(),
        })
    }
}
/// Prepared while holding the runtime's state lock; contains only changed data,
/// except at snapshot boundaries. File IO runs on the shared blocking pool.
pub struct Prepared {
    sequence: u64,
    bytes: Vec<u8>,
    snapshot: Option<Checkpoint>,
    next: Cursor,
}
#[cfg(test)]
type FaultHook = std::sync::Arc<dyn Fn(u8) + Send + Sync>;

pub struct JournalStore {
    directory: PathBuf,
    owner: Option<String>,
    cursor: Mutex<Option<Cursor>>,
    poisoned: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    fault: Mutex<Option<FaultHook>>,
}
impl JournalStore {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            owner: None,
            cursor: Mutex::new(None),
            poisoned: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fault: Mutex::new(None),
        }
    }
    pub fn for_root(mut self, id: &str) -> Self {
        self.owner = Some(id.to_owned());
        self
    }
    fn validate_owner(&self, state: &Checkpoint) -> Result<(), Error> {
        if self
            .owner
            .as_ref()
            .is_some_and(|owner| owner != &state.root_id)
        {
            Err(Error::InvalidCheckpoint)
        } else {
            Ok(())
        }
    }
    fn fence_legacy(
        &self,
        root_id: &str,
        expected_legacy: Option<&Checkpoint>,
    ) -> Result<(), Error> {
        let path = self.directory.join("checkpoint.json");
        let backup = self.directory.join("checkpoint.v1.backup.json");
        match std::fs::File::open(&path) {
            Ok(file) => {
                let marker: serde_json::Value =
                    serde_json::from_reader(std::io::BufReader::new(file))
                        .map_err(|_| Error::InvalidCheckpoint)?;
                if marker["format"] == "zuse-transitions-v2" && marker["rootId"] == root_id {
                    return Ok(());
                }
                let expected = expected_legacy.ok_or(Error::InvalidCheckpoint)?;
                let legacy: Checkpoint =
                    serde_json::from_value(marker).map_err(|_| Error::InvalidCheckpoint)?;
                legacy.validate()?;
                if legacy.root_id != root_id
                    || serde_json::to_value(&legacy).map_err(|_| Error::Storage)?
                        != serde_json::to_value(expected).map_err(|_| Error::Storage)?
                {
                    return Err(Error::InvalidCheckpoint);
                }
                if !backup.exists() {
                    if std::fs::hard_link(&path, &backup).is_err() {
                        std::fs::copy(&path, &backup).map_err(|_| Error::Storage)?;
                        std::fs::File::open(&backup)
                            .and_then(|f| f.sync_all())
                            .map_err(|_| Error::Storage)?;
                    }
                    sync_directory(&self.directory).map_err(|_| Error::Storage)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Storage),
        }
        atomic_json(
            &path,
            &serde_json::json!({"version":2,"rootId":root_id,"format":"zuse-transitions-v2"}),
        )
    }
    fn snapshot_path(&self) -> PathBuf {
        self.directory.join("snapshot.json")
    }
    fn log_path(&self) -> PathBuf {
        self.directory.join("transitions.log")
    }
    pub fn load(&self) -> Result<Option<Checkpoint>, Error> {
        self.load_impl(true)
    }
    /// Observational reads never repair a tail belonging to an active writer.
    pub fn inspect(directory: &Path) -> Result<Option<Checkpoint>, Error> {
        let store = Self::new(directory);
        for _ in 0..8 {
            let before = std::fs::read(store.snapshot_path()).ok();
            let result = store.load_impl(false);
            let after = std::fs::read(store.snapshot_path()).ok();
            if before == after {
                return result;
            }
        }
        Err(Error::Storage)
    }
    fn load_impl(&self, repair: bool) -> Result<Option<Checkpoint>, Error> {
        let mut cursor = self.cursor.lock().map_err(|_| Error::Storage)?;
        let snapshot = match std::fs::read(self.snapshot_path()) {
            Ok(bytes) => {
                let s: Snapshot =
                    serde_json::from_slice(&bytes).map_err(|_| Error::InvalidCheckpoint)?;
                if s.format != 2 {
                    return Err(Error::InvalidCheckpoint);
                }
                Some(s)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(Error::Storage),
        };
        let artifacts = Artifacts::new(&self.directory);
        let Some(snapshot) = snapshot else {
            // Keep legacy storage untouched until an atomic new snapshot exists.
            let state = store::FileStore::new(self.directory.join("checkpoint.json")).load()?;
            if self.log_path().exists() {
                return Err(Error::InvalidCheckpoint);
            }
            if let Some(state) = &state {
                self.validate_owner(state)?;
            }
            if repair && let Some(state) = &state {
                atomic_json(
                    &self.snapshot_path(),
                    &Snapshot {
                        format: 2,
                        sequence: 0,
                        state: artifacts
                            .archive(serde_json::to_value(state).map_err(|_| Error::Storage)?)?,
                    },
                )?;
                self.fence_legacy(&state.root_id, Some(state))?;
                *cursor = Some(Cursor::state(state, 0, 0, 0)?);
            }
            return Ok(state);
        };
        let mut state: Checkpoint = serde_json::from_value(artifacts.restore(snapshot.state)?)
            .map_err(|_| Error::InvalidCheckpoint)?;
        let mut sequence = snapshot.sequence;
        state.validate()?;
        self.validate_owner(&state)?;
        let mut bytes = 0;
        let mut commits = 0;
        let mut last = None;
        match std::fs::File::open(self.log_path()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::Storage),
            Ok(mut file) => loop {
                let start = bytes;
                let mut header = [0; HEADER];
                let read = read_partial(&mut file, &mut header)?;
                if read == 0 {
                    break;
                }
                if read < HEADER {
                    if repair {
                        self.truncate_tail(start)?;
                    }
                    break;
                }
                let length = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
                let seq = u64::from_le_bytes(header[4..12].try_into().unwrap());
                if length > MAX_RECORD || length == 0 {
                    return Err(Error::InvalidCheckpoint);
                }
                let mut payload = vec![0; length];
                if read_partial(&mut file, &mut payload)? < length {
                    if has_commit_marker(&self.log_path(), start)? {
                        return Err(Error::InvalidCheckpoint);
                    }
                    if repair {
                        self.truncate_tail(start)?;
                    }
                    break;
                }
                let mut footer = [0; FOOTER];
                if read_partial(&mut file, &mut footer)? < FOOTER {
                    if has_commit_marker(&self.log_path(), start)? {
                        return Err(Error::InvalidCheckpoint);
                    }
                    if repair {
                        self.truncate_tail(start)?;
                    }
                    break;
                }
                if footer != commit_marker(seq, length) {
                    return Err(Error::InvalidCheckpoint);
                }
                if digest(seq, &payload).as_slice() != &header[12..] {
                    return Err(Error::InvalidCheckpoint);
                }
                if last.is_some_and(|previous: u64| previous.checked_add(1) != Some(seq)) {
                    return Err(Error::InvalidCheckpoint);
                }
                last = Some(seq);
                bytes += HEADER as u64 + length as u64 + FOOTER as u64;
                if seq <= sequence {
                    continue;
                }
                if seq != sequence + 1 {
                    return Err(Error::InvalidCheckpoint);
                }
                let archived: ArchivedValue =
                    serde_json::from_slice(&payload).map_err(|_| Error::InvalidCheckpoint)?;
                let commit: Commit = serde_json::from_value(artifacts.restore(archived)?)
                    .map_err(|_| Error::InvalidCheckpoint)?;
                commit.apply(&mut state)?;
                sequence = seq;
                commits += 1;
            },
        }
        state.validate()?;
        self.validate_owner(&state)?;
        if repair {
            self.fence_legacy(
                &state.root_id,
                if sequence == 0 { Some(&state) } else { None },
            )?;
            let backup = self.directory.join("checkpoint.v1.backup.json");
            if backup.exists() {
                std::fs::remove_file(backup).map_err(|_| Error::Storage)?;
                sync_directory(&self.directory).map_err(|_| Error::Storage)?;
            }
        }
        *cursor = Some(Cursor::state(&state, sequence, bytes, commits)?);
        Ok(Some(state))
    }
    fn truncate_tail(&self, length: u64) -> Result<(), Error> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(self.log_path())
            .map_err(|_| Error::Storage)?;
        file.set_len(length)
            .and_then(|_| file.sync_all())
            .map_err(|_| Error::Storage)
    }
    pub fn prepare(&self, state: &Checkpoint) -> Result<Option<Prepared>, Error> {
        state.validate()?;
        self.validate_owner(state)?;
        if self.poisoned.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::Storage);
        }
        let cursor = self.cursor.lock().map_err(|_| Error::Storage)?;
        let previous = cursor.as_ref();
        if previous.is_none() && (self.snapshot_path().exists() || self.log_path().exists()) {
            return Err(Error::Storage);
        }
        if previous.is_none() {
            return Ok(Some(Prepared {
                sequence: 0,
                bytes: vec![],
                snapshot: Some(state.clone()),
                next: Cursor::state(state, 0, 0, 0)?,
            }));
        }
        let previous = previous.unwrap();
        if previous.root_id != state.root_id
            || previous.version != state.version
            || previous
                .agents
                .keys()
                .any(|id| !state.agents.iter().any(|a| &a.id == id))
        {
            return Err(Error::InvalidCheckpoint);
        }
        if state.agents.len() < previous.agents.len() || state.reviews.len() < previous.reviews {
            return Err(Error::InvalidCheckpoint);
        }
        let mut agents = Vec::new();
        for agent in &state.agents {
            let old = previous.agents.get(&agent.id);
            let from = old.map_or(0, |a| a.length);
            if from > agent.history.len() {
                return Err(Error::InvalidCheckpoint);
            }
            let encoded = head_fingerprint(agent)?;
            if old.is_none_or(|old| old.head != encoded || from != agent.history.len()) {
                agents.push(AgentChange {
                    head: AgentHead::from(agent),
                    from,
                    append: agent.history[from..].to_vec(),
                });
            }
        }
        let mut receipts = Vec::new();
        for (id, r) in &state.tools {
            let started = matches!(r.status, ReceiptStatus::Started);
            if previous.receipts.get(id).is_none_or(|old| {
                old.started != started
                    || old.signature != r.signature
                    || match (&old.output, &r.output) {
                        (Some(a), Some(b)) => !a.ptr_eq(b),
                        (None, None) => false,
                        _ => true,
                    }
            }) {
                receipts.push((id.clone(), Some(r.clone())));
            }
        }
        receipts.extend(
            previous
                .receipts
                .keys()
                .filter(|id| !state.tools.contains_key(*id))
                .map(|id| (id.clone(), None)),
        );
        let mut consumed = state
            .consumed_notifications
            .iter()
            .filter(|(id, value)| previous.consumed.get(*id) != Some(*value))
            .map(|(id, v)| (id.clone(), Some(v.clone())))
            .collect::<Vec<_>>();
        consumed.extend(
            previous
                .consumed
                .keys()
                .filter(|id| !state.consumed_notifications.contains_key(*id))
                .map(|id| (id.clone(), None)),
        );
        let commit = Commit {
            model: state.model.clone(),
            reasoning: state.reasoning.clone(),
            requests: state.requests,
            agents,
            receipts,
            consumed,
            notifications: SetChange::between(&previous.notifications, &state.notifications),
            cancelled: SetChange::between(&previous.cancelled, &state.cancelled_notifications),
            reviews: state.reviews[previous.reviews..].to_vec(),
        };
        if commit.model == previous.model
            && commit.reasoning == previous.reasoning
            && commit.requests == previous.requests
            && commit.agents.is_empty()
            && commit.receipts.is_empty()
            && commit.consumed.is_empty()
            && commit.reviews.is_empty()
            && commit.notifications.add.is_empty()
            && commit.notifications.remove.is_empty()
            && commit.cancelled.add.is_empty()
            && commit.cancelled.remove.is_empty()
        {
            return Ok(None);
        }
        let bytes = serde_json::to_vec(&commit).map_err(|_| Error::Storage)?;
        if bytes.len() > MAX_RECORD {
            return Err(Error::Storage);
        }
        let sequence = previous.sequence.checked_add(1).ok_or(Error::Storage)?;
        let next_bytes = previous.bytes + HEADER as u64 + bytes.len() as u64 + FOOTER as u64 + 64;
        let next_commits = previous.commits + 1;
        let snapshot = if next_bytes >= SNAPSHOT_BYTES || next_commits >= SNAPSHOT_COMMITS {
            Some(state.clone())
        } else {
            None
        };
        Ok(Some(Prepared {
            sequence,
            bytes,
            snapshot,
            next: Cursor::state(state, sequence, next_bytes, next_commits)?,
        }))
    }
    pub fn commit(&self, mut prepared: Prepared) -> Result<(), Error> {
        let mut cursor = self.cursor.lock().map_err(|_| Error::Storage)?;
        if self.poisoned.load(std::sync::atomic::Ordering::Acquire) {
            return Err(Error::Storage);
        }
        if cursor
            .as_ref()
            .map_or(Some(0), |c| c.sequence.checked_add(1))
            != Some(prepared.sequence)
        {
            return Err(Error::Storage);
        }
        let artifacts = Artifacts::new(&self.directory);
        if !prepared.bytes.is_empty() {
            let value = serde_json::from_slice(&prepared.bytes).map_err(|_| Error::Storage)?;
            let archived = artifacts.archive(value)?;
            let bytes = serde_json::to_vec(&archived).map_err(|_| Error::Storage)?;
            prepared.next.bytes = cursor.as_ref().map_or(0, |c| c.bytes)
                + HEADER as u64
                + bytes.len() as u64
                + FOOTER as u64;
            prepared.bytes = bytes;
            if prepared.next.bytes < SNAPSHOT_BYTES && prepared.next.commits < SNAPSHOT_COMMITS {
                prepared.snapshot = None;
            }
            std::fs::create_dir_all(&self.directory).map_err(|_| Error::Storage)?;
            let mut options = std::fs::OpenOptions::new();
            options.append(true).create(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(self.log_path()).map_err(|_| Error::Storage)?;
            let start = file.metadata().map_err(|_| Error::Storage)?.len();
            let result = (|| {
                file.write_all(&(prepared.bytes.len() as u32).to_le_bytes())?;
                #[cfg(test)]
                self.point(1);
                file.write_all(&prepared.sequence.to_le_bytes())?;
                file.write_all(&digest(prepared.sequence, &prepared.bytes))?;
                file.write_all(&prepared.bytes)?;
                #[cfg(test)]
                self.point(2);
                file.write_all(&commit_marker(prepared.sequence, prepared.bytes.len()))?;
                file.sync_all()?;
                #[cfg(test)]
                self.point(3);
                sync_directory(&self.directory)
            })();
            if result.is_err() {
                if file.set_len(start).and_then(|_| file.sync_all()).is_err() {
                    self.poisoned
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                return Err(Error::Storage);
            }
        }
        // A committed frame is authoritative even if optional snapshot rotation
        // fails. Never report that transition as uncommitted after its fsync.
        if let Some(state) = prepared.snapshot {
            let archived = serde_json::to_value(state)
                .map_err(|_| Error::Storage)
                .and_then(|state| artifacts.archive(state));
            let snapshot = archived.map(|state| Snapshot {
                format: 2,
                sequence: prepared.sequence,
                state,
            });
            if let Some(snapshot) = snapshot
                .ok()
                .filter(|s| atomic_json(&self.snapshot_path(), s).is_ok())
            {
                if cursor.is_none() {
                    self.fence_legacy(&prepared.next.root_id, None)?;
                }
                #[cfg(test)]
                self.point(4);
                if self.log_path().exists() {
                    let _ = std::fs::OpenOptions::new()
                        .write(true)
                        .open(self.log_path())
                        .and_then(|f| f.set_len(0).and_then(|_| f.sync_all()));
                }
                #[cfg(test)]
                self.point(5);
                prepared.next.bytes = std::fs::metadata(self.log_path()).map_or(0, |m| m.len());
                prepared.next.commits = 0;
                let _ = artifacts.reclaim(&snapshot.state);
            } else if cursor.is_none() {
                return Err(Error::Storage);
            }
        }
        *cursor = Some(prepared.next);
        Ok(())
    }
    #[cfg(test)]
    fn point(&self, point: u8) {
        if let Some(fault) = self.fault.lock().unwrap().as_ref() {
            fault(point);
        }
    }
    pub fn save(&self, state: &Checkpoint) -> Result<(), Error> {
        if let Some(prepared) = self.prepare(state)? {
            self.commit(prepared)
        } else {
            Ok(())
        }
    }
}
impl Commit {
    fn apply(self, state: &mut Checkpoint) -> Result<(), Error> {
        state.model = self.model;
        state.reasoning = self.reasoning;
        state.requests = self.requests;
        for change in self.agents {
            if let Some(index) = state.agents.iter().position(|a| a.id == change.head.id) {
                if state.agents[index].history.len() != change.from {
                    return Err(Error::InvalidCheckpoint);
                }
                let mut history = std::mem::take(&mut state.agents[index].history);
                history.extend(change.append);
                state.agents[index] = change.head.apply(history);
            } else {
                if change.from != 0 {
                    return Err(Error::InvalidCheckpoint);
                }
                state.agents.push(change.head.apply(change.append));
            }
        }
        for (id, receipt) in self.receipts {
            if let Some(receipt) = receipt {
                state.tools.insert(id, receipt);
            } else {
                state.tools.remove(&id);
            }
        }
        self.notifications.apply(&mut state.notifications);
        self.cancelled.apply(&mut state.cancelled_notifications);
        for (id, value) in self.consumed {
            if let Some(value) = value {
                state.consumed_notifications.insert(id, value);
            } else {
                state.consumed_notifications.remove(&id);
            }
        }
        state.reviews.extend(self.reviews);
        Ok(())
    }
}
fn digest(sequence: u64, payload: &[u8]) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(sequence.to_le_bytes());
    hash.update(payload);
    hash.finalize().into()
}
fn read_partial(file: &mut std::fs::File, buffer: &mut [u8]) -> Result<usize, Error> {
    let mut read = 0;
    while read < buffer.len() {
        match file.read(&mut buffer[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Error::Storage),
        }
    }
    Ok(read)
}
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(directory)?.sync_all()?;
    }
    Ok(())
}
pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<(), Error> {
    let directory = path.parent().ok_or(Error::Storage)?;
    std::fs::create_dir_all(directory).map_err(|_| Error::Storage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| Error::Storage)?;
    }
    let temp = directory.join(format!(".snapshot-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = std::io::BufWriter::new(options.open(&temp)?);
        serde_json::to_writer(&mut file, value)?;
        file.flush()?;
        file.get_ref().sync_all()?;
        std::fs::rename(&temp, path)?;
        sync_directory(directory)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result.map_err(|_: std::io::Error| Error::Storage)
}

fn commit_marker(sequence: u64, length: usize) -> [u8; FOOTER] {
    let mut marker = [0; FOOTER];
    marker[..4].copy_from_slice(b"ZCMT");
    marker[4..8].copy_from_slice(&(length as u32).to_le_bytes());
    marker[8..].copy_from_slice(&sequence.to_le_bytes());
    marker
}
// A damaged length must not turn a complete committed frame into a torn tail.
fn has_commit_marker(path: &Path, start: u64) -> Result<bool, Error> {
    use std::io::Seek;
    let mut file = std::fs::File::open(path).map_err(|_| Error::Storage)?;
    file.seek(std::io::SeekFrom::Start(start + HEADER as u64))
        .map_err(|_| Error::Storage)?;
    let mut remaining = Vec::new();
    file.take((MAX_RECORD + FOOTER) as u64)
        .read_to_end(&mut remaining)
        .map_err(|_| Error::Storage)?;
    Ok(remaining
        .windows(FOOTER)
        .enumerate()
        .any(|(index, footer)| {
            footer[..4] == *b"ZCMT"
                && u32::from_le_bytes(footer[4..8].try_into().unwrap()) as usize == index
        }))
}
fn head_fingerprint(agent: &Agent) -> Result<[u8; 32], Error> {
    struct Writer(Sha256);
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let Agent {
        id,
        name,
        parent_id,
        parent_item_id,
        depth,
        read_only,
        history: _,
        mailbox,
        status,
        turns,
        summary,
        pending_calls,
        completed_delivery,
        checkpoint,
    } = agent;
    let mut writer = Writer(Sha256::new());
    serde_json::to_writer(
        &mut writer,
        &(
            id,
            name,
            parent_id,
            parent_item_id,
            depth,
            read_only,
            mailbox,
            status,
            turns,
            summary,
            pending_calls,
            completed_delivery,
            checkpoint,
        ),
    )
    .map_err(|_| Error::Storage)?;
    Ok(writer.0.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn initial() -> Checkpoint {
        serde_json::from_value(serde_json::json!({"version":1,"rootId":"root","model":"fixture","requests":0,"tools":{},"agents":[{"id":"root","name":"root","parentId":null,"parentItemId":null,"depth":0,"readOnly":false,"history":[{"role":"user","content":"original task"}],"mailbox":[],"status":"idle","turns":0,"summary":"","pendingCalls":[],"completedDelivery":false}]})).unwrap()
    }
    #[test]
    fn crash_probe() {
        let Some(path) = std::env::var_os("ZUSE_JOURNAL_CRASH_DIRECTORY") else {
            return;
        };
        let point = std::env::var("ZUSE_JOURNAL_CRASH_POINT")
            .unwrap()
            .parse::<u8>()
            .unwrap();
        let store = JournalStore::new(path);
        let mut state = store.load().unwrap().unwrap();
        state.requests += 1;
        state.agents[0]
            .history
            .push(Message::text(Role::Assistant, "payload".repeat(800000)));
        *store.fault.lock().unwrap() = Some(std::sync::Arc::new(move |stage| {
            if stage == point {
                std::process::exit(81);
            }
        }));
        store.save(&state).unwrap();
        panic!("crash point was not reached");
    }
    #[test]
    fn abrupt_process_exit_preserves_only_complete_commits_at_every_boundary() {
        for point in 1..=5 {
            let directory = tempfile::tempdir().unwrap();
            let store = JournalStore::new(directory.path());
            let mut initial = initial();
            store.save(&initial).unwrap();
            if point >= 4 {
                for _ in 0..999 {
                    initial.requests += 1;
                    store.save(&initial).unwrap();
                }
            }
            drop(store);
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "journal::tests::crash_probe", "--nocapture"])
                .env("ZUSE_JOURNAL_CRASH_DIRECTORY", directory.path())
                .env("ZUSE_JOURNAL_CRASH_POINT", point.to_string())
                .output()
                .unwrap();
            assert_eq!(
                result.status.code(),
                Some(81),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            let recovered = JournalStore::new(directory.path()).load().unwrap().unwrap();
            assert_eq!(
                recovered.requests,
                if point <= 2 {
                    0
                } else if point >= 4 {
                    1000
                } else {
                    1
                }
            );
            assert_eq!(
                recovered.agents[0].history.len(),
                if point <= 2 { 1 } else { 2 }
            );
        }
    }
    #[test]
    fn corrupt_record_length_cannot_discard_a_committed_tail() {
        let directory = tempfile::tempdir().unwrap();
        let store = JournalStore::new(directory.path());
        let mut state = initial();
        store.save(&state).unwrap();
        state.requests = 1;
        store.save(&state).unwrap();
        drop(store);
        let path = directory.path().join("transitions.log");
        let mut bytes = std::fs::read(&path).unwrap();
        let length = u32::from_le_bytes(bytes[..4].try_into().unwrap());
        bytes[..4].copy_from_slice(&(length + 1024).to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            JournalStore::new(directory.path()).load(),
            Err(Error::InvalidCheckpoint)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
