//! Pinned, host-owned helpers. Checking never installs; only explicit enable/retry does.
use crate::{Runner, SystemRunner, Toolchain};
use anyhow::{Context, Result, bail, ensure};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    process::{Child, Command},
    task::JoinHandle,
};
use zuse_protocol::simulators::{AgentStatus, Setup, SetupRequirement, SetupStatus};

pub const HUB: Tool = Tool {
    name: "expo-device-hub",
    version: "0.15.3",
    entry: "dist/server/cli.mjs",
};
pub const AGENT: Tool = Tool {
    name: "agent-device",
    version: "0.21.23",
    entry: "bin/agent-device.mjs",
};
#[derive(Clone, Copy)]
pub struct Tool {
    pub name: &'static str,
    pub version: &'static str,
    pub entry: &'static str,
}
impl Tool {
    fn directory(self, root: &Path) -> PathBuf {
        root.join(self.name).join(self.version)
    }
    pub fn entry(self, root: &Path) -> PathBuf {
        self.directory(root)
            .join("node_modules")
            .join(self.name)
            .join(self.entry)
    }
}

/// A lease owns the whole helper process group, including native descendants.
pub(crate) struct Process {
    child: Child,
    pid: Option<u32>,
}
impl Process {
    pub fn start(command: &mut Command) -> Result<Self> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().context("Could not start device helper")?;
        Ok(Self {
            pid: child.id(),
            child,
        })
    }
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }
    pub fn alive(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid {
            // This group was created exclusively for this owned child.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}

#[async_trait]
pub(crate) trait Hub: Send {
    async fn request(&mut self, path: &str, body: Option<Value>) -> Result<Value>;
    fn origin(&self) -> &str;
}
#[async_trait]
pub(crate) trait Runtime: Send + Sync {
    async fn check(&self) -> Result<Vec<SetupRequirement>>;
    async fn install(&self, tool: Tool) -> Result<PathBuf>;
    async fn start_hub(&self, entry: &Path) -> Result<Box<dyn Hub>>;
}
pub(crate) struct SystemRuntime {
    pub root: PathBuf,
    pub tools: Toolchain,
}
fn supported_node(version: &str) -> bool {
    let mut parts = version.trim().trim_start_matches('v').split('.');
    matches!((parts.next().and_then(|s|s.parse::<u32>().ok()),parts.next().and_then(|s|s.parse::<u32>().ok())), (Some(major),Some(minor)) if major>22 || major==22 && minor>=12)
}
pub(crate) fn private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn installed(root: &Path, tool: Tool) -> bool {
    let base = tool.directory(root);
    let manifest = base
        .join("node_modules")
        .join(tool.name)
        .join("package.json");
    tool.entry(root).is_file()
        && std::fs::read_to_string(base.join(".install-complete"))
            .is_ok_and(|s| s.trim() == tool.version)
        && std::fs::read(manifest)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .is_some_and(|p| p["version"] == tool.version)
}
#[async_trait]
impl Runtime for SystemRuntime {
    async fn check(&self) -> Result<Vec<SetupRequirement>> {
        let node = SystemRunner
            .run("node", &["--version".into()], Duration::from_secs(10))
            .await;
        let node = node
            .as_ref()
            .map(|s| String::from_utf8_lossy(s).trim().to_owned())
            .unwrap_or_default();
        let npm = SystemRunner
            .run("npm", &["--version".into()], Duration::from_secs(10))
            .await
            .is_ok();
        let ios = self.tools.macos
            && SystemRunner
                .run(
                    &self.tools.xcrun,
                    &[
                        "simctl".into(),
                        "list".into(),
                        "devices".into(),
                        "available".into(),
                        "--json".into(),
                    ],
                    Duration::from_secs(15),
                )
                .await
                .is_ok();
        let android = SystemRunner
            .run(
                &self.tools.adb,
                &["version".into()],
                Duration::from_secs(10),
            )
            .await
            .is_ok()
            && SystemRunner
                .run(
                    &self.tools.emulator,
                    &["-list-avds".into()],
                    Duration::from_secs(10),
                )
                .await
                .is_ok();
        Ok(vec![
            SetupRequirement { label:"Node.js".into(), ready:supported_node(&node), detail:if supported_node(&node) {node} else {"Install Node.js 22.12 or newer on this host, then restart Zuse.".into()} },
            SetupRequirement {label:"npm".into(),ready:npm,detail:if npm {"Available"} else {"Install npm with Node.js on this host."}.into()},
            SetupRequirement {label:"Device platform".into(),ready:ios||android,detail:if ios {"Xcode simulator tools available"} else if android {"Android SDK tools available"} else {"Set up Xcode on a Mac, or Android SDK Platform-Tools and Emulator on this host."}.into()},
        ])
    }
    async fn install(&self, tool: Tool) -> Result<PathBuf> {
        if installed(&self.root, tool) {
            return Ok(tool.entry(&self.root));
        }
        let parent = self.root.join(tool.name);
        private_dir(&parent)?;
        let stage = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&parent)?;
        private_dir(stage.path())?;
        // npm's lock/integrity checks apply inside an isolated, versioned prefix.
        // Cancellation drops SystemRunner's process group before staging is removed.
        let mut command = Command::new("npm");
        command
            .args([
                "install".into(),
                "--prefix".into(),
                stage.path().to_string_lossy().into_owned(),
                "--no-fund".into(),
                "--no-audit".into(),
                "--save-exact".into(),
                format!("{}@{}", tool.name, tool.version),
            ])
            .current_dir(stage.path())
            .env_remove("NODE_OPTIONS");
        crate::process::output(command, Duration::from_secs(600)).await?;
        let package = stage.path().join("node_modules").join(tool.name);
        ensure!(
            package.join(tool.entry).is_file(),
            "Installed helper entry is missing"
        );
        let manifest: Value =
            serde_json::from_slice(&tokio::fs::read(package.join("package.json")).await?)?;
        ensure!(
            manifest["version"] == tool.version,
            "Installed helper version did not match the pin"
        );
        if tool.name == AGENT.name {
            let mut command = crate::process::node_command();
            command
                .arg(package.join(tool.entry))
                .arg("--version")
                .current_dir(stage.path())
                .env("AGENT_DEVICE_NO_UPDATE_NOTIFIER", "1");
            crate::process::output(command, Duration::from_secs(15))
                .await
                .context("Agent helper verification failed")?;
        }
        tokio::fs::write(stage.path().join(".install-complete"), tool.version).await?;
        let target = tool.directory(&self.root);
        if target.exists() {
            tokio::fs::remove_dir_all(&target).await?;
        }
        tokio::fs::rename(stage.path(), &target).await?;
        Ok(tool.entry(&self.root))
    }
    async fn start_hub(&self, entry: &Path) -> Result<Box<dyn Hub>> {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let mut command = crate::process::node_command();
        crate::process::sdk_environment(&mut command, &self.tools);
        command.arg(entry).args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--hide-sidebar",
            "--hide-boot-device",
        ]);
        let process = Process::start(&mut command)?;
        Ok(Box::new(SystemHub {
            process,
            origin: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()?,
        }))
    }
}
struct SystemHub {
    process: Process,
    origin: String,
    client: reqwest::Client,
}
#[async_trait]
impl Hub for SystemHub {
    async fn request(&mut self, path: &str, body: Option<Value>) -> Result<Value> {
        ensure!(
            self.process.alive()?,
            "Device Hub stopped. Retry to reconnect."
        );
        ensure!(
            path.starts_with('/') && !path.contains(".."),
            "Invalid device route"
        );
        let url = format!("{}{path}", self.origin);
        let timeout = if body.is_some() {
            Duration::from_secs(240)
        } else {
            Duration::from_secs(5)
        };
        let request = if let Some(body) = body {
            self.client.post(url).json(&body)
        } else {
            self.client.get(url)
        };
        let mut response = request.timeout(timeout).send().await?.error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 8 * 1024 * 1024,
                "Device Hub response exceeded its limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes)?)
    }
    fn origin(&self) -> &str {
        &self.origin
    }
}

