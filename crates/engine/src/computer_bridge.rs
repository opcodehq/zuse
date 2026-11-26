//! Cloud-chat enrollment uses a dedicated profile key. A Remote Access
//! invitation and its host key never participate in this protocol.
//!
//! A connector offers this computer's shell (or one fixed executable) to the
//! owner's attached cloud chats over outbound HTTPS. Each command the owner
//! approves is admitted with a durable receipt before it starts, rechecks its
//! approval every heartbeat while it runs, and stops its whole process group
//! on Stop, revocation, detachment, deadline, sign-out or shutdown. A command
//! whose outcome was never confirmed is never run again.
use anyhow::{Context, Result, ensure};
use p256::ecdsa::SigningKey;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;
use zuse_protocol::computer_tasks::{DEFAULT_TIMEOUT_SECS, HEARTBEAT_SECS, MAX_TIMEOUT_SECS};

/// Commands one computer runs at once; later approvals wait their turn.
const MAX_CONCURRENT: usize = 4;
/// Bytes kept from the start and the end of each output stream.
const HEAD_BYTES: usize = 4096;
const TAIL_BYTES: usize = 8192;
/// Finished receipts are removed once the server has them; unconfirmed ones
/// stay this long so their outcome can still be inspected.
const RECEIPT_RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

fn signing_key(profile: &Path) -> Result<SigningKey> {
    let dir = profile.join("computer-bridge");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = dir.join("proof.key");
    if !path.try_exists()? {
        let secret = SigningKey::random(&mut rand_core::OsRng);
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        tmp.write_all(&Zeroizing::new(secret.to_bytes())[..])?;
        tmp.as_file().sync_all()?;
        match tmp.persist_noclobber(&path) {
            Ok(_) => (),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.error.into()),
        }
    }
    let file = std::fs::File::open(&path)?;
    ensure!(file.metadata()?.is_file(), "Invalid computer bridge key");
    let mut data = Zeroizing::new(Vec::new());
    file.take(33).read_to_end(&mut data)?;
    ensure!(data.len() == 32, "Invalid computer bridge key");
    SigningKey::from_slice(&data).context("Invalid computer bridge key")
}

/// The cloud API origin: HTTPS, or HTTP only on loopback for development.
fn api_origin() -> Result<String> {
    origin(&crate::cloud::configured_api_origin())
}

fn origin(configured: &str) -> Result<String> {
    let url = reqwest::Url::parse(configured).context("Invalid cloud API address")?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    ensure!(
        (url.scheme() == "https" || (url.scheme() == "http" && loopback))
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "Invalid cloud API address"
    );
    Ok(url.origin().ascii_serialization())
}

/// Enroll this computer's bridge key (standalone CLI). A signed-out profile
/// first signs in with a code approved on another device.
pub async fn enroll(
    profile: &Path,
    label: &str,
    show_code: impl Fn(&crate::account::SignInCode),
) -> Result<String> {
    std::fs::create_dir_all(profile)?;
    let _profile_lock = crate::InstanceLock::acquire(profile)?;
    let account = crate::account::Account::open(profile)?;
    if account.status().user.is_none() {
        let (code, signed_in) = account.sign_in_with_code().await?;
        show_code(&code);
        signed_in.await?;
    }
    enroll_account(profile, label, &account).await
}

async fn enroll_account(
    profile: &Path,
    label: &str,
    account: &crate::account::Account,
) -> Result<String> {
    let label = label.trim();
    ensure!(
        !label.is_empty() && label.len() <= 128 && !label.chars().any(char::is_control),
        "Invalid computer name"
    );
    let url = format!("{}/v1/cloud/bridge/computers", api_origin()?);
    let credential = account
        .credential()
        .await
        .context("Sign in to your Zuse account to enroll a computer")?;
    let key = proof::ProofKey::new(&signing_key(profile)?)?;
    let proof = key.sign("POST", &url, &credential.access_token)?;
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()?
        .post(url)
        .bearer_auth(credential.access_token)
        .header("dpop", proof)
        .json(&json!({"label":label,"platform":std::env::consts::OS}))
        .send()
        .await?
        .error_for_status()?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Enrolled {
        device_id: String,
    }
    Ok(response.json::<Enrolled>().await?.device_id)
}

