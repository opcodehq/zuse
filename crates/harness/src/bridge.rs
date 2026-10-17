//! A single lazy SDK process multiplexes model streams. It has no tools,
//! filesystem authority, session journal, retry policy, or agent scheduler.
use crate::{Error, ModelInput, ModelOutput};
use futures::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::{ChildStdin, Command},
    sync::{Mutex as AsyncMutex, mpsc},
    task::JoinHandle,
};
use tokio_util::codec::{FramedRead, LinesCodec};
const SOURCE: &str = include_str!("../bridge/model-bridge.mjs");
const MAX_FRAME: usize = 96 * 1024 * 1024;
// No Debug implementation: credentials only cross the private engine/SDK pipe.
pub struct Credential {
    pub connection_id: String,
    pub provider: String,
    pub access_token: String,
}
type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<Value>>>>;
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
pub type EnvironmentSource = Arc<dyn Fn() -> BTreeMap<OsString, OsString> + Send + Sync>;
pub struct ModelBridge {
    directory: PathBuf,
    node: PathBuf,
    next: AtomicU64,
    worker: AsyncMutex<Option<Arc<Worker>>>,
    environment: Option<EnvironmentSource>,
    retired: AtomicBool,
}
struct Worker {
    stdin: Arc<AsyncMutex<ChildStdin>>,
    pending: Pending,
    alive: Arc<AtomicBool>,
    task: Mutex<Option<JoinHandle<()>>>,
}
impl Worker {
    fn shutdown(&self) {
        self.alive.store(false, Ordering::Release);
        lock(&self.pending).clear();
        if let Some(task) = lock(&self.task).as_ref() {
            task.abort();
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}
struct ExitGuard {
    alive: Arc<AtomicBool>,
    pending: Pending,
}
impl Drop for ExitGuard {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        lock(&self.pending).clear();
    }
}
struct RequestGuard {
    worker: Arc<Worker>,
    id: u64,
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        if lock(&self.worker.pending).remove(&self.id).is_some() {
            let stdin = self.worker.stdin.clone();
            let id = self.id;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = write(&stdin, json!({"id":id,"cancel":true})).await;
                });
            }
        }
    }
}
async fn write(stdin: &AsyncMutex<ChildStdin>, value: Value) -> Result<(), Error> {
    let mut bytes = serde_json::to_vec(&value).map_err(|_| failed("bridge_request"))?;
    if bytes.len() > MAX_FRAME {
        return Err(failed("model_input_too_large"));
    }
    bytes.push(b'\n');
    stdin
        .lock()
        .await
        .write_all(&bytes)
        .await
        .map_err(|_| failed("bridge_closed"))
}
fn failed(code: &str) -> Error {
    Error::Model {
        code: code.into(),
        status: None,
    }
}
impl ModelBridge {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            node: std::env::var_os("ZUSE_NODE_EXECUTABLE")
                .map(PathBuf::from)
                .unwrap_or_else(|| "node".into()),
            next: AtomicU64::new(1),
            worker: AsyncMutex::new(None),
            environment: None,
            retired: AtomicBool::new(false),
        }
    }
    /// Select a bundled or explicitly configured Node runtime.
    pub fn with_node(mut self, node: PathBuf) -> Self {
        self.node = node;
        self
    }

    /// Resolve process environment lazily, off the async executor. Application
    /// adapters use this for login-shell PATH discovery, never model credentials.
    pub fn with_environment_source(mut self, source: EnvironmentSource) -> Self {
        self.environment = Some(source);
        self
    }
    async fn connect(&self) -> Result<Arc<Worker>, Error> {
        let mut slot = self.worker.lock().await;
        if self.retired.load(Ordering::Acquire) {
            return Err(Error::Interrupted);
        }
        if let Some(worker) = &*slot {
            if worker.alive.load(Ordering::Acquire) {
                return Ok(worker.clone());
            }
        }
        tokio::fs::create_dir_all(&self.directory)
            .await
            .map_err(|_| Error::Storage)?;
        let digest = format!("{:x}", Sha256::digest(SOURCE.as_bytes()));
        let path = self.directory.join(format!("model-{digest}.mjs"));
        if !path.exists() {
            let temp = self
                .directory
                .join(format!(".model-{}.tmp", uuid::Uuid::new_v4()));
            tokio::fs::write(&temp, SOURCE)
                .await
                .map_err(|_| Error::Storage)?;
            tokio::fs::rename(&temp, &path)
                .await
                .map_err(|_| Error::Storage)?;
        }
        let environment = if let Some(source) = &self.environment {
            let source = source.clone();
            tokio::task::spawn_blocking(move || source())
                .await
                .map_err(|_| failed("model_runtime_environment"))?
        } else {
            BTreeMap::new()
        };
        if self.retired.load(Ordering::Acquire) {
            return Err(Error::Interrupted);
        }
        let mut command = Command::new(&self.node);
        command.envs(environment);
        let mut child = command
            .arg(&path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| failed("model_transport_requires_node_22"))?;
        let stdin = Arc::new(AsyncMutex::new(
            child.stdin.take().ok_or_else(|| failed("bridge_closed"))?,
        ));
        let stdout = child.stdout.take().ok_or_else(|| failed("bridge_closed"))?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let pending_read = pending.clone();
        let alive_read = alive.clone();
        let control = stdin.clone();
        let task = tokio::spawn(async move {
            let _exit = ExitGuard {
                alive: alive_read,
                pending: pending_read.clone(),
            };
            let mut lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(MAX_FRAME));
            let mut idle = tokio::time::interval(Duration::from_secs(60));
            idle.tick().await;
            loop {
                tokio::select! {
                    _=child.wait()=>break,
                    _=idle.tick()=>if lock(&pending_read).is_empty(){break;},
                    line=lines.next()=>{
                        let Some(Ok(line))=line else{break;};
                        let Ok(frame)=serde_json::from_str::<Value>(&line) else{break;};let Some(id)=frame["id"].as_u64() else{continue;};
                        let terminal=matches!(frame["type"].as_str(),Some("result"|"error"));
                        let sender={let mut pending=lock(&pending_read);if terminal{pending.remove(&id)}else{pending.get(&id).cloned()}};
                        if let Some(sender)=sender {
                            if sender.try_send(frame).is_err(){
                                lock(&pending_read).remove(&id);let control=control.clone();
                                tokio::spawn(async move{let _=write(&control,json!({"id":id,"cancel":true})).await;});
                            }
                        }
                    }
                }
            }
            let _ = child.start_kill();
            let _ = child.wait().await;
        });
        let worker = Arc::new(Worker {
            stdin,
            pending,
            alive,
            task: Mutex::new(Some(task)),
        });
        *slot = Some(worker.clone());
        Ok(worker)
    }
    pub async fn request(
        &self,
        input: ModelInput,
        credential: Credential,
    ) -> Result<ModelOutput, Error> {
        if !matches!(credential.provider.as_str(), "chatgpt" | "supergrok") {
            return Err(failed("invalid_model_provider"));
        }
        let worker = self.connect().await?;
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, mut rx) = mpsc::channel(64);
        {
            let mut pending = lock(&worker.pending);
            if self.retired.load(Ordering::Acquire) || !worker.alive.load(Ordering::Acquire) {
                return Err(Error::Interrupted);
            }
            if pending.len() >= 128 {
                return Err(failed("bridge_busy"));
            }
            pending.insert(id, tx);
        }
        let _guard = RequestGuard {
            worker: worker.clone(),
            id,
        };
        let frame = json!({"id":id,"input":{"rootId":input.root_id,"agentId":input.agent_id,"model":input.model,"reasoning":input.reasoning,"messages":input.messages,"tools":input.tools},"credential":{"connectionId":credential.connection_id,"provider":credential.provider,"accessToken":credential.access_token}});
        tokio::select! {biased;_=input.cancel.cancelled()=>return Err(Error::Interrupted),sent=write(&worker.stdin,frame)=>sent?,}
        loop {
            let frame = tokio::select! {biased;_=input.cancel.cancelled()=>return Err(Error::Interrupted),frame=rx.recv()=>frame.ok_or_else(||failed("bridge_closed"))?};
            match frame["type"].as_str() {
                Some("text") => {
                    if let Some(text) = frame["text"].as_str() {
                        (input.text)(text.to_owned());
                    }
                }
                Some("result") => {
                    return serde_json::from_value(frame["result"].clone())
                        .map_err(|_| failed("invalid_model_response"));
                }
                Some("error") => {
                    return Err(Error::Model {
                        code: frame["code"].as_str().unwrap_or("bridge_failed").into(),
                        status: frame["status"].as_u64().and_then(|n| n.try_into().ok()),
                    });
                }
                _ => return Err(failed("invalid_model_frame")),
            }
        }
    }
    pub async fn close(&self) {
        self.retired.store(true, Ordering::Release);
        let mut slot = self.worker.lock().await;
        if let Some(worker) = slot.take() {
            worker.shutdown();
            let task = lock(&worker.task).take();
            if let Some(task) = task {
                let _ = task.await;
            }
        }
    }
}