pub(crate) struct DeviceSetup {
    state: Arc<Mutex<Setup>>,
    runtime: Arc<dyn Runtime>,
    hub: Arc<tokio::sync::Mutex<Option<(u64, Box<dyn Hub>)>>>,
    cancel: tokio::sync::watch::Sender<u64>,
    prepared: Arc<Mutex<std::collections::HashMap<String, u64>>>,
    hub_job: Mutex<Option<JoinHandle<()>>>,
    agent_job: Mutex<Option<JoinHandle<()>>>,
    hub_generation: Arc<AtomicU64>,
    agent_generation: Arc<AtomicU64>,
}
impl DeviceSetup {
    pub fn system(root: PathBuf, tools: Toolchain) -> Self {
        Self::new(Arc::new(SystemRuntime { root, tools }))
    }
    fn new(runtime: Arc<dyn Runtime>) -> Self {
        Self {
            state: Arc::new(Mutex::new(Setup::default())),
            runtime,
            hub: Arc::new(tokio::sync::Mutex::new(None)),
            cancel: tokio::sync::watch::channel(0).0,
            prepared: Arc::new(Mutex::new(std::collections::HashMap::new())),
            hub_job: Mutex::new(None),
            agent_job: Mutex::new(None),
            hub_generation: Arc::new(AtomicU64::new(0)),
            agent_generation: Arc::new(AtomicU64::new(0)),
        }
    }
    pub fn snapshot(&self, agents: bool) -> Setup {
        let mut s = self.state.lock().unwrap().clone();
        if !agents {
            s.agent_status = AgentStatus::Off;
            s.agent_detail.clear();
        }
        s
    }
    pub async fn inspect(&self) {
        if self.state.lock().unwrap().status != SetupStatus::Disabled {
            return;
        }
        if let Ok(requirements) = self.runtime.check().await {
            let mut state = self.state.lock().unwrap();
            if state.status == SetupStatus::Disabled {
                state.requirements = requirements;
            }
        }
    }
    pub fn configure(&self, enabled: bool, agents: bool) {
        if !enabled {
            self.stop();
            return;
        }
        let mut hub_job = self.hub_job.lock().unwrap();
        if hub_job.as_ref().is_none_or(|job| job.is_finished()) {
            if let Some(job) = hub_job.take() {
                job.abort();
            }
            let generation = self.hub_generation.fetch_add(1, Ordering::SeqCst) + 1;
            let valid = self.hub_generation.clone();
            let runtime = self.runtime.clone();
            let state = self.state.clone();
            let shared_hub = self.hub.clone();
            {
                let mut s = state.lock().unwrap();
                s.status = SetupStatus::Checking;
                s.step = 0;
                s.detail = "Checking this host…".into();
            }
            *hub_job = Some(tokio::spawn(async move {
                let publish = |status, detail: String| {
                    let mut s = state.lock().unwrap();
                    if valid.load(Ordering::SeqCst) == generation {
                        s.status = status;
                        s.step = match status {
                            SetupStatus::Checking => 0,
                            SetupStatus::Installing => 1,
                            SetupStatus::Starting => 2,
                            SetupStatus::Discovering => 3,
                            SetupStatus::Ready => 4,
                            _ => s.step,
                        };
                        s.detail = detail;
                    }
                };
                let result: Result<()> = async {
                    let requirements = runtime.check().await?;
                    let ready = requirements.iter().all(|r| r.ready);
                    {
                        let mut s = state.lock().unwrap();
                        if valid.load(Ordering::SeqCst) == generation {
                            s.requirements = requirements;
                        }
                    }
                    if !ready {
                        publish(
                            SetupStatus::Unavailable,
                            "Finish the host requirements, then check again.".into(),
                        );
                        return Ok(());
                    }
                    publish(
                        SetupStatus::Installing,
                        format!("Installing Device Hub {}…", HUB.version),
                    );
                    let entry = runtime.install(HUB).await?;
                    publish(SetupStatus::Starting, "Starting Device Hub…".into());
                    let mut hub = runtime.start_hub(&entry).await?;
                    let ready = async {
                        loop {
                            if hub.request("/api/devices", None).await.is_ok() {
                                return;
                            }
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                    };
                    tokio::time::timeout(Duration::from_secs(30), ready)
                        .await
                        .context("Device Hub startup timed out")?;
                    publish(
                        SetupStatus::Discovering,
                        "Finding available devices…".into(),
                    );
                    let catalog = hub.request("/api/devices", None).await?;
                    ensure!(
                        catalog["simulators"].is_array() || catalog["emulators"].is_array(),
                        "Device Hub returned an invalid device catalog"
                    );
                    let mut slot = shared_hub.lock().await;
                    if valid.load(Ordering::SeqCst) != generation {
                        return Ok(());
                    }
                    *slot = Some((generation, hub));
                    drop(slot);
                    publish(SetupStatus::Ready, "Device Hub is ready".into());
                    loop {
                        tokio::time::sleep(Duration::from_secs(4)).await;
                        let mut slot = shared_hub.lock().await;
                        slot.as_mut()
                            .context("Device Hub stopped")?
                            .1
                            .request("/api/devices", None)
                            .await?;
                    }
                }
                .await;
                // Kill the lease on failure. Cancellation is handled by the job's guard below.
                if let Err(error) = result {
                    let mut slot = shared_hub.lock().await;
                    if slot.as_ref().is_some_and(|(owner, _)| *owner == generation) {
                        slot.take();
                    }
                    drop(slot);
                    publish(
                        SetupStatus::Error,
                        format!("Device setup failed: {error:#}"),
                    );
                }
            }));
        }
        drop(hub_job);
        let mut agent_job = self.agent_job.lock().unwrap();
        if !agents {
            self.agent_generation.fetch_add(1, Ordering::SeqCst);
            if let Some(job) = agent_job.take() {
                job.abort();
            }
            let mut s = self.state.lock().unwrap();
            s.agent_status = AgentStatus::Off;
            s.agent_detail.clear();
        } else if agent_job.as_ref().is_none_or(|job| job.is_finished())
            && self.state.lock().unwrap().agent_status != AgentStatus::Ready
        {
            let generation = self.agent_generation.fetch_add(1, Ordering::SeqCst) + 1;
            let valid = self.agent_generation.clone();
            let state = self.state.clone();
            let runtime = self.runtime.clone();
            *agent_job = Some(tokio::spawn(async move {
                let result: Result<()> = async {
                    loop {
                        if state.lock().unwrap().status == SetupStatus::Ready {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    {
                        let mut s = state.lock().unwrap();
                        if valid.load(Ordering::SeqCst) == generation {
                            s.agent_status = AgentStatus::Installing;
                            s.agent_detail = format!("Installing agent-device {}…", AGENT.version);
                        }
                    }
                    runtime.install(AGENT).await?;
                    Ok(())
                }
                .await;
                let mut s = state.lock().unwrap();
                if valid.load(Ordering::SeqCst) == generation {
                    match result {
                        Ok(()) => {
                            s.agent_status = AgentStatus::Ready;
                            s.agent_detail="Agent tools are ready. The first iOS interaction may take a few minutes to prepare XCTest.".into();
                        }
                        Err(e) => {
                            s.agent_status = AgentStatus::Error;
                            s.agent_detail = format!("Agent tool setup failed: {e:#}");
                        }
                    }
                }
            }));
        }
    }
    pub fn revoke_agents(&self) {
        self.agent_generation.fetch_add(1, Ordering::SeqCst);
        if let Some(job) = self.agent_job.lock().unwrap().take() {
            job.abort();
        }
        let mut s = self.state.lock().unwrap();
        s.agent_status = AgentStatus::Off;
        s.agent_detail.clear();
    }
    pub fn stop(&self) {
        let old = self.hub_generation.fetch_add(1, Ordering::SeqCst);
        self.cancel.send_replace(old + 1);
        self.prepared.lock().unwrap().clear();
        self.revoke_agents();
        if let Some(job) = self.hub_job.lock().unwrap().take() {
            job.abort();
        }
        let hub = self.hub.clone();
        if let Ok(mut slot) = hub.try_lock() {
            if slot.as_ref().is_some_and(|(owner, _)| *owner <= old) {
                slot.take();
            }
        } else if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let mut slot = hub.lock().await;
                if slot.as_ref().is_some_and(|(owner, _)| *owner <= old) {
                    slot.take();
                }
            });
        }
        let mut s = self.state.lock().unwrap();
        s.status = SetupStatus::Disabled;
        s.detail.clear();
    }
    pub async fn boot(&self, platform: crate::Platform, id: &str, name: &str) -> Result<()> {
        ensure!(
            self.state.lock().unwrap().status == SetupStatus::Ready,
            "Device Hub is not ready; finish setup in the Simulator pane"
        );
        let generation = self.hub_generation.load(Ordering::SeqCst);
        let mut cancelled = self.cancel.subscribe();
        let mut slot = self.hub.lock().await;
        let hub = &mut slot.as_mut().context("Device Hub is not running")?.1;
        let (path, body) = if platform == crate::Platform::Ios {
            ("/vendor/serve-sim/grid/api/start", json!({"udid":id}))
        } else {
            (
                "/api/devices/boot",
                json!({"platform":"android","id":name,"name":name}),
            )
        };
        let result = tokio::select! {biased;
            _=cancelled.changed()=>bail!("Device setup was cancelled"),
            result=hub.request(path,Some(body))=>result?
        };
        ensure!(
            result["ok"] == true,
            "Device Hub could not start device: {}",
            result["error"]
        );
        let mut prepared = self.prepared.lock().unwrap();
        if self.hub_generation.load(Ordering::SeqCst) == generation {
            prepared.insert(id.to_owned(), generation);
        }
        Ok(())
    }
    pub async fn shutdown_device(&self, device: &crate::Device) -> Result<()> {
        let mut cancelled = self.cancel.subscribe();
        let mut slot = self.hub.lock().await;
        let hub = &mut slot.as_mut().context("Device Hub is not running")?.1;
        let (path, body) = match device.platform {
            crate::Platform::Ios => (
                "/vendor/serve-sim/grid/api/shutdown",
                json!({"udid":device.id}),
            ),
            crate::Platform::Android => (
                "/api/devices/shutdown",
                json!({"platform":"android","id":device.serial.as_deref().context("Emulator has no serial")?}),
            ),
        };
        let result = tokio::select! {biased;_=cancelled.changed()=>bail!("Device support was disabled"),result=hub.request(path,Some(body))=>result?};
        ensure!(
            result["ok"] == true,
            "Device shutdown failed: {}",
            result["error"]
        );
        self.prepared.lock().unwrap().remove(&device.id);
        Ok(())
    }
    pub async fn ios_input(&self, id: &str, input: &crate::Input) -> Result<()> {
        let attached = self.prepared.lock().unwrap().get(id).copied()
            == Some(self.hub_generation.load(Ordering::SeqCst));
        if !attached {
            self.boot(crate::Platform::Ios, id, id).await?;
        }
        if let crate::Input::Text { text } = input {
            ensure!(
                text.chars().all(|c| hid_character(c).is_some()),
                "Simulator typing supports ASCII letters, digits, and common punctuation"
            );
        }
        use futures::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;
        let origin = {
            let mut slot = self.hub.lock().await;
            slot.as_mut()
                .context("Device Hub is not running")?
                .1
                .origin()
                .replace("http:", "ws:")
        };
        let mut socket = tokio_tungstenite::connect_async(format!(
            "{origin}/vendor/serve-sim/helper/ws?device={id}"
        ))
        .await?
        .0;
        let packet = |tag: u8, payload: Value| {
            let mut bytes = vec![tag];
            bytes.extend(serde_json::to_vec(&payload).unwrap());
            Message::Binary(bytes)
        };
        socket.send(packet(13, json!({"enabled":false}))).await?;
        tokio::time::timeout(Duration::from_secs(15), async {
            while let Some(message) = socket.next().await {
                if let Message::Binary(bytes) = message? {
                    if bytes.first() == Some(&131) {
                        return Ok(());
                    }
                    if bytes.first() == Some(&130) && bytes.len() < 65536 {
                        let metadata: Value = serde_json::from_slice(&bytes[1..])?;
                        ensure!(
                            metadata["inputUnavailable"] != true,
                            "Simulator input is unavailable. Reconnect the device."
                        );
                    }
                }
            }
            bail!("Simulator input connection closed")
        })
        .await
        .context("Simulator input handshake timed out")??;
        match input {
            crate::Input::Tap { x, y } => {
                socket
                    .send(packet(3, json!({"type":"begin","x":x,"y":y})))
                    .await?;
                socket
                    .send(packet(3, json!({"type":"end","x":x,"y":y})))
                    .await?;
            }
            crate::Input::Home => socket.send(packet(4, json!({"button":"home"}))).await?,
            crate::Input::Text { text } => {
                // Text uses USB HID usages, as the reference viewer's keyboard does.
                for c in text.chars() {
                    let (usage, shift) = hid_character(c).context(
                        "Simulator typing supports ASCII letters, digits, and common punctuation",
                    )?;
                    if shift {
                        socket
                            .send(packet(6, json!({"type":"down","usage":225})))
                            .await?;
                    }
                    socket
                        .send(packet(6, json!({"type":"down","usage":usage})))
                        .await?;
                    socket
                        .send(packet(6, json!({"type":"up","usage":usage})))
                        .await?;
                    if shift {
                        socket
                            .send(packet(6, json!({"type":"up","usage":225})))
                            .await?;
                    }
                }
            }
            _ => bail!("This input is not supported by iOS"),
        }
        socket.close(None).await?;
        Ok(())
    }
}
impl Drop for DeviceSetup {
    fn drop(&mut self) {
        self.stop();
    }
}
fn hid_character(c: char) -> Option<(u16, bool)> {
    if c.is_ascii_alphabetic() {
        return Some((
            4 + c.to_ascii_lowercase() as u16 - 'a' as u16,
            c.is_ascii_uppercase(),
        ));
    }
    if ('1'..='9').contains(&c) {
        return Some((30 + c as u16 - '1' as u16, false));
    }
    Some(match c {
        '0' => (39, false),
        ' ' => (44, false),
        '\n' => (40, false),
        '\t' => (43, false),
        '-' => (45, false),
        '_' => (45, true),
        '=' => (46, false),
        '+' => (46, true),
        '[' => (47, false),
        '{' => (47, true),
        ']' => (48, false),
        '}' => (48, true),
        '\\' => (49, false),
        '|' => (49, true),
        '`' => (53, false),
        '~' => (53, true),
        ';' => (51, false),
        ':' => (51, true),
        '\'' => (52, false),
        '"' => (52, true),
        ',' => (54, false),
        '<' => (54, true),
        '.' => (55, false),
        '>' => (55, true),
        '/' => (56, false),
        '?' => (56, true),
        '!' => (30, true),
        '@' => (31, true),
        '#' => (32, true),
        '$' => (33, true),
        '%' => (34, true),
        '^' => (35, true),
        '&' => (36, true),
        '*' => (37, true),
        '(' => (38, true),
        ')' => (39, true),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        allow_install: tokio::sync::Notify,
        block: AtomicBool,
        block_agent: AtomicBool,
        fail_agent: AtomicBool,
        platform: AtomicBool,
        dropped: Arc<AtomicBool>,
        booted: Arc<AtomicBool>,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                calls: Arc::default(),
                allow_install: tokio::sync::Notify::new(),
                block: AtomicBool::new(false),
                block_agent: AtomicBool::new(false),
                fail_agent: AtomicBool::new(false),
                platform: AtomicBool::new(true),
                dropped: Arc::new(AtomicBool::new(false)),
                booted: Arc::new(AtomicBool::new(false)),
            }
        }
    }
    struct FakeHub {
        dropped: Arc<AtomicBool>,
        booted: Arc<AtomicBool>,
        calls: Arc<Mutex<Vec<String>>>,
    }
    impl Drop for FakeHub {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }
    #[async_trait]
    impl Hub for FakeHub {
        async fn request(&mut self, path: &str, _: Option<Value>) -> Result<Value> {
            if path != "/api/devices" {
                self.calls.lock().unwrap().push(path.into());
            }
            if path == "/api/devices/boot" {
                self.booted.store(true, Ordering::SeqCst);
            }
            if path == "/api/devices/shutdown" {
                self.booted.store(false, Ordering::SeqCst);
            }
            Ok(if path == "/api/devices" {
                json!({"simulators":[],"emulators":[]})
            } else {
                json!({"ok":true})
            })
        }
        fn origin(&self) -> &str {
            "http://127.0.0.1:12345"
        }
    }
    #[async_trait]
    impl Runtime for Fake {
        async fn check(&self) -> Result<Vec<SetupRequirement>> {
            self.calls.lock().unwrap().push("check".into());
            Ok(vec![SetupRequirement {
                label: "Device platform".into(),
                ready: self.platform.load(Ordering::SeqCst),
                detail: "fixture".into(),
            }])
        }
        async fn install(&self, tool: Tool) -> Result<PathBuf> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("install:{}@{}", tool.name, tool.version));
            if self.block.load(Ordering::SeqCst)
                || tool.name == AGENT.name && self.block_agent.load(Ordering::SeqCst)
            {
                self.allow_install.notified().await;
            }
            if tool.name == AGENT.name && self.fail_agent.load(Ordering::SeqCst) {
                bail!("fixture npm failure");
            }
            Ok(PathBuf::from(tool.entry))
        }
        async fn start_hub(&self, _: &Path) -> Result<Box<dyn Hub>> {
            self.calls.lock().unwrap().push("start".into());
            Ok(Box::new(FakeHub {
                dropped: self.dropped.clone(),
                booted: self.booted.clone(),
                calls: self.calls.clone(),
            }))
        }
    }
    async fn wait(setup: &DeviceSetup, predicate: impl Fn(&Setup) -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if predicate(&setup.snapshot(true)) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn checking_is_read_only_and_missing_platform_never_installs() {
        let fake = Arc::new(Fake::default());
        let setup = DeviceSetup::new(fake.clone());
        setup.inspect().await;
        assert_eq!(&*fake.calls.lock().unwrap(), &["check"]);
        fake.platform.store(false, Ordering::SeqCst);
        setup.configure(true, true);
        wait(&setup, |s| s.status == SetupStatus::Unavailable).await;
        assert_eq!(setup.snapshot(true).step, 0);
        assert!(
            !fake
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.starts_with("install"))
        );
    }
    #[tokio::test]
    async fn enable_installs_pins_starts_hub_then_installs_agent() {
        let fake = Arc::new(Fake::default());
        let setup = DeviceSetup::new(fake.clone());
        setup.configure(true, true);
        wait(&setup, |s| s.agent_status == AgentStatus::Ready).await;
        let state = setup.snapshot(true);
        assert_eq!(state.status, SetupStatus::Ready);
        assert_eq!(state.step, 4);
        assert_eq!(state.hub_version, HUB.version);
        assert_eq!(state.agent_version, AGENT.version);
        assert_eq!(
            &*fake.calls.lock().unwrap(),
            &[
                "check",
                "install:expo-device-hub@0.15.3",
                "start",
                "install:agent-device@0.21.23"
            ]
        );
        assert_eq!(setup.snapshot(false).agent_status, AgentStatus::Off);
        setup.stop();
        tokio::task::yield_now().await;
        assert!(fake.dropped.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn duplicate_enable_shares_install_and_cancel_cannot_publish_late_ready() {
        let fake = Arc::new(Fake::default());
        fake.block.store(true, Ordering::SeqCst);
        let setup = DeviceSetup::new(fake.clone());
        setup.configure(true, false);
        wait(&setup, |s| s.status == SetupStatus::Installing).await;
        setup.configure(true, false);
        assert_eq!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.starts_with("install"))
                .count(),
            1
        );
        setup.stop();
        fake.allow_install.notify_waiters();
        tokio::task::yield_now().await;
        assert_eq!(setup.snapshot(false).status, SetupStatus::Disabled);
        assert!(!fake.calls.lock().unwrap().contains(&"start".into()));
    }
    #[tokio::test]
    async fn agent_failure_retries_without_restarting_ready_hub_and_revocation_cancels() {
        let fake = Arc::new(Fake::default());
        fake.fail_agent.store(true, Ordering::SeqCst);
        let setup = DeviceSetup::new(fake.clone());
        setup.configure(true, true);
        wait(&setup, |s| s.agent_status == AgentStatus::Error).await;
        fake.fail_agent.store(false, Ordering::SeqCst);
        setup.configure(true, true);
        wait(&setup, |s| s.agent_status == AgentStatus::Ready).await;
        assert_eq!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|s| *s == "start")
                .count(),
            1
        );
        setup.configure(true, false);
        assert_eq!(setup.snapshot(true).agent_status, AgentStatus::Off);
        assert_eq!(setup.snapshot(false).status, SetupStatus::Ready);
    }
    #[tokio::test]
    async fn cancelling_agent_install_keeps_manual_hub_running() {
        let fake = Arc::new(Fake::default());
        fake.block_agent.store(true, Ordering::SeqCst);
        let setup = DeviceSetup::new(fake.clone());
        setup.configure(true, true);
        wait(&setup, |s| s.agent_status == AgentStatus::Installing).await;
        setup.configure(true, false);
        fake.allow_install.notify_waiters();
        tokio::task::yield_now().await;
        let state = setup.snapshot(true);
        assert_eq!(state.agent_status, AgentStatus::Off);
        assert_eq!(state.status, SetupStatus::Ready);
        assert!(!fake.dropped.load(Ordering::SeqCst));
    }
    #[tokio::test]
    async fn chats_share_setup_but_one_chat_cannot_cancel_another_chat() {
        let fake = Arc::new(Fake::default());
        fake.block.store(true, Ordering::SeqCst);
        let mut controller = crate::Simulators::new(Arc::new(SystemRunner), Toolchain::default());
        Arc::get_mut(&mut controller.0).unwrap().setup = Some(DeviceSetup::new(fake.clone()));
        controller.configure("a", true, true).await;
        controller.configure("b", true, false).await;
        let setup = controller.0.setup.as_ref().unwrap();
        wait(setup, |s| s.status == SetupStatus::Installing).await;
        controller.configure("a", false, false).await;
        assert_eq!(setup.snapshot(false).status, SetupStatus::Installing);
        assert_eq!(
            fake.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|s| s.starts_with("install:"))
                .count(),
            1
        );
        controller.forget_chat("b");
        fake.allow_install.notify_waiters();
        tokio::task::yield_now().await;
        assert_eq!(setup.snapshot(false).status, SetupStatus::Disabled);
        assert!(!fake.calls.lock().unwrap().contains(&"start".into()));
    }
    struct NativeFixture(Arc<AtomicBool>);
    #[async_trait]
    impl Runner for NativeFixture {
        async fn run(&self, _: &str, args: &[String], _: Duration) -> Result<Vec<u8>> {
            let args = args.iter().map(String::as_str).collect::<Vec<_>>();
            Ok(match args.as_slice() {
                ["-list-avds"] => b"Pixel\n".to_vec(),
                ["devices"] => {
                    if self.0.load(Ordering::SeqCst) {
                        b"emulator-5554\tdevice\n".to_vec()
                    } else {
                        vec![]
                    }
                }
                ["-s", _, "emu", "avd", "name"] => b"Pixel\nOK\n".to_vec(),
                ["-s", _, "shell", "getprop", "sys.boot_completed"] => b"1\n".to_vec(),
                ["-s", _, "exec-out", "screencap", "-p"] => {
                    let mut p = vec![0; 33];
                    p[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
                    p[12..16].copy_from_slice(b"IHDR");
                    p[16..20].copy_from_slice(&10u32.to_be_bytes());
                    p[20..24].copy_from_slice(&20u32.to_be_bytes());
                    p
                }
                _ => vec![],
            })
        }
        async fn start(&self, _: &str, _: &[String]) -> Result<()> {
            panic!("managed boot must use Device Hub")
        }
    }
    #[tokio::test]
    async fn managed_hub_boot_capture_and_shutdown_keep_stable_avd_identity() {
        let fake = Arc::new(Fake::default());
        let mut controller = crate::Simulators::new(
            Arc::new(NativeFixture(fake.booted.clone())),
            Toolchain {
                macos: false,
                ..Default::default()
            },
        );
        Arc::get_mut(&mut controller.0).unwrap().setup = Some(DeviceSetup::new(fake.clone()));
        controller.configure("chat", true, false).await;
        wait(controller.0.setup.as_ref().unwrap(), |s| {
            s.status == SetupStatus::Ready
        })
        .await;
        let target = crate::Target {
            chat_id: "chat".into(),
            device_id: "android:Pixel".into(),
        };
        let device = controller.open(&target, false).await.unwrap();
        assert_eq!(device.serial.as_deref(), Some("emulator-5554"));
        assert_eq!(device.id, "android:Pixel");
        let frame = controller.screenshot(&target, false).await.unwrap();
        assert_eq!((frame.width, frame.height), (10, 20));
        controller.close(&target, true, false).await.unwrap();
        assert!(!fake.booted.load(Ordering::SeqCst));
        let calls = fake.calls.lock().unwrap();
        assert!(calls.contains(&"/api/devices/boot".into()));
        assert!(calls.contains(&"/api/devices/shutdown".into()));
    }
    #[test]
    fn cache_requires_pinned_manifest_marker_and_entry() {
        let root = tempfile::tempdir().unwrap();
        let package = AGENT.entry(root.path());
        std::fs::create_dir_all(package.parent().unwrap()).unwrap();
        std::fs::write(&package, "fixture").unwrap();
        let base = AGENT.directory(root.path());
        std::fs::write(base.join(".install-complete"), AGENT.version).unwrap();
        assert!(!installed(root.path(), AGENT));
        let manifest = base.join("node_modules/agent-device/package.json");
        std::fs::write(&manifest, b"{\"version\":\"0.0.1\"}").unwrap();
        assert!(!installed(root.path(), AGENT));
        std::fs::write(
            manifest,
            serde_json::to_vec(&json!({"version":AGENT.version})).unwrap(),
        )
        .unwrap();
        assert!(installed(root.path(), AGENT));
    }
    #[test]
    fn node_versions_and_hid_input_are_explicit() {
        for version in ["v22.12.0", "v24.0.1", "23.1.0"] {
            assert!(supported_node(version));
        }
        for version in ["v20.19.0", "v22.11.9", "invalid", "v22.nope.12"] {
            assert!(!supported_node(version));
        }
        assert_eq!(hid_character('A'), Some((4, true)));
        assert_eq!(hid_character('@'), Some((31, true)));
        assert_eq!(hid_character('界'), None);
    }
}