/// Why a host request failed: the server answered no, or it was unreachable.
#[derive(Debug)]
pub(crate) enum SendError {
    Rejected(u16),
    Unavailable(String),
}
impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(status) => write!(f, "The cloud refused this computer ({status})"),
            Self::Unavailable(reason) => write!(f, "Couldn't reach the cloud: {reason}"),
        }
    }
}
impl std::error::Error for SendError {}

#[async_trait::async_trait]
pub(crate) trait Transport: Send + Sync {
    async fn send(&self, body: Value) -> Result<Value, SendError>;
}

/// The host-task endpoint, authenticated by the account and this computer's key.
struct Http {
    account: crate::account::Account,
    key: proof::ProofKey,
    url: String,
    client: reqwest::Client,
}
#[async_trait::async_trait]
impl Transport for Http {
    async fn send(&self, body: Value) -> Result<Value, SendError> {
        let credential = self
            .account
            .credential()
            .await
            .map_err(|e| SendError::Unavailable(e.to_string()))?;
        let proof = self
            .key
            .sign("POST", &self.url, &credential.access_token)
            .map_err(|e| SendError::Unavailable(e.to_string()))?;
        let response = self
            .client
            .post(&self.url)
            .bearer_auth(credential.access_token)
            .header("dpop", proof)
            .json(&body)
            .send()
            .await
            .map_err(|e| SendError::Unavailable(e.without_url().to_string()))?;
        let status = response.status();
        if status.is_client_error() {
            return Err(SendError::Rejected(status.as_u16()));
        }
        if !status.is_success() {
            return Err(SendError::Unavailable(status.to_string()));
        }
        response
            .json()
            .await
            .map_err(|e| SendError::Unavailable(e.without_url().to_string()))
    }
}

/// Intervals the connector uses; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Timing {
    /// Between offers, and between a running command's checks.
    pub heartbeat: Duration,
    /// A running command stops after this long without reaching the cloud.
    pub offline_limit: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
    /// A session online this long resets the reconnect backoff.
    pub stable_after: Duration,
}
impl Default for Timing {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(HEARTBEAT_SECS),
            offline_limit: Duration::from_secs(60),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            stable_after: Duration::from_secs(60),
        }
    }
}

/// What the connector is doing, as Settings and the composer show it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    /// `off`, `starting`, `online`, `reconnecting` or `paused`.
    pub state: &'static str,
    pub device_id: Option<String>,
    pub name: Option<String>,
    pub error: Option<String>,
    /// Commands running right now.
    pub running: usize,
}

/// Admits approved commands for one enrolled host.
pub(crate) struct Host {
    argv: Vec<String>,
    dynamic: bool,
    cwd: PathBuf,
    user: String,
    journal: PathBuf,
    task: uuid::Uuid,
    transport: Arc<dyn Transport>,
    timing: Timing,
    /// False once the account that enabled the host is no longer signed in.
    still_owner: Arc<dyn Fn() -> bool + Send + Sync>,
    status: watch::Sender<BridgeStatus>,
}

