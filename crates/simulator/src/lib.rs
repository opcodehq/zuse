//! Host simulator controller. No tools are installed and no devices boot on discovery.
mod agents;
mod process;
mod setup;
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
pub use process::{Runner, SystemRunner, Toolchain};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use zuse_protocol::simulators::{Device, Frame, Input, Platform, Requirement, State, Target};

const MAX_PNG_BYTES: usize = 5 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const BOOT_TIMEOUT: Duration = Duration::from_secs(120);
#[derive(Default)]
struct Chat {
    enabled: bool,
    agent_access: bool,
    sessions: HashMap<String, Device>,
}
struct Inner {
    stop: tokio::sync::watch::Sender<bool>,
    runner: Arc<dyn Runner>,
    tools: Toolchain,
    setup: Option<setup::DeviceSetup>,
    agents: Option<agents::Agents>,
    catalog: std::sync::Mutex<(Vec<Requirement>, Vec<Device>)>,
    chats: std::sync::Mutex<HashMap<String, Chat>>,
    /// Serializes lifecycle and input against shutdown; each capture is bounded.
    operation: Mutex<()>,
}
#[derive(Clone)]
pub struct Simulators(Arc<Inner>);
impl Default for Simulators {
    fn default() -> Self {
        Self::new(Arc::new(SystemRunner), Toolchain::default())
    }
}
impl Simulators {
    pub fn new(runner: Arc<dyn Runner>, tools: Toolchain) -> Self {
        Self(Arc::new(Inner {
            stop: tokio::sync::watch::channel(false).0,
            runner,
            tools,
            setup: None,
            agents: None,
            catalog: std::sync::Mutex::new((Vec::new(), Vec::new())),
            chats: std::sync::Mutex::new(HashMap::new()),
            operation: Mutex::new(()),
        }))
    }
    pub fn managed(root: std::path::PathBuf) -> Self {
        let tools = Toolchain::default();
        let mut simulators = Self::new(Arc::new(SystemRunner), tools.clone());
        let inner = Arc::get_mut(&mut simulators.0).unwrap();
        inner.setup = Some(setup::DeviceSetup::system(root.clone(), tools.clone()));
        inner.agents = Some(agents::Agents::new(root, tools));
        simulators
    }
    fn sync_setup(&self) {
        if let Some(setup) = &self.0.setup {
            let chats = self.chats();
            if self.is_stopped() {
                setup.stop();
                return;
            }
            setup.configure(
                chats.values().any(|c| c.enabled),
                chats.values().any(|c| c.enabled && c.agent_access),
            );
        }
    }
    pub fn preferences(&self, chat_id: &str) -> (bool, bool) {
        self.chats()
            .get(chat_id)
            .map(|c| (c.enabled, c.agent_access))
            .unwrap_or_default()
    }
    fn chats(&self) -> std::sync::MutexGuard<'_, HashMap<String, Chat>> {
        self.0.chats.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn forget_chat(&self, chat_id: &str) {
        self.chats().remove(chat_id);
        if let Some(agents) = &self.0.agents {
            agents.revoke_chat(chat_id);
        }
        self.reconcile_setup();
    }
    pub fn retain_chats(&self, ids: &std::collections::HashSet<String>) {
        self.chats().retain(|id, _| ids.contains(id));
        if let Some(agents) = &self.0.agents {
            agents.retain(ids);
        }
        self.reconcile_setup();
    }
    fn reconcile_setup(&self) {
        if let Some(setup) = &self.0.setup {
            let chats = self.chats();
            if !chats.values().any(|c| c.enabled) {
                setup.stop();
            } else if !chats.values().any(|c| c.enabled && c.agent_access) {
                setup.revoke_agents();
            }
        }
    }
    fn is_stopped(&self) -> bool {
        *self.0.stop.borrow()
    }
    async fn stopped(&self) {
        let mut stop = self.0.stop.subscribe();
        let _ = stop.wait_for(|stopped| *stopped).await;
    }
    pub fn shutdown(&self) {
        self.0.stop.send_replace(true);
        self.chats().clear();
        if let Some(agents) = &self.0.agents {
            agents.shutdown();
        }
        if let Some(setup) = &self.0.setup {
            setup.stop();
        }
    }
    async fn command(&self, program: &str, args: &[String], timeout: Duration) -> Result<Vec<u8>> {
        tokio::select! {biased;
            _=self.stopped()=>bail!("Simulator host stopped"),
            result=self.0.runner.run(program,args,timeout)=>result
        }
    }
    async fn run(&self, program: &str, args: &[&str]) -> Result<Vec<u8>> {
        self.command(
            program,
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            COMMAND_TIMEOUT,
        )
        .await
    }
    async fn simctl(&self, args: &[&str]) -> Result<Vec<u8>> {
        ensure!(self.0.tools.macos, "iOS Simulator requires macOS and Xcode");
        let mut full = vec!["simctl"];
        full.extend_from_slice(args);
        self.run(&self.0.tools.xcrun, &full).await
    }
    async fn adb(&self, serial: &str, args: &[&str]) -> Result<Vec<u8>> {
        ensure!(
            emulator_serial(serial),
            "Only Android emulators are supported"
        );
        let mut full = vec!["-s", serial];
        full.extend_from_slice(args);
        self.run(&self.0.tools.adb, &full).await
    }
    pub async fn configure(&self, chat_id: &str, enabled: bool, agent_access: bool) {
        if self.is_stopped() {
            return;
        }
        let mut chats = self.chats();
        if self.is_stopped() {
            return;
        }
        let chat = chats.entry(chat_id.into()).or_default();
        chat.enabled = enabled;
        chat.agent_access = enabled && agent_access;
        if !enabled {
            chat.sessions.clear();
        }
        if let Some(agents) = &self.0.agents {
            if enabled && agent_access {
                agents.allow(chat_id);
            } else {
                agents.revoke_chat(chat_id);
            }
        }
        drop(chats);
        self.sync_setup();
    }
    pub async fn authorize(&self, chat_id: &str, agent: bool) -> Result<()> {
        ensure!(!self.is_stopped(), "Simulator host stopped");
        let chats = self.chats();
        let chat = chats
            .get(chat_id)
            .context("Enable device support in this chat's Simulator panel")?;
        ensure!(
            chat.enabled,
            "Enable device support in this chat's Simulator panel"
        );
        ensure!(
            !agent || chat.agent_access,
            "Enable agent access in this chat's Simulator panel"
        );
        Ok(())
    }
    pub async fn state(&self, chat_id: &str) -> State {
        if self.is_stopped() {
            return State::default();
        }
        let (enabled, agents) = self.preferences(chat_id);
        if let Some(setup) = &self.0.setup {
            if !enabled {
                setup.inspect().await;
            }
        }
        let setup = self.0.setup.as_ref().map(|s| s.snapshot(agents));
        let preparing = setup.as_ref().is_some_and(|s| {
            enabled
                && (s.status != zuse_protocol::simulators::SetupStatus::Ready
                    || agents && s.agent_status != zuse_protocol::simulators::AgentStatus::Ready)
        });
        if !preparing {
            let catalog = self.discover().await;
            *self.0.catalog.lock().unwrap() = catalog;
        }
        self.snapshot(chat_id)
    }
    /// Mutation replies must remain immediate even if host discovery is slow.
    pub fn snapshot(&self, chat_id: &str) -> State {
        if self.is_stopped() {
            return State::default();
        }
        let (enabled, agent_access, mut sessions) = {
            let chats = self.chats();
            let chat = chats.get(chat_id);
            (
                chat.is_some_and(|c| c.enabled),
                chat.is_some_and(|c| c.agent_access),
                chat.map(|c| c.sessions.values().cloned().collect::<Vec<Device>>())
                    .unwrap_or_default(),
            )
        };
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        let (requirements, devices) = self.0.catalog.lock().unwrap().clone();
        let setup = self.0.setup.as_ref().map(|s| s.snapshot(agent_access));
        State {
            enabled,
            agent_access,
            sessions,
            requirements,
            devices,
            setup,
        }
    }
    async fn discover(&self) -> (Vec<Requirement>, Vec<Device>) {
        let mut requirements = Vec::new();
        let mut devices = Vec::new();
        if self.0.tools.macos {
            match self.simctl(&["list", "devices", "available", "--json"]).await.and_then(|b| parse_ios(&b)) {
                Ok(found) => {
                    requirements.push(requirement(Platform::Ios, true, "Xcode simulator tools available"));
                    if found.is_empty() { requirements.push(requirement(Platform::Ios, false, "Install an iOS runtime and create a simulator in Xcode → Settings → Components")); }
                    devices.extend(found);
                }
                Err(error) => requirements.push(requirement(Platform::Ios, false, &format!("Install Xcode, select it with xcode-select, and complete first launch: {error}"))),
            }
            if self.0.setup.is_none() {
                let input = self.run(&self.0.tools.idb, &["--help"]).await.is_ok();
                requirements.push(requirement(Platform::Ios, input, if input { "idb input helper available" } else { "For touch and typing, install idb and idb_companion: https://fbidb.io/docs/installation/" }));
            }
        } else {
            requirements.push(requirement(
                Platform::Ios,
                false,
                "iOS Simulator requires a Mac with Xcode; Android is available on this host",
            ));
        }
        let adb = self.run(&self.0.tools.adb, &["version"]).await;
        requirements.push(requirement(Platform::Android, adb.is_ok(), if adb.is_ok() { "Android Platform-Tools available" } else { "Install Android SDK Platform-Tools; set ANDROID_HOME or ANDROID_SDK_ROOT, or add adb to PATH" }));
        let avds = self.run(&self.0.tools.emulator, &["-list-avds"]).await;
        match avds {
            Ok(bytes) => {
                let names = lines(&bytes);
                requirements.push(requirement(Platform::Android, true, "Android emulator available"));
                if names.is_empty() { requirements.push(requirement(Platform::Android, false, "Create a virtual device in Android Studio → Device Manager, or use avdmanager")); }
                let running = match self.android_running().await {
                    Ok(running) => running,
                    Err(error) => {
                        requirements.push(requirement(Platform::Android,false,&format!("Could not query running emulators: {error}")));
                        HashMap::new()
                    }
                };
                devices.extend(names.into_iter().filter(|n| adb.is_ok() && valid_avd(n)).map(|name| {
                    let serial = running.get(&name).cloned();
                    Device { id: format!("android:{name}"), name, platform: Platform::Android, runtime: "Android AVD".into(), booted: serial.is_some(), serial }
                }));
            }
            Err(_) => requirements.push(requirement(Platform::Android, false, "Install Android Emulator in Android Studio → SDK Manager → SDK Tools; set ANDROID_HOME or ANDROID_SDK_ROOT")),
        }
        // AVD manager is optional when Android Studio creates the AVDs.
        if self
            .run(&self.0.tools.avdmanager, &["list", "avd", "-c"])
            .await
            .is_err()
        {
            requirements.push(requirement(Platform::Android, false, "Optional: install a compatible JDK (JAVA_HOME/PATH) and Android SDK Command-line Tools for avdmanager; Android Studio can manage devices instead"));
        }
        devices.sort_by(|a, b| {
            let platform = |p| if p == Platform::Ios { 0 } else { 1 };
            let version = |s: &str| {
                s.split(|c: char| !c.is_ascii_digit())
                    .filter_map(|n| n.parse::<u32>().ok())
                    .collect::<Vec<_>>()
            };
            platform(a.platform)
                .cmp(&platform(b.platform))
                .then_with(|| version(&b.runtime).cmp(&version(&a.runtime)))
                .then_with(|| a.name.cmp(&b.name))
        });
        (requirements, devices)
    }
    async fn android_running(&self) -> Result<HashMap<String, String>> {
        let bytes = self.run(&self.0.tools.adb, &["devices"]).await?;
        let mut running = HashMap::new();
        for line in String::from_utf8_lossy(&bytes).lines() {
            let mut fields = line.split_whitespace();
            if let (Some(serial), Some("device")) = (fields.next(), fields.next()) {
                if !emulator_serial(serial) {
                    continue;
                }
                if let Ok(name) = self.adb(serial, &["emu", "avd", "name"]).await {
                    if let Some(name) = lines(&name).first().filter(|n| valid_avd(n)) {
                        running.insert(name.clone(), serial.into());
                    }
                }
            }
        }
        Ok(running)
    }
    pub async fn open(&self, target: &Target, agent: bool) -> Result<Device> {
        let _operation = self.0.operation.lock().await;
        self.authorize(&target.chat_id, agent).await?;
        let mut device = self
            .discover()
            .await
            .1
            .into_iter()
            .find(|d| d.id == target.device_id)
            .context("Device not found; refresh the simulator list")?;
        if let Some(setup) = &self.0.setup {
            if device.platform == Platform::Ios || !device.booted {
                tokio::select! {biased;
                    _=self.stopped()=>bail!("Simulator host stopped"),
                    result=setup.boot(device.platform, &device.id, &device.name)=>result?
                }
                device.booted = true;
            }
        }
        match device.platform {
            Platform::Ios => {
                if !device.booted {
                    self.simctl(&["boot", &device.id]).await?;
                }
                self.command(
                    &self.0.tools.xcrun,
                    &["simctl", "bootstatus", &device.id, "-b"]
                        .map(String::from)
                        .to_vec(),
                    BOOT_TIMEOUT,
                )
                .await?;
                device.booted = true;
            }
            Platform::Android => {
                self.run(&self.0.tools.adb, &["version"]).await?;
                if device.serial.is_none() && self.0.setup.is_none() {
                    let args = ["-avd", &device.name, "-no-window", "-no-audio"]
                        .map(String::from)
                        .to_vec();
                    tokio::select! {biased;
                        _=self.stopped()=>bail!("Simulator host stopped"),
                        result=self.0.runner.start(&self.0.tools.emulator,&args)=>result?
                    }
                }
                let ready = async {
                    loop {
                        if let Some(serial) =
                            self.android_running().await?.get(&device.name).cloned()
                        {
                            if let Ok(complete) = self
                                .adb(&serial, &["shell", "getprop", "sys.boot_completed"])
                                .await
                            {
                                if String::from_utf8_lossy(&complete).trim() == "1" {
                                    return Ok::<_, anyhow::Error>(serial);
                                }
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                };
                device.serial = Some(tokio::time::timeout(BOOT_TIMEOUT, ready).await.context("Emulator boot timed out; check hardware acceleration, SDK licenses, and the AVD in Android Studio")??);
                device.booted = true;
            }
        }
        if agent && let Some(agents) = &self.0.agents {
            self.authorize(&target.chat_id, true).await?;
            ensure!(
                self.0
                    .setup
                    .as_ref()
                    .is_some_and(|s| s.snapshot(true).agent_status
                        == zuse_protocol::simulators::AgentStatus::Ready),
                "Finish installing agent tools in the Simulator pane"
            );
            tokio::select! {biased;_=self.stopped()=>bail!("Simulator host stopped"),result=agents.prepare(target,&device)=>result?};
        }
        let mut chats = self.chats();
        let chat = chats
            .get_mut(&target.chat_id)
            .context("Device session closed while booting")?;
        ensure!(
            chat.enabled && (!agent || chat.agent_access),
            "Device access was revoked while booting"
        );
        chat.sessions.insert(device.id.clone(), device.clone());
        Ok(device)
    }
    async fn session(&self, target: &Target, agent: bool) -> Result<Device> {
        self.authorize(&target.chat_id, agent).await?;
        let device = self
            .chats()
            .get(&target.chat_id)
            .and_then(|c| c.sessions.get(&target.device_id))
            .cloned()
            .context("Open this device in this chat first")?;
        Ok(device)
    }
    async fn verify_device(&self, device: &Device) -> Result<()> {
        if device.platform == Platform::Android {
            let serial = device.serial.as_deref().context("Emulator has no serial")?;
            let name = self.adb(serial, &["emu", "avd", "name"]).await?;
            ensure!(
                lines(&name).first() == Some(&device.name),
                "Emulator serial now belongs to another AVD; reconnect the intended device"
            );
        }
        Ok(())
    }
    /// Release viewers after any in-flight boot, without powering off the device.
    pub async fn close_chat(&self, chat_id: &str) {
        if let Some(agents) = &self.0.agents {
            agents.close_chat(chat_id);
        }
        let _operation = self.0.operation.lock().await;
        if let Some(agents) = &self.0.agents {
            agents.close_chat(chat_id);
        }
        if let Some(chat) = self.chats().get_mut(chat_id) {
            chat.sessions.clear();
        }
    }
    pub async fn close(&self, target: &Target, shutdown: bool, agent: bool) -> Result<()> {
        self.authorize(&target.chat_id, agent).await?;
        if let Some(agents) = &self.0.agents {
            agents.close(target);
        }
        let _operation = self.0.operation.lock().await;
        let device = self.session(target, agent).await?;
        if shutdown {
            self.verify_device(&device).await?;
            ensure!(
                !self
                    .chats()
                    .iter()
                    .any(|(id, c)| id != &target.chat_id
                        && c.sessions.contains_key(&target.device_id)),
                "Device is also open in another chat; close this viewer without shutting down"
            );
            if let Some(setup) = &self.0.setup {
                setup.shutdown_device(&device).await?;
            } else {
                match device.platform {
                    Platform::Ios => {
                        self.simctl(&["shutdown", &device.id]).await?;
                    }
                    Platform::Android => {
                        self.adb(
                            device.serial.as_deref().context("Emulator has no serial")?,
                            &["emu", "kill"],
                        )
                        .await?;
                    }
                }
            }
        }
        if let Some(agents) = &self.0.agents {
            agents.close(target);
        }
        if let Some(chat) = self.chats().get_mut(&target.chat_id) {
            chat.sessions.remove(&target.device_id);
        }
        Ok(())
    }
    pub async fn screenshot(&self, target: &Target, agent: bool) -> Result<Frame> {
        let _operation = self.0.operation.lock().await;
        let device = self.session(target, agent).await?;
        self.capture(&device).await
    }
    async fn capture(&self, device: &Device) -> Result<Frame> {
        self.verify_device(device).await?;
        let bytes = match device.platform {
            Platform::Ios => {
                self.simctl(&["io", &device.id, "screenshot", "--type=png", "-"])
                    .await?
            }
            Platform::Android => {
                self.adb(
                    device.serial.as_deref().context("Emulator has no serial")?,
                    &["exec-out", "screencap", "-p"],
                )
                .await?
            }
        };
        let (width, height) = png_size(&bytes)?;
        Ok(Frame {
            data: STANDARD.encode(bytes),
            mime_type: "image/png".into(),
            width,
            height,
        })
    }
    pub async fn input(&self, target: &Target, input: Input, agent: bool) -> Result<()> {
        validate_input(&input)?;
        let _operation = self.0.operation.lock().await;
        let device = self.session(target, agent).await?;
        self.verify_device(&device).await?;
        if agent && let Some(agents) = &self.0.agents {
            ensure!(
                self.0.setup.as_ref().is_some_and(|s| {
                    let state = s.snapshot(true);
                    state.status == zuse_protocol::simulators::SetupStatus::Ready
                        && state.agent_status == zuse_protocol::simulators::AgentStatus::Ready
                }),
                "Finish installing agent tools in the Simulator pane"
            );
            let args = match &input {
                Input::Tap { x, y } => {
                    let frame = self.capture(&device).await?;
                    let scale = if device.platform == Platform::Ios {
                        let bytes = self
                            .simctl(&["getenv", &device.id, "SIMULATOR_MAINSCREEN_SCALE"])
                            .await?;
                        let scale = std::str::from_utf8(&bytes)?.trim().parse::<f64>()?;
                        ensure!(
                            scale.is_finite() && (1.0..=4.0).contains(&scale),
                            "Invalid simulator display scale"
                        );
                        scale
                    } else {
                        1.
                    };
                    vec![
                        "click".into(),
                        (x * (frame.width as f64 / scale - 1.).max(0.))
                            .round()
                            .to_string(),
                        (y * (frame.height as f64 / scale - 1.).max(0.))
                            .round()
                            .to_string(),
                    ]
                }
                Input::Text { text } => vec!["type".into(), "--".into(), text.clone()],
                Input::Home => vec!["home".into()],
                Input::Back => vec!["back".into(), "--system".into()],
                Input::Recents => vec!["app-switcher".into()],
            };
            tokio::select! {biased;_=self.stopped()=>bail!("Simulator host stopped"),result=agents.input(target,&device,args)=>result?};
            self.authorize(&target.chat_id, true).await?;
            return Ok(());
        }
        if device.platform == Platform::Ios
            && let Some(setup) = &self.0.setup
        {
            tokio::select! {biased;
                _=self.stopped()=>bail!("Simulator host stopped"),
                result=tokio::time::timeout(Duration::from_secs(30),setup.ios_input(&device.id,&input))=>result.context("Simulator input timed out")??
            }
            self.authorize(&target.chat_id, agent).await?;
            return Ok(());
        }
        let args = match input {
            Input::Tap { x, y } => {
                let frame = self.capture(&device).await?;
                let scale = if device.platform == Platform::Ios {
                    let bytes = self
                        .simctl(&["getenv", &device.id, "SIMULATOR_MAINSCREEN_SCALE"])
                        .await?;
                    let scale: f64 = std::str::from_utf8(&bytes)?
                        .trim()
                        .parse()
                        .context("Could not read simulator display scale")?;
                    ensure!(
                        scale.is_finite() && (1.0..=4.0).contains(&scale),
                        "Invalid simulator display scale"
                    );
                    scale
                } else {
                    1.0
                };
                vec![
                    "tap".into(),
                    (x * (frame.width as f64 / scale - 1.0).max(0.0))
                        .round()
                        .to_string(),
                    (y * (frame.height as f64 / scale - 1.0).max(0.0))
                        .round()
                        .to_string(),
                ]
            }
            Input::Text { text } => {
                if device.platform == Platform::Android {
                    ensure!(
                        text.chars().all(|c| c.is_ascii() && !c.is_ascii_control())
                            && !text.contains('%'),
                        "Android typing supports printable ASCII without %. Paste Unicode through your app or an input method instead"
                    );
                    vec!["text".into(), shell_quote(&text.replace(' ', "%s"))]
                } else {
                    ensure!(
                        text.chars()
                            .all(|c| c.is_ascii() && (!c.is_ascii_control() || c == '\n')),
                        "iOS idb typing supports ASCII; Unicode input requires an app input method"
                    );
                    vec!["text".into(), text]
                }
            }
            Input::Home => vec!["button".into(), "HOME".into()],
            Input::Back => {
                ensure!(
                    device.platform == Platform::Android,
                    "Back is available on Android only"
                );
                vec!["keyevent".into(), "4".into()]
            }
            Input::Recents => {
                ensure!(
                    device.platform == Platform::Android,
                    "Recents is available on Android only"
                );
                vec!["keyevent".into(), "187".into()]
            }
        };
        match device.platform {
            Platform::Ios => {
                let mut args = args;
                let command = args.remove(0);
                let mut full = vec![
                    "ui".into(),
                    command,
                    "--udid".into(),
                    device.id,
                    "--".into(),
                ];
                full.extend(args);
                self.command(&self.0.tools.idb, &full, COMMAND_TIMEOUT).await.context("iOS input requires idb and a connected idb_companion for this UDID; see https://fbidb.io/docs/installation/")?;
            }
            Platform::Android => {
                let args = if args.first().is_some_and(|a| a == "button") {
                    vec!["keyevent".into(), "3".into()]
                } else {
                    args
                };
                let mut full = vec!["shell", "input"];
                full.extend(args.iter().map(String::as_str));
                self.adb(
                    device.serial.as_deref().context("Emulator has no serial")?,
                    &full,
                )
                .await?;
            }
        }
        Ok(())
    }
}
fn requirement(platform: Platform, ready: bool, message: &str) -> Requirement {
    Requirement {
        platform,
        ready,
        message: message.into(),
    }
}
fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "OK")
        .map(str::to_owned)
        .collect()
}
fn valid_avd(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        && !name.starts_with('-')
}
fn emulator_serial(serial: &str) -> bool {
    serial
        .strip_prefix("emulator-")
        .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
pub fn validate_input(input: &Input) -> Result<()> {
    match input {
        Input::Tap { x, y } => ensure!(
            x.is_finite() && y.is_finite() && (0.0..=1.0).contains(x) && (0.0..=1.0).contains(y),
            "Tap coordinates must be finite and between 0 and 1"
        ),
        Input::Text { text } => ensure!(
            text.len() <= 4096 && !text.contains('\0'),
            "Device text must be at most 4096 bytes without NUL"
        ),
        _ => {}
    }
    Ok(())
}
pub fn png_size(bytes: &[u8]) -> Result<(u32, u32)> {
    ensure!(
        bytes.len() >= 33
            && bytes.len() <= MAX_PNG_BYTES
            && &bytes[..8] == b"\x89PNG\r\n\x1a\n"
            && &bytes[12..16] == b"IHDR",
        "Device returned an invalid PNG screenshot"
    );
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    ensure!(
        width > 0
            && height > 0
            && width <= 8192
            && height <= 8192
            && u64::from(width) * u64::from(height) <= 32_000_000,
        "Device screenshot dimensions exceed the limit"
    );
    Ok((width, height))
}
fn parse_ios(bytes: &[u8]) -> Result<Vec<Device>> {
    let json: serde_json::Value = serde_json::from_slice(bytes)?;
    let groups = json["devices"]
        .as_object()
        .context("Invalid simctl device catalog")?;
    let mut devices = Vec::new();
    for (runtime, rows) in groups {
        if !runtime.contains(".iOS-") {
            continue;
        }
        for row in rows.as_array().context("Invalid simctl runtime")? {
            if row["isAvailable"] != true {
                continue;
            }
            let Some(id) = row["udid"].as_str().filter(|id| {
                id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
            }) else {
                continue;
            };
            devices.push(Device {
                id: id.into(),
                name: row["name"].as_str().unwrap_or("iOS Simulator").into(),
                platform: Platform::Ios,
                runtime: runtime
                    .rsplit('.')
                    .next()
                    .unwrap_or(runtime)
                    .replace('-', " "),
                booted: row["state"] == "Booted",
                serial: None,
            });
        }
    }
    Ok(devices)
}
#[cfg(test)]
mod tests;
