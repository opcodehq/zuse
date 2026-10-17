//! Supervised commands, bounded artifacts and a durable notification outbox.
pub mod supervisor;
use crate::{Error, ToolContext};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    process::{Child, ChildStdin, Command},
    sync::{Mutex as AsyncMutex, Semaphore, watch},
};
use tokio_util::{
    codec::{FramedRead, LinesCodec},
    sync::CancellationToken,
};

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn failed(message: &str) -> Error {
    Error::Tool(message.into())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Running,
    Exited,
    Cancelled,
    TimedOut,
    OutputLimit,
    Interrupted,
    Failed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRecord {
    pub id: String,
    pub agent_id: String,
    pub label: String,
    pub cwd: PathBuf,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub bytes: u64,
    pub ready: bool,
    pub ready_notified: bool,
    pub completion_notified: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessNotice {
    pub id: String,
    pub process_id: String,
    pub agent_id: String,
    pub kind: String,
    pub status: ProcessStatus,
    pub exit_code: Option<i32>,
    pub label: String,
    pub artifact: PathBuf,
}
#[async_trait]
pub trait ProcessNotifier: Send + Sync {
    async fn notify(&self, notice: ProcessNotice) -> Result<(), Error>;
}
pub struct ProcessOptions {
    pub root_id: String,
    pub directory: PathBuf,
    pub command_host: PathBuf,
    pub notifier: Arc<dyn ProcessNotifier>,
    pub env: Option<BTreeMap<String, String>>,
    pub max_running: usize,
    pub max_retained: usize,
    pub max_output_bytes: u64,
}
impl ProcessOptions {
    pub fn new(
        root_id: String,
        directory: PathBuf,
        notifier: Arc<dyn ProcessNotifier>,
    ) -> Result<Self, Error> {
        Ok(Self {
            root_id,
            directory,
            command_host: std::env::current_exe().map_err(|_| Error::Storage)?,
            notifier,
            env: None,
            max_running: 8,
            max_retained: 128,
            max_output_bytes: 64 * 1024 * 1024,
        })
    }
}
pub struct ProcessStart {
    pub file: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub label: String,
    pub timeout_ms: u64,
    pub notify_on_output: Option<String>,
    pub notify: bool,
}
#[derive(Debug, Serialize)]
pub struct ProcessOutput {
    pub process: ProcessRecord,
    pub text: String,
    pub next_cursor: u64,
    pub artifact: PathBuf,
    pub storage_error: bool,
}
struct Entry {
    record: Mutex<ProcessRecord>,
    input: AsyncMutex<Option<ChildStdin>>,
    stop: CancellationToken,
    done: watch::Receiver<bool>,
    initialized: watch::Receiver<bool>,
    persistence: AsyncMutex<()>,
    delivery: AsyncMutex<()>,
    storage_error: AtomicBool,
    // 0: tool owns cancellation; 1: handed to session; 2: cancellation won.
    ownership: AtomicU8,
}
struct Inner {
    options: Arc<ProcessOptions>,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    capacity: Arc<Semaphore>,
    lifecycle: Arc<tokio::sync::RwLock<()>>,
    closed: AtomicBool,
    shutdown: CancellationToken,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}
#[derive(Clone)]
pub struct Processes(Arc<Inner>);
struct Done(watch::Sender<bool>);
impl Drop for Done {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
struct Group(Option<u32>);
impl Group {
    fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0.take() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}
fn artifact(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.log"))
}
async fn persist(entry: &Entry, directory: &Path) -> Result<(), Error> {
    let _gate = entry.persistence.lock().await;
    let record = lock(&entry.record).clone();
    let directory = directory.to_path_buf();
    // Small metadata commits finish in one poll so cancellation cannot leave
    // a detached filesystem writer running after root ownership is released.
    (|| {
        use std::io::Write;
        let path = directory.join(format!("{}.json", record.id));
        let temp = directory.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp).map_err(|_| Error::Storage)?;
            serde_json::to_writer(&mut file, &record).map_err(|_| Error::Storage)?;
            file.flush()
                .and_then(|_| file.sync_all())
                .map_err(|_| Error::Storage)?;
            std::fs::rename(&temp, &path).map_err(|_| Error::Storage)?;
            #[cfg(unix)]
            std::fs::File::open(&directory)
                .and_then(|f| f.sync_all())
                .map_err(|_| Error::Storage)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temp);
        }
        result
    })()
}
async fn deliver(entry: &Entry, options: &ProcessOptions) {
    let _gate = entry.delivery.lock().await;
    for ready in [true, false] {
        let record = lock(&entry.record).clone();
        if if ready {
            !record.ready || record.ready_notified
        } else {
            record.status == ProcessStatus::Running || record.completion_notified
        } {
            continue;
        }
        let kind = if ready { "ready" } else { "completed" };
        let notice = ProcessNotice {
            id: format!("{}:{kind}", record.id),
            process_id: record.id.clone(),
            agent_id: record.agent_id,
            kind: kind.into(),
            status: record.status,
            exit_code: record.exit_code,
            label: record.label,
            artifact: artifact(&options.directory, &record.id),
        };
        if !matches!(
            tokio::time::timeout(Duration::from_secs(5), options.notifier.notify(notice)).await,
            Ok(Ok(()))
        ) {
            continue;
        }
        {
            let mut record = lock(&entry.record);
            if ready {
                record.ready_notified = true;
            } else {
                record.completion_notified = true;
            }
        }
        if persist(entry, &options.directory).await.is_err() {
            let mut record = lock(&entry.record);
            if ready {
                record.ready_notified = false;
            } else {
                record.completion_notified = false;
            }
            entry.storage_error.store(true, Ordering::Release);
        }
    }
}
fn mark_stop(entry: &Entry, status: ProcessStatus) {
    let mut record = lock(&entry.record);
    if record.status == ProcessStatus::Running {
        record.status = status;
    }
    entry.stop.cancel();
}
async fn write_control(input: &AsyncMutex<Option<ChildStdin>>, value: Value) -> Result<(), Error> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| failed("Invalid process input"))?;
    bytes.push(b'\n');
    let mut slot = input.lock().await;
    slot.as_mut()
        .ok_or_else(|| failed("Process stdin is closed"))?
        .write_all(&bytes)
        .await
        .map_err(|_| failed("Process stdin is closed"))
}
fn trim_completed(entries: &mut HashMap<String, Arc<Entry>>, limit: usize) {
    while entries.len() > limit {
        let oldest = entries
            .iter()
            .filter_map(|(id, entry)| {
                let record = lock(&entry.record);
                (record.status != ProcessStatus::Running
                    && *entry.done.borrow()
                    && record.completion_notified
                    && (!record.ready || record.ready_notified))
                    .then_some((id.clone(), record.started_at))
            })
            .min_by_key(|(_, at)| *at);
        if let Some((id, _)) = oldest {
            entries.remove(&id);
        } else {
            break;
        }
    }
}
impl Processes {
    /// Must be initialized after taking the conversation's RootStore lease.
    pub async fn open(options: ProcessOptions) -> Result<Self, Error> {
        if !(1..=32).contains(&options.max_running)
            || options.max_retained < options.max_running
            || options.max_retained > 4096
            || options.max_output_bytes == 0
            || options.max_output_bytes > 1024 * 1024 * 1024
        {
            return Err(failed("Invalid process limits"));
        }
        tokio::fs::create_dir_all(&options.directory)
            .await
            .map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(&options.directory, std::fs::Permissions::from_mode(0o700))
                .await
                .map_err(|_| Error::Storage)?;
        }
        let capacity = Arc::new(Semaphore::new(options.max_running));
        let options = Arc::new(options);
        let mut entries = HashMap::new();
        let mut directory = tokio::fs::read_dir(&options.directory)
            .await
            .map_err(|_| Error::Storage)?;
        while let Some(file) = directory.next_entry().await.map_err(|_| Error::Storage)? {
            let path = file.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(id) = name
                .strip_suffix(".json")
                .filter(|id| uuid::Uuid::parse_str(id).is_ok())
            else {
                continue;
            };
            if !file
                .file_type()
                .await
                .map_err(|_| Error::Storage)?
                .is_file()
                || file.metadata().await.map_err(|_| Error::Storage)?.len() > 64000
            {
                continue;
            }
            let Ok(bytes) = tokio::fs::read(&path).await else {
                continue;
            };
            let Ok(mut record) = serde_json::from_slice::<ProcessRecord>(&bytes) else {
                continue;
            };
            if record.id != id {
                continue;
            }
            if record.status == ProcessStatus::Running {
                wait_for_prior_command(&options.directory, id).await?;
                record.status = ProcessStatus::Interrupted;
                record.finished_at = Some(now());
            }
            record.bytes = tokio::fs::metadata(artifact(&options.directory, id))
                .await
                .map_or(record.bytes, |m| m.len());
            let (_, done) = watch::channel(true);
            let (_, initialized) = watch::channel(true);
            let entry = Arc::new(Entry {
                record: Mutex::new(record),
                input: AsyncMutex::new(None),
                stop: CancellationToken::new(),
                done,
                initialized,
                persistence: AsyncMutex::new(()),
                delivery: AsyncMutex::new(()),
                storage_error: AtomicBool::new(false),
                ownership: AtomicU8::new(0),
            });
            persist(&entry, &options.directory).await?;
            entries.insert(id.to_owned(), entry);
            trim_completed(&mut entries, options.max_retained);
            if entries.len() > options.max_retained {
                return Err(Error::paused(
                    "The process notification backlog is full. Recover pending notifications before starting more commands.",
                ));
            }
        }
        let manager = Self(Arc::new(Inner {
            options,
            entries: Mutex::new(entries),
            capacity,
            lifecycle: Arc::new(tokio::sync::RwLock::new(())),
            closed: AtomicBool::new(false),
            shutdown: CancellationToken::new(),
        }));
        let weak = Arc::downgrade(&manager.0);
        tokio::spawn(async move {
            loop {
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                let lifecycle = inner.lifecycle.clone();
                let shutdown = inner.shutdown.clone();
                let options = inner.options.clone();
                let entries = lock(&inner.entries).values().cloned().collect::<Vec<_>>();
                // The retry worker must not keep the manager alive while a
                // notification waits on a consumer. Drop must cancel commands.
                drop(inner);
                let delivery = lifecycle.read_owned().await;
                if shutdown.is_cancelled() {
                    return;
                }
                for entry in entries {
                    tokio::select! {biased;_=shutdown.cancelled()=>return,_=deliver(&entry,&options)=>{}}
                }
                drop(delivery);
                tokio::select! {biased;_=shutdown.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(1))=>{}}
            }
        });
        Ok(manager)
    }
    fn own(&self, id: &str, agent: &str) -> Result<Arc<Entry>, Error> {
        lock(&self.0.entries)
            .get(id)
            .filter(|entry| {
                agent == self.0.options.root_id || lock(&entry.record).agent_id == agent
            })
            .cloned()
            .ok_or_else(|| failed("Unknown process for this agent"))
    }
    pub fn list(&self, agent: &str) -> Vec<ProcessRecord> {
        let mut entries = lock(&self.0.entries)
            .values()
            .filter_map(|entry| {
                let record = lock(&entry.record);
                (agent == self.0.options.root_id || record.agent_id == agent)
                    .then(|| record.clone())
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.started_at);
        entries
    }
    pub async fn start(
        &self,
        input: ProcessStart,
        context: &ToolContext,
    ) -> Result<ProcessRecord, Error> {
        let _admission = self.0.lifecycle.read().await;
        if self.0.closed.load(Ordering::Acquire) || context.cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        if !(100..=86400000).contains(&input.timeout_ms)
            || input.args.len() > 256
            || input.args.iter().map(String::len).sum::<usize>() > 128 * 1024
            || input.label.len() > 512
            || input
                .notify_on_output
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 256)
        {
            return Err(failed("Invalid process arguments"));
        }
        let permit = self
            .0
            .capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| failed("Running process limit reached"))?;
        {
            let mut entries = lock(&self.0.entries);
            trim_completed(&mut entries, self.0.options.max_retained.saturating_sub(1));
            if entries.len() >= self.0.options.max_retained {
                return Err(failed(
                    "Retained process limit reached. Pending notifications must be recovered first.",
                ));
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let mut file_options = tokio::fs::OpenOptions::new();
        file_options.write(true).create_new(true);
        #[cfg(unix)]
        file_options.mode(0o600);
        let output = file_options
            .open(artifact(&self.0.options.directory, &id))
            .await
            .map_err(|_| Error::Storage)?;
        let (done_tx, done) = watch::channel(false);
        let (started_tx, initialized) = watch::channel(false);
        let entry = Arc::new(Entry {
            record: Mutex::new(ProcessRecord {
                id: id.clone(),
                agent_id: context.agent_id.clone(),
                label: input.label,
                cwd: input.cwd.clone(),
                status: ProcessStatus::Running,
                exit_code: None,
                started_at: now(),
                finished_at: None,
                bytes: 0,
                ready: false,
                ready_notified: !input.notify,
                completion_notified: !input.notify,
            }),
            input: AsyncMutex::new(None),
            stop: CancellationToken::new(),
            done,
            initialized,
            persistence: AsyncMutex::new(()),
            delivery: AsyncMutex::new(()),
            storage_error: AtomicBool::new(false),
            ownership: AtomicU8::new(0),
        });
        persist(&entry, &self.0.options.directory).await?;
        if self.0.closed.load(Ordering::Acquire) || context.cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        let spec = supervisor::Specification {
            file: input.file,
            args: input.args,
            cwd: input.cwd,
            env: self.0.options.env.clone(),
            timeout_ms: input.timeout_ms,
            lease: self.0.options.directory.join(format!("{id}.lease")),
        };
        let mut entries = lock(&self.0.entries);
        if self.0.closed.load(Ordering::Acquire) || context.cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        trim_completed(&mut entries, self.0.options.max_retained.saturating_sub(1));
        if entries.len() >= self.0.options.max_retained {
            return Err(failed("Retained process limit reached"));
        }
        let mut command = Command::new(&self.0.options.command_host);
        command
            .arg(supervisor::FLAG)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|_| failed("Could not start the command supervisor"))?;
        let group = Group(child.id());
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| failed("Command supervisor has no control pipe"))?;
        // There are no await points between spawning and handing all process
        // ownership to the worker; cancelling start cannot orphan a child.
        *entry
            .input
            .try_lock()
            .map_err(|_| failed("Process control is busy"))? = Some(stdin);
        entries.insert(id, entry.clone());
        drop(entries);
        let result = lock(&entry.record).clone();
        let options = self.0.options.clone();
        let shutdown = self.0.shutdown.clone();
        let cancel = context.cancel.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _done = Done(done_tx);
            let mut _group = group;
            let result = run_process(
                &entry,
                &options,
                &mut child,
                output,
                spec,
                input.notify_on_output,
                shutdown.clone(),
                cancel,
                started_tx,
            )
            .await;
            if result.is_err() {
                entry.storage_error.store(true, Ordering::Release);
                mark_stop(&entry, ProcessStatus::Failed);
            }
            _group.kill();
            let _ = child.start_kill();
            let _ = child.wait().await;
            entry.input.lock().await.take();
            {
                let mut record = lock(&entry.record);
                if record.status == ProcessStatus::Running {
                    record.status = ProcessStatus::Failed;
                }
                record.finished_at = Some(now());
            }
            if persist(&entry, &options.directory).await.is_err() {
                entry.storage_error.store(true, Ordering::Release);
            }
            if !shutdown.is_cancelled() {
                deliver(&entry, &options).await;
            }
        });
        Ok(result)
    }
    pub async fn output(
        &self,
        id: &str,
        agent: &str,
        cursor: u64,
        max_bytes: usize,
    ) -> Result<ProcessOutput, Error> {
        if !(1..=64000).contains(&max_bytes) {
            return Err(failed("Output limit must be between 1 and 64000 bytes"));
        }
        let entry = self.own(id, agent)?;
        let path = artifact(&self.0.options.directory, id);
        let mut file = File::open(&path).await.map_err(|_| Error::Storage)?;
        let size = file.metadata().await.map_err(|_| Error::Storage)?.len();
        if cursor > size {
            return Err(failed("Output cursor is outside the artifact"));
        }
        file.seek(std::io::SeekFrom::Start(cursor))
            .await
            .map_err(|_| Error::Storage)?;
        let mut bytes = vec![0; max_bytes.max(4).min((size - cursor).min(64000) as usize)];
        let count = file.read(&mut bytes).await.map_err(|_| Error::Storage)?;
        bytes.truncate(count);
        let record = lock(&entry.record).clone();
        if cursor + (bytes.len() as u64) < size || record.status == ProcessStatus::Running {
            if let Err(error) = std::str::from_utf8(&bytes) {
                if error.error_len().is_none() {
                    bytes.truncate(error.valid_up_to());
                }
            }
        }
        Ok(ProcessOutput {
            process: record,
            text: String::from_utf8_lossy(&bytes).into_owned(),
            next_cursor: cursor + bytes.len() as u64,
            artifact: path,
            storage_error: entry.storage_error.load(Ordering::Acquire),
        })
    }
    pub async fn wait(
        &self,
        id: &str,
        agent: &str,
        timeout_ms: u64,
        cancel: &CancellationToken,
    ) -> Result<ProcessRecord, Error> {
        if timeout_ms > 30000 {
            return Err(failed("Wait is limited to 30 seconds"));
        }
        let entry = self.own(id, agent)?;
        let mut done = entry.done.clone();
        let completed = async {
            while !*done.borrow_and_update() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        };
        tokio::select! {biased;_=cancel.cancelled()=>return Err(Error::Interrupted),_=tokio::time::sleep(Duration::from_millis(timeout_ms))=>{},_=completed=>{}}
        Ok(lock(&entry.record).clone())
    }
    pub async fn write(
        &self,
        id: &str,
        agent: &str,
        data: &str,
        eof: bool,
        cancel: &CancellationToken,
    ) -> Result<(), Error> {
        if data.len() > 64000 {
            return Err(failed("Process input exceeds 64000 bytes"));
        }
        let entry = self.own(id, agent)?;
        if lock(&entry.record).status != ProcessStatus::Running {
            return Err(failed("Process stdin is closed"));
        }
        let send = async {
            let mut initialized = entry.initialized.clone();
            while !*initialized.borrow_and_update() {
                initialized
                    .changed()
                    .await
                    .map_err(|_| failed("Process stdin is closed"))?;
            }
            write_control(
                &entry.input,
                json!({"kind":"input","data":STANDARD.encode(data),"eof":eof}),
            )
            .await
        };
        tokio::select! {biased;_=cancel.cancelled()=>Err(Error::Interrupted),_=entry.stop.cancelled()=>Err(failed("Process stdin is closed")),result=send=>result}
    }
    /// Transfer a successfully returned command from its tool invocation to the
    /// session. This trusted host operation is never exposed as a model tool.
    /// Explicit process/agent interruption and manager shutdown retain ownership.
    pub fn handoff(&self, id: &str, agent: &str, cancel: &CancellationToken) -> Result<(), Error> {
        let entry = self.own(id, agent)?;
        if self.0.closed.load(Ordering::Acquire) || cancel.is_cancelled() {
            return Err(Error::Interrupted);
        }
        match entry
            .ownership
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) | Err(1) => Ok(()),
            Err(_) => Err(Error::Interrupted),
        }
    }
    pub async fn stop(&self, id: &str, agent: &str) -> Result<(), Error> {
        let entry = self.own(id, agent)?;
        mark_stop(&entry, ProcessStatus::Cancelled);
        await_done(entry).await;
        Ok(())
    }
    pub async fn interrupt_agents(&self, ids: &[String]) {
        let entries = lock(&self.0.entries)
            .values()
            .filter(|entry| ids.contains(&lock(&entry.record).agent_id))
            .cloned()
            .collect::<Vec<_>>();
        for entry in &entries {
            mark_stop(entry, ProcessStatus::Cancelled);
        }
        for entry in entries {
            await_done(entry).await;
        }
    }
    pub async fn close(&self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.shutdown.cancel();
        let _closing = self.0.lifecycle.write().await;
        let entries = {
            let entries = lock(&self.0.entries);
            entries.values().cloned().collect::<Vec<_>>()
        };
        for entry in &entries {
            mark_stop(entry, ProcessStatus::Cancelled);
        }
        for entry in entries {
            await_done(entry).await;
        }
    }
}
async fn await_done(entry: Arc<Entry>) {
    let mut done = entry.done.clone();
    while !*done.borrow_and_update() {
        if done.changed().await.is_err() {
            break;
        }
    }
}
async fn run_process(
    entry: &Entry,
    options: &ProcessOptions,
    child: &mut Child,
    mut output: File,
    spec: supervisor::Specification,
    marker: Option<String>,
    shutdown: CancellationToken,
    cancel: CancellationToken,
    started: watch::Sender<bool>,
) -> Result<(), Error> {
    let spec = serde_json::to_value(spec).map_err(|_| failed("Invalid command specification"))?;
    write_control(&entry.input, spec).await?;
    let mut started = Some(started);
    let mut launched = false;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| failed("Command supervisor has no output pipe"))?;
    let mut frames = FramedRead::new(
        stdout,
        LinesCodec::new_with_max_length(supervisor::MAX_FRAME),
    );
    let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tail = Vec::new();
    loop {
        let frame = tokio::select! {
            biased;
            _=shutdown.cancelled()=>{mark_stop(entry,ProcessStatus::Cancelled);break;},
            _=cancel.cancelled(), if entry.ownership.load(Ordering::Acquire) != 1 => {
                if entry.ownership.compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                    mark_stop(entry,ProcessStatus::Cancelled);
                    break;
                }
                continue;
            },
            _=entry.stop.cancelled()=>break,
            _=heartbeat.tick()=>{write_control(&entry.input,json!({"kind":"lease"})).await?;continue;},
            frame=frames.next()=>frame,
        };
        let Some(Ok(frame)) = frame else {
            break;
        };
        let value: Value =
            serde_json::from_str(&frame).map_err(|_| failed("Invalid supervisor response"))?;
        match value["kind"].as_str() {
            Some("ready") if !launched => {
                write_control(&entry.input, json!({"kind":"launch"})).await?;
                launched = true;
            }
            Some("started") if launched => {
                started
                    .take()
                    .ok_or_else(|| failed("Duplicate supervisor handshake"))?
                    .send_replace(true);
            }
            Some("output") => {
                let bytes = STANDARD
                    .decode(
                        value["data"]
                            .as_str()
                            .ok_or_else(|| failed("Invalid output frame"))?,
                    )
                    .map_err(|_| failed("Invalid output frame"))?;
                if bytes.len() > 64000 {
                    return Err(failed("Oversized output frame"));
                }
                let remaining = options
                    .max_output_bytes
                    .saturating_sub(lock(&entry.record).bytes)
                    as usize;
                let accepted = &bytes[..bytes.len().min(remaining)];
                output
                    .write_all(accepted)
                    .await
                    .map_err(|_| Error::Storage)?;
                lock(&entry.record).bytes += accepted.len() as u64;
                if let Some(marker) = &marker {
                    if !lock(&entry.record).ready {
                        tail.extend_from_slice(accepted);
                        if tail
                            .windows(marker.len())
                            .any(|part| part == marker.as_bytes())
                        {
                            lock(&entry.record).ready = true;
                            persist(entry, &options.directory).await?;
                            // Delivery runs on the outbox worker, independently of
                            // output drainage and the supervisor lease heartbeat.
                        }
                        let keep = marker.len().saturating_sub(1);
                        if tail.len() > keep {
                            tail.drain(..tail.len() - keep);
                        }
                    }
                }
                if bytes.len() > remaining {
                    mark_stop(entry, ProcessStatus::OutputLimit);
                    break;
                }
            }
            Some("exit") => {
                let mut record = lock(&entry.record);
                if record.status == ProcessStatus::Running {
                    record.status = match value["status"].as_str() {
                        Some("exited") => ProcessStatus::Exited,
                        Some("cancelled") => ProcessStatus::Cancelled,
                        Some("timed_out") => ProcessStatus::TimedOut,
                        _ => ProcessStatus::Failed,
                    };
                }
                record.exit_code = value["code"].as_i64().and_then(|code| code.try_into().ok());
                break;
            }
            _ => return Err(failed("Unknown supervisor response")),
        }
    }
    output.flush().await.map_err(|_| Error::Storage)?;
    output.sync_all().await.map_err(|_| Error::Storage)?;
    Ok(())
}

async fn wait_for_prior_command(directory: &Path, id: &str) -> Result<(), Error> {
    let path = directory.join(format!("{id}.lease"));
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|_| Error::Storage)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match file.try_lock() {
            Ok(()) => {
                let _ = file.unlock();
                return Ok(());
            }
            Err(std::fs::TryLockError::WouldBlock) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(Error::paused(
                    "A previous command is still releasing ownership. Wait for it to stop before recovering this conversation.",
                ));
            }
            Err(_) => return Err(Error::Storage),
        }
    }
}