/// Why a host session ended without an error.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Exit {
    Shutdown,
    AccountChanged,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Claimed {
    id: uuid::Uuid,
    command: String,
    cwd: PathBuf,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

impl Host {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        argv: Vec<String>,
        dynamic: bool,
        cwd: PathBuf,
        user: String,
        journal: PathBuf,
        transport: Arc<dyn Transport>,
        timing: Timing,
        still_owner: Arc<dyn Fn() -> bool + Send + Sync>,
        status: watch::Sender<BridgeStatus>,
    ) -> Result<Self> {
        ensure!(
            !argv.is_empty(),
            "Specify an absolute executable path and its arguments"
        );
        std::fs::create_dir_all(&journal)?;
        Ok(Self {
            argv,
            dynamic,
            cwd,
            user,
            journal,
            task: uuid::Uuid::new_v4(),
            transport,
            timing,
            still_owner,
            status,
        })
    }

    fn set_running(&self, running: usize) {
        self.status.send_if_modified(|status| {
            let changed = status.running != running;
            status.running = running;
            changed
        });
    }

    /// Offer, claim and run until shutdown or an account change. Returns an
    /// error when the cloud can't be reached and nothing is running, so the
    /// supervisor reconnects with backoff. Running commands ride out short
    /// outages on their own.
    pub(crate) async fn run(self: Arc<Self>, stop: CancellationToken) -> Result<Exit> {
        self.republish().await?;
        let preview = if self.dynamic {
            self.argv[0].clone()
        } else {
            serde_json::to_string(&self.argv)?
        };
        let offer = json!({"action":"offer","task":self.task,"command":preview,
            "cwd":self.cwd,"user":self.user,"dynamic":self.dynamic});
        let mut runs = tokio::task::JoinSet::new();
        let finish = |runs: &mut tokio::task::JoinSet<()>| {
            let mut runs = std::mem::take(runs);
            async move { while runs.join_next().await.is_some() {} }
        };
        let mut online = false;
        loop {
            while runs.try_join_next().is_some() {}
            self.set_running(runs.len());
            if !(self.still_owner)() {
                // Each running command notices on its next check and stops
                // with "signed out".
                finish(&mut runs).await;
                return Ok(Exit::AccountChanged);
            }
            match self.transport.send(offer.clone()).await {
                Ok(_) => {
                    if !online {
                        online = true;
                        self.status.send_modify(|status| {
                            status.state = "online";
                            status.error = None;
                        });
                    }
                    if runs.len() < MAX_CONCURRENT && !stop.is_cancelled() {
                        match self
                            .transport
                            .send(json!({"action":"claim","task":self.task}))
                            .await
                        {
                            Ok(Value::Null) => {}
                            Ok(value) => match serde_json::from_value::<Claimed>(value) {
                                Ok(claimed) => {
                                    let host = self.clone();
                                    let cancel = stop.child_token();
                                    runs.spawn(async move { host.run_one(claimed, cancel).await });
                                    self.set_running(runs.len());
                                    // Take any further approved work right away.
                                    continue;
                                }
                                Err(error) => {
                                    tracing::warn!(%error, "computer bridge: unreadable claim");
                                }
                            },
                            Err(error) if runs.is_empty() => return Err(error.into()),
                            Err(_) => {}
                        }
                    }
                }
                Err(error) if runs.is_empty() => return Err(error.into()),
                // Keep heartbeating; each running command decides for itself
                // when the cloud has been unreachable for too long.
                Err(error) => {
                    online = false;
                    self.status.send_modify(|status| {
                        status.state = "reconnecting";
                        status.error = Some(error.to_string());
                    });
                }
            }
            tokio::select! {
                _ = stop.cancelled() => {
                    finish(&mut runs).await;
                    return Ok(Exit::Shutdown);
                }
                _ = tokio::time::sleep(self.timing.heartbeat) => {}
                Some(_) = runs.join_next(), if !runs.is_empty() => {}
            }
        }
    }

    /// Republish results finished before a restart; drop old unconfirmed receipts.
    async fn republish(&self) -> Result<()> {
        for entry in std::fs::read_dir(&self.journal)? {
            let entry = entry?;
            let Some(request) = entry
                .file_name()
                .to_str()
                .and_then(|name| uuid::Uuid::parse_str(name).ok())
            else {
                continue;
            };
            let metadata = entry.metadata()?;
            if !metadata.is_file() || metadata.len() > 262_144 {
                continue;
            }
            let bytes = Zeroizing::new(std::fs::read(entry.path())?);
            let Ok(saved) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if saved["state"] != "finished" {
                // Admitted without a recorded result: its outcome stays
                // unknown, and it is never started again.
                let old = metadata
                    .modified()
                    .ok()
                    .and_then(|at| at.elapsed().ok())
                    .is_some_and(|age| age > RECEIPT_RETENTION);
                if old {
                    let _ = std::fs::remove_file(entry.path());
                }
                continue;
            }
            let body = json!({"action":"finish","task":saved["task"],"request":request,
                "status":saved["status"],"output":saved["output"]});
            match self.transport.send(body).await {
                Ok(_) | Err(SendError::Rejected(409)) => {
                    let _ = std::fs::remove_file(entry.path());
                }
                // Not accepted (for example a rejected proof after clock
                // skew): keep the result and try again on the next session.
                Err(error @ (SendError::Unavailable(_) | SendError::Rejected(401 | 403))) => {
                    return Err(error.into());
                }
                Err(SendError::Rejected(_)) => {}
            }
        }
        Ok(())
    }

    async fn run_one(self: Arc<Self>, claimed: Claimed, cancel: CancellationToken) {
        let request = claimed.id;
        let mut argv = self.argv.clone();
        let cwd = if self.dynamic {
            claimed.cwd
        } else {
            self.cwd.clone()
        };
        if self.dynamic {
            #[cfg(windows)]
            argv.extend([
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                claimed.command,
            ]);
            #[cfg(not(windows))]
            argv.extend(["-c".into(), claimed.command]);
        }
        let deadline = Duration::from_millis(
            claimed
                .timeout_ms
                .unwrap_or(u64::from(DEFAULT_TIMEOUT_SECS) * 1000)
                .clamp(1000, u64::from(MAX_TIMEOUT_SECS) * 1000),
        );
        let receipt = self.journal.join(request.to_string());
        if cancel.is_cancelled() {
            // Claimed as the connector stopped: it never starts, and says so.
            if write_result(
                &receipt,
                &self.journal,
                self.task,
                "stopped before start",
                "",
            )
            .is_ok()
            {
                self.deliver(request, &receipt, "stopped before start", "", &cancel)
                    .await;
            }
            return;
        }
        // A durable receipt is required before starting a process.
        if let Err(error) = write_receipt(&receipt, &self.journal, self.task, &argv, &cwd) {
            tracing::warn!(%error, "computer bridge: command not admitted");
            return;
        }
        let progress: Progress = Arc::default();
        let authorization = async {
            let mut reached = Instant::now();
            loop {
                tokio::time::sleep(self.timing.heartbeat).await;
                if !(self.still_owner)() {
                    return "stopped: signed out".to_owned();
                }
                let output = render(&progress);
                let check =
                    json!({"action":"check","task":self.task,"request":request,"output":output});
                // A check still waiting when the approval can no longer be
                // confirmed counts as lost too.
                let remaining = self.timing.offline_limit.saturating_sub(reached.elapsed());
                match tokio::time::timeout(remaining, self.transport.send(check)).await {
                    Err(_) => return "stopped: connection lost".to_owned(),
                    Ok(Ok(Value::Bool(true))) => reached = Instant::now(),
                    Ok(Ok(_) | Err(SendError::Rejected(_))) => {
                        return "stopped: approval ended".to_owned();
                    }
                    Ok(Err(SendError::Unavailable(_))) => {
                        if reached.elapsed() >= self.timing.offline_limit {
                            return "stopped: connection lost".to_owned();
                        }
                    }
                }
            }
        };
        let (status, output) = tokio::select! {
            done = run_process(&argv, &cwd, progress.clone(), deadline) => done,
            reason = authorization => (reason, render(&progress)),
            _ = cancel.cancelled() => ("stopped: connector shut down".to_owned(), render(&progress)),
        };
        if let Err(error) = write_result(&receipt, &self.journal, self.task, &status, &output) {
            tracing::warn!(%error, "computer bridge: result not saved");
        }
        self.deliver(request, &receipt, &status, &output, &cancel)
            .await;
    }

    /// Deliver a saved result once the cloud accepts it. The receipt is
    /// removed only when the cloud has the outcome; on shutdown or sign-out
    /// it stays for the next session to republish.
    async fn deliver(
        &self,
        request: uuid::Uuid,
        receipt: &Path,
        status: &str,
        output: &str,
        cancel: &CancellationToken,
    ) {
        let body = json!({"action":"finish","task":self.task,"request":request,
            "status":status,"output":output});
        let mut delay = self.timing.heartbeat;
        loop {
            match self.transport.send(body.clone()).await {
                Ok(_) | Err(SendError::Rejected(409)) => {
                    let _ = std::fs::remove_file(receipt);
                    return;
                }
                Err(SendError::Unavailable(_) | SendError::Rejected(401 | 403)) => {
                    if cancel.is_cancelled() || !(self.still_owner)() {
                        return;
                    }
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = tokio::time::sleep(delay) => {}
                    }
                    delay = (delay * 2).min(self.timing.backoff_max);
                }
                // A malformed result won't be accepted by retrying; keep it.
                Err(SendError::Rejected(_)) => return,
            }
        }
    }
}