#[cfg(test)]
mod smoke {
    use super::*;
    #[tokio::test]
    #[ignore = "downloads the reference's pinned npm helpers and starts a real loopback hub"]
    async fn pinned_helpers_install_verify_and_start_on_linux() {
        let root = tempfile::tempdir().unwrap();
        let runtime = SystemRuntime {
            root: root.path().into(),
            tools: Toolchain::default(),
        };
        let hub_entry = runtime.install(HUB).await.unwrap();
        let agent_entry = runtime.install(AGENT).await.unwrap();
        assert!(installed(root.path(), HUB));
        assert!(installed(root.path(), AGENT));
        let version = SystemRunner
            .run(
                "node",
                &[
                    agent_entry.to_string_lossy().into_owned(),
                    "--version".into(),
                ],
                Duration::from_secs(15),
            )
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&version).contains(AGENT.version));
        let mut hub = runtime.start_hub(&hub_entry).await.unwrap();
        let catalog = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Ok(catalog) = hub.request("/api/devices", None).await {
                    break catalog;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        assert!(catalog["simulators"].is_array());
        println!(
            "Installed Device Hub {} and agent-device {}; real loopback hub returned a device catalog. No simulator or emulator booted.",
            HUB.version, AGENT.version
        );
        let agents = crate::agents::Agents::new(root.path().into(), Toolchain::default());
        agents.allow("smoke");
        agents
            .prepare(
                &crate::Target {
                    chat_id: "smoke".into(),
                    device_id: "android:No_Device".into(),
                },
                &crate::Device {
                    id: "android:No_Device".into(),
                    name: "No_Device".into(),
                    platform: crate::Platform::Android,
                    runtime: "fixture".into(),
                    booted: false,
                    serial: Some("emulator-65534".into()),
                },
            )
            .await
            .unwrap();
        agents.shutdown();
        println!("Owned agent daemon registered and stopped; no device input sent.");
        drop(hub);
    }
}