fn write_receipt(
    receipt: &Path,
    journal: &Path,
    task: uuid::Uuid,
    argv: &[String],
    cwd: &Path,
) -> Result<()> {
    let mut record = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(receipt)
        .context(
            "This command was already admitted. Check its outcome before requesting another run",
        )?;
    record.write_all(
        serde_json::to_string(&json!({"task":task,"argv":argv,"cwd":cwd,"state":"admitted"}))?
            .as_bytes(),
    )?;
    record.sync_all()?;
    #[cfg(unix)]
    std::fs::File::open(journal)?.sync_all()?;
    Ok(())
}

fn write_result(
    receipt: &Path,
    journal: &Path,
    task: uuid::Uuid,
    status: &str,
    output: &str,
) -> Result<()> {
    let mut saved = tempfile::NamedTempFile::new_in(journal)?;
    saved.write_all(
        serde_json::to_string(
            &json!({"task":task,"state":"finished","status":status,"output":output}),
        )?
        .as_bytes(),
    )?;
    saved.as_file().sync_all()?;
    saved.persist(receipt)?;
    Ok(())
}

/// The start and end of a stream, with the size of what was left out.
#[derive(Default)]
pub(crate) struct Capture {
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    omitted: u64,
}
impl Capture {
    fn push(&mut self, mut bytes: &[u8]) {
        let room = HEAD_BYTES.saturating_sub(self.head.len()).min(bytes.len());
        self.head.extend_from_slice(&bytes[..room]);
        bytes = &bytes[room..];
        self.tail.extend(bytes);
        let over = self.tail.len().saturating_sub(TAIL_BYTES);
        if over > 0 {
            self.tail.drain(..over);
            self.omitted += over as u64;
        }
    }
    fn render(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.head).into_owned();
        if self.omitted > 0 {
            text.push_str(&format!("\n[… {} bytes omitted …]\n", self.omitted));
        }
        let (a, b) = self.tail.as_slices();
        text.push_str(&String::from_utf8_lossy(&[a, b].concat()));
        text
    }
}

type Progress = Arc<Mutex<(Capture, Capture)>>;

fn render(progress: &Progress) -> String {
    let streams = progress.lock().unwrap_or_else(|e| e.into_inner());
    format!("{}{}", streams.0.render(), streams.1.render())
}

/// Run one command in its own process group. Dropping the future (Stop,
/// revocation, shutdown) kills the group; so does the deadline.
async fn run_process(
    argv: &[String],
    cwd: &Path,
    progress: Progress,
    deadline: Duration,
) -> (String, String) {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;
    async fn drain(mut pipe: impl tokio::io::AsyncRead + Unpin, progress: Progress, stderr: bool) {
        let mut buffer = [0; 8192];
        while let Ok(n) = pipe.read(&mut buffer).await {
            if n == 0 {
                break;
            }
            let mut streams = progress.lock().unwrap_or_else(|e| e.into_inner());
            if stderr {
                streams.1.push(&buffer[..n]);
            } else {
                streams.0.push(&buffer[..n]);
            }
        }
    }
    struct Abort(tokio::task::JoinHandle<()>);
    impl Drop for Abort {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return ("start failed".into(), e.to_string()),
    };
    let group = ProcessGroup(child.id());
    let mut stdout = Abort(tokio::spawn(drain(
        child.stdout.take().unwrap(),
        progress.clone(),
        false,
    )));
    let mut stderr = Abort(tokio::spawn(drain(
        child.stderr.take().unwrap(),
        progress.clone(),
        true,
    )));
    match tokio::time::timeout(deadline, child.wait()).await {
        Ok(Ok(status)) => {
            // Background children may hold the pipes open; don't wait on them.
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                let _ = (&mut stdout.0).await;
                let _ = (&mut stderr.0).await;
            })
            .await;
            (status.to_string(), render(&progress))
        }
        Ok(Err(e)) => ("output unavailable".into(), e.to_string()),
        Err(_) => {
            // Kill the whole tree while its root still exists.
            drop(group);
            let _ = child.kill().await;
            (
                format!("timed out after {}s", deadline.as_secs()),
                render(&progress),
            )
        }
    }
}

fn shell() -> Result<String> {
    #[cfg(windows)]
    let shell =
        std::path::PathBuf::from(std::env::var_os("SystemRoot").context("Missing SystemRoot")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    #[cfg(not(windows))]
    let shell = std::path::PathBuf::from("/bin/sh");
    Ok(shell.to_str().context("Unsupported shell path")?.to_owned())
}

/// A host for this computer's shell or one fixed executable.
fn local_host(
    profile: &Path,
    account: &crate::account::Account,
    argv: Vec<String>,
    dynamic: bool,
    timing: Timing,
    status: watch::Sender<BridgeStatus>,
) -> Result<Host> {
    let mut argv = argv;
    ensure!(
        !argv.is_empty(),
        "Specify an absolute executable path and its arguments"
    );
    let executable = Path::new(&argv[0]);
    ensure!(
        executable.is_absolute() && executable.is_file(),
        "Specify an absolute executable path"
    );
    argv[0] = executable
        .canonicalize()?
        .to_str()
        .context("Unsupported executable path")?
        .to_owned();
    let cwd = if dynamic {
        PathBuf::from(
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .context("Missing home directory")?,
        )
        .canonicalize()?
    } else {
        std::env::current_dir()?.canonicalize()?
    };
    let user = std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" })
        .context("Missing OS user")?;
    let owner = account
        .status()
        .user
        .map(|u| u.id)
        .context("Sign in to use the computer bridge")?;
    let key = proof::ProofKey::new(&signing_key(profile)?)?;
    let transport = Arc::new(Http {
        account: account.clone(),
        key,
        url: format!("{}/v1/cloud/bridge/host-task", api_origin()?),
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()?,
    });
    let watched = account.clone();
    Host::new(
        argv,
        dynamic,
        cwd,
        user,
        profile.join("computer-bridge").join("runs"),
        transport,
        timing,
        Arc::new(move || watched.status().user.is_some_and(|u| u.id == owner)),
        status,
    )
}

/// Only the foreground operator supplies argv and cwd. Network replies carry a request UUID.
pub async fn serve_task(profile: &Path, argv: Vec<String>) -> Result<()> {
    serve_standalone(profile, argv, false).await
}
pub async fn serve_computer(profile: &Path) -> Result<()> {
    serve_standalone(profile, vec![shell()?], true).await
}

/// The standalone CLI connector: holds the profile lock, reconnects with
/// backoff, and stops running commands on Ctrl-C.
async fn serve_standalone(profile: &Path, argv: Vec<String>, dynamic: bool) -> Result<()> {
    std::fs::create_dir_all(profile)?;
    let _lock = crate::InstanceLock::acquire(profile)?;
    let account = crate::account::Account::open(profile)?;
    let owner = account
        .status()
        .user
        .map(|u| u.id)
        .context("Sign in to use the computer bridge")?;
    let (status, mut updates) = watch::channel(BridgeStatus {
        state: "starting",
        ..Default::default()
    });
    let stop = CancellationToken::new();
    let interrupt = stop.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        interrupt.cancel();
    });
    tokio::spawn(async move {
        let mut last = String::new();
        while updates.changed().await.is_ok() {
            let now = updates.borrow().clone();
            let line = match (now.state, now.error.as_deref()) {
                ("online", _) => "Connected. Waiting for approved commands.".to_owned(),
                ("reconnecting", Some(error)) => format!("Reconnecting: {error}"),
                ("paused", Some(error)) => error.to_owned(),
                _ => continue,
            };
            if line != last {
                eprintln!("{line}");
                last = line;
            }
        }
    });
    let timing = Timing::default();
    let still = account.clone();
    supervise(
        move |status| local_host(profile, &account, argv.clone(), dynamic, timing, status),
        Arc::new(move || still.status().user.is_some_and(|u| u.id == owner)),
        None,
        timing,
        status,
        stop,
    )
    .await;
    Ok(())
}

/// Keep a host session running: reconnect with capped exponential backoff,
/// pause while another account (or none) is signed in, stop on `stop`.
async fn supervise(
    mut host: impl FnMut(watch::Sender<BridgeStatus>) -> Result<Host>,
    still_owner: Arc<dyn Fn() -> bool + Send + Sync>,
    mut account_changes: Option<watch::Receiver<crate::account::AccountStatus>>,
    timing: Timing,
    status: watch::Sender<BridgeStatus>,
    stop: CancellationToken,
) {
    let mut backoff = timing.backoff_min;
    loop {
        if stop.is_cancelled() {
            break;
        }
        if !still_owner() {
            status.send_modify(|s| {
                s.state = "paused";
                s.running = 0;
                s.error = Some("Sign in with the account that allowed this computer.".into());
            });
            let changed = async {
                match account_changes.as_mut() {
                    Some(changes) => {
                        let _ = changes.changed().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = changed => {}
                _ = tokio::time::sleep(timing.backoff_max) => {}
            }
            continue;
        }
        let started = Instant::now();
        let result = match host(status.clone()) {
            Ok(host) => Arc::new(host).run(stop.child_token()).await,
            Err(error) => Err(error),
        };
        match result {
            Ok(Exit::Shutdown) => break,
            Ok(Exit::AccountChanged) => continue,
            Err(error) => {
                if started.elapsed() >= timing.stable_after {
                    backoff = timing.backoff_min;
                }
                status.send_modify(|s| {
                    s.state = "reconnecting";
                    s.running = 0;
                    s.error = Some(error.to_string());
                });
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(timing.backoff_max);
            }
        }
    }
    status.send_modify(|s| {
        s.state = "off";
        s.running = 0;
        s.error = None;
    });
}

/// What the engine remembers about the connector it owns, so it resumes after
/// a restart for the same account.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Enabled {
    owner: String,
    device_id: String,
    label: String,
}

fn enabled_path(profile: &Path) -> PathBuf {
    profile.join("computer-bridge").join("connector.json")
}

fn read_enabled(profile: &Path) -> Option<Enabled> {
    serde_json::from_slice(&std::fs::read(enabled_path(profile)).ok()?).ok()
}

fn write_enabled(profile: &Path, enabled: &Enabled) -> Result<()> {
    let path = enabled_path(profile);
    let dir = path.parent().context("Invalid profile")?;
    std::fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(&serde_json::to_vec(enabled)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(&path)?;
    Ok(())
}

/// The engine-owned connector for this computer's shell.
pub(crate) struct Connector {
    task: tokio::sync::Mutex<Option<(CancellationToken, tokio::task::JoinHandle<()>)>>,
    status: watch::Sender<BridgeStatus>,
}
impl Default for Connector {
    fn default() -> Self {
        Self {
            task: Default::default(),
            status: watch::channel(BridgeStatus {
                state: "off",
                ..Default::default()
            })
            .0,
        }
    }
}
impl Drop for Connector {
    fn drop(&mut self) {
        if let Some((stop, task)) = self.task.get_mut().take() {
            stop.cancel();
            task.abort();
        }
    }
}
impl Connector {
    pub fn status(&self) -> BridgeStatus {
        self.status.borrow().clone()
    }

    /// Enroll (or re-confirm) this computer and run its connector until stopped.
    pub async fn start(
        &self,
        profile: &Path,
        account: crate::account::Account,
        label: &str,
    ) -> Result<String> {
        let owner = account
            .status()
            .user
            .map(|u| u.id)
            .context("Sign in to allow cloud chats to use this computer")?;
        let id = enroll_account(profile, label, &account).await?;
        write_enabled(
            profile,
            &Enabled {
                owner,
                device_id: id.clone(),
                label: label.trim().to_owned(),
            },
        )?;
        self.launch(profile, account, Timing::default()).await;
        Ok(id)
    }

    /// Resume the connector this profile enabled earlier, if any. It waits,
    /// paused, while a different account is signed in.
    pub async fn resume(&self, profile: &Path, account: crate::account::Account) {
        if read_enabled(profile).is_some() {
            self.launch(profile, account, Timing::default()).await;
        }
    }

    async fn launch(&self, profile: &Path, account: crate::account::Account, timing: Timing) {
        let mut slot = self.task.lock().await;
        // Read under the lock: a concurrent Stop either removed the choice
        // first, or takes and cancels the task installed here.
        let Some(enabled) = read_enabled(profile) else {
            return;
        };
        if let Some((stop, task)) = slot.take() {
            stop.cancel();
            let _ = task.await;
        }
        self.status.send_replace(BridgeStatus {
            state: "starting",
            device_id: Some(enabled.device_id.clone()),
            name: Some(enabled.label.clone()),
            error: None,
            running: 0,
        });
        let stop = CancellationToken::new();
        let profile = profile.to_owned();
        let status = self.status.clone();
        let owner = enabled.owner.clone();
        let watched = account.clone();
        let changes = account.watch();
        let session_stop = stop.clone();
        let task = tokio::spawn(async move {
            let shell = match shell() {
                Ok(shell) => shell,
                Err(error) => {
                    status.send_modify(|s| {
                        s.state = "off";
                        s.error = Some(error.to_string());
                    });
                    return;
                }
            };
            supervise(
                move |status| {
                    local_host(
                        &profile,
                        &account,
                        vec![shell.clone()],
                        true,
                        timing,
                        status,
                    )
                },
                Arc::new(move || watched.status().user.is_some_and(|u| u.id == owner)),
                Some(changes),
                timing,
                status,
                session_stop,
            )
            .await;
        });
        *slot = Some((stop, task));
    }

    /// Stop admitting work, stop running commands, and forget the choice.
    /// Fails, after stopping, when the saved choice couldn't be removed (the
    /// connector would come back on the next start).
    pub async fn stop(&self, profile: &Path) -> Result<()> {
        let mut slot = self.task.lock().await;
        let forgotten = match std::fs::remove_file(enabled_path(profile)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
        Self::finish(slot.take()).await;
        self.status.send_replace(BridgeStatus {
            state: "off",
            ..Default::default()
        });
        forgotten
            .context("Stopped, but couldn't forget this choice; it will start again with the app")
    }

    /// Engine shutdown: stop running commands and wait for their outcome to
    /// be saved, keeping the choice so the connector resumes next start.
    pub async fn shutdown(&self) {
        let task = self.task.lock().await.take();
        Self::finish(task).await;
    }

    async fn finish(task: Option<(CancellationToken, tokio::task::JoinHandle<()>)>) {
        if let Some((stop, mut task)) = task {
            stop.cancel();
            // Running commands record their stop and report it briefly.
            if tokio::time::timeout(Duration::from_secs(8), &mut task)
                .await
                .is_err()
            {
                tracing::warn!("computer bridge: connector stop timed out");
                task.abort();
            }
        }
    }
}

impl Connector {
    /// The owner is sending a message from this computer. If it is enabled
    /// for `account_id`, it becomes the chat's primary, proven with its
    /// enrolled key. Otherwise the chat's primary is cleared: the user isn't
    /// typing on any enabled computer. Returns the primary's device ID.
    pub async fn claim_primary(
        &self,
        profile: &Path,
        account: &crate::account::Account,
        account_id: &str,
        workspace: &str,
        chat: &str,
    ) -> Result<Option<String>> {
        claim_primary_at(
            &api_origin()?,
            profile,
            account,
            account_id,
            workspace,
            chat,
        )
        .await
    }
}

async fn claim_primary_at(
    origin: &str,
    profile: &Path,
    account: &crate::account::Account,
    account_id: &str,
    workspace: &str,
    chat: &str,
) -> Result<Option<String>> {
    ensure!(
        account.status().user.is_some_and(|u| u.id == account_id),
        "Your account changed. Reopen this chat."
    );
    let segment = |value: &str| -> Result<String> {
        ensure!(
            !value.is_empty()
                && value.len() <= 512
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
            "Invalid chat"
        );
        Ok(value.to_owned())
    };
    let (workspace, chat) = (segment(workspace)?, segment(chat)?);
    let credential = account.credential().await?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?;
    if read_enabled(profile).is_none_or(|e| e.owner != account_id) {
        client
            .delete(format!(
                "{origin}/v1/cloud/workspaces/{workspace}/chats/{chat}/primary-computer"
            ))
            .bearer_auth(credential.access_token)
            .send()
            .await
            .map_err(|e| e.without_url())?
            .error_for_status()
            .map_err(|e| e.without_url())?;
        return Ok(None);
    }
    let url = format!("{origin}/v1/cloud/bridge/primary");
    let key = proof::ProofKey::new(&signing_key(profile)?)?;
    let signed = key.sign("POST", &url, &credential.access_token)?;
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Claimed {
        device_id: String,
    }
    let claimed: Claimed = client
        .post(url)
        .bearer_auth(credential.access_token)
        .header("dpop", signed)
        .json(&json!({"workspaceId":workspace,"chatId":chat}))
        .send()
        .await
        .map_err(|e| e.without_url())?
        .error_for_status()
        .map_err(|e| e.without_url())?
        .json()
        .await?;
    Ok(Some(claimed.device_id))
}

/// Kills a command's whole process group when dropped.
struct ProcessGroup(Option<u32>);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0 {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if let Some(pid) = self.0 {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .creation_flags(0x08000000)
                .status();
        }
    }
}

mod proof;
#[cfg(all(test, unix))]
mod tests;
