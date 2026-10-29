//! Durable, host-local creation jobs. Registration remains the workspace's responsibility.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::watch,
};
use tokio_util::sync::CancellationToken;
use zuse_protocol::project_creation::*;

#[derive(Default, Serialize, Deserialize)]
struct Store {
    operations: BTreeMap<String, ProjectCreation>,
    parent: Option<String>,
}
struct Inner {
    path: PathBuf,
    device: String,
    state: Mutex<Store>,
    cancels: Mutex<BTreeMap<String, CancellationToken>>,
    updates: watch::Sender<Vec<ProjectCreation>>,
    runner: Arc<dyn CreationRunner>,
    shutdown: CancellationToken,
}
#[derive(Clone)]
pub struct ProjectCreationService(Arc<Inner>);

impl ProjectCreationService {
    pub fn open(root: &Path, device: String) -> Result<Self, String> {
        Self::open_with_runner(root, device, Arc::new(SystemCreationRunner))
    }
    fn open_with_runner(
        root: &Path,
        device: String,
        runner: Arc<dyn CreationRunner>,
    ) -> Result<Self, String> {
        let path = root.join("project-creations.json");
        let mut state: Store = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Cannot read project creation journal")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Store::default(),
            Err(_) => return Err("Cannot open project creation journal".into()),
        };
        for op in state.operations.values_mut() {
            if op.status == ProjectCreationStatus::Running {
                if (op.files_ready
                    || std::fs::read_to_string(handoff_marker(Path::new(&op.destination), &op.id))
                        .is_ok_and(|owner| owner == op.id))
                    && Path::new(&op.destination).is_dir()
                {
                    op.status = ProjectCreationStatus::Ready;
                    op.files_ready = true;
                    op.warning = Some(
                        "Local project recovered after restart. Check its files before continuing."
                            .into(),
                    );
                    if matches!(
                        op.request.source,
                        ProjectSource::Create { publish: true, .. }
                    ) {
                        op.publish_error = Some(
                            "Publishing was interrupted. Check GitHub before retrying.".into(),
                        );
                    }
                } else {
                    op.status = ProjectCreationStatus::Interrupted;
                    op.error = Some("Creation was interrupted. Retry to start again.".into());
                }
            }
        }
        let (updates, _) = watch::channel(state.operations.values().cloned().collect());
        let service = Self(Arc::new(Inner {
            path,
            device,
            state: Mutex::new(state),
            cancels: Mutex::new(BTreeMap::new()),
            updates,
            runner,
            shutdown: CancellationToken::new(),
        }));
        service.persist()?;
        for op in service.list().iter().filter(|op| op.files_ready) {
            let _ = std::fs::remove_file(handoff_marker(Path::new(&op.destination), &op.id));
        }
        Ok(service)
    }
    fn persist(&self) -> Result<(), String> {
        let state = self.0.state.lock().unwrap();
        let bytes =
            serde_json::to_vec(&*state).map_err(|_| "Cannot encode project creation journal")?;
        let tmp = self.0.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes)
            .and_then(|_| std::fs::rename(tmp, &self.0.path))
            .map_err(|_| "Cannot save project creation journal".to_string())?;
        self.0
            .updates
            .send_replace(state.operations.values().cloned().collect());
        Ok(())
    }
    pub fn list(&self) -> Vec<ProjectCreation> {
        self.0
            .state
            .lock()
            .unwrap()
            .operations
            .values()
            .cloned()
            .collect()
    }
    pub fn subscribe(&self) -> watch::Receiver<Vec<ProjectCreation>> {
        self.0.updates.subscribe()
    }
    fn update(&self, id: &str, f: impl FnOnce(&mut ProjectCreation)) {
        if let Some(op) = self.0.state.lock().unwrap().operations.get_mut(id) {
            f(op);
        }
        if let Err(error) = self.persist() {
            tracing::error!(%error, "project creation persistence failed");
        }
    }
    pub fn blocks(&self, path: &Path) -> bool {
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                std::path::Component::CurDir => {}
                value => normalized.push(value.as_os_str()),
            }
        }
        self.list()
            .iter()
            .any(|op| op.blocks_execution() && normalized.starts_with(&op.destination))
    }
    pub fn blocks_request(&self, request: &zuse_protocol::RunRequest) -> bool {
        self.blocks(Path::new(&request.cwd))
            || request
                .worktree
                .as_ref()
                .is_some_and(|worktree| self.blocks(Path::new(&worktree.repo_path)))
    }
    pub async fn default_parent(&self) -> String {
        if let Some(parent) = self.0.state.lock().unwrap().parent.clone() {
            return parent;
        }
        let home = home();
        let developer = home.join("Developer");
        if tokio::fs::metadata(&developer)
            .await
            .is_ok_and(|m| m.is_dir())
        {
            developer
        } else {
            home
        }
        .to_string_lossy()
        .into_owned()
    }
    pub async fn start(&self, request: StartProjectCreation) -> Result<ProjectCreation, String> {
        if self.0.shutdown.is_cancelled() {
            return Err("Project service is shutting down".into());
        }
        uuid::Uuid::parse_str(&request.id).map_err(|_| "Invalid operation id")?;
        if let Some(op) = self.0.state.lock().unwrap().operations.get(&request.id) {
            return if op.request == request {
                Ok(op.clone())
            } else {
                Err("Operation id already used".into())
            };
        }
        match &request.source {
            ProjectSource::Clone { url } => {
                validate_clone_name(&request.name)?;
                validate_url(url)?;
            }
            ProjectSource::Create { .. } => validate_name(&request.name)?,
        }
        let parent = if request.parent.trim().is_empty() {
            self.default_parent().await
        } else {
            expand_home(&request.parent)
        };
        let parent = tokio::fs::canonicalize(&parent)
            .await
            .map_err(|_| "Choose an existing parent folder")?;
        if !tokio::fs::metadata(&parent).await.is_ok_and(|m| m.is_dir()) {
            return Err("Choose a folder, not a file".into());
        }
        let destination = parent.join(&request.name);
        if tokio::fs::symlink_metadata(&destination).await.is_ok() {
            return Err("Destination already exists. Choose another name or folder.".into());
        }
        let op = ProjectCreation {
            id: request.id.clone(),
            device_id: self.0.device.clone(),
            destination: destination.to_string_lossy().into_owned(),
            request,
            stage: "Preparing".into(),
            status: ProjectCreationStatus::Running,
            percent: None,
            files_ready: false,
            error: None,
            warning: None,
            publish_error: None,
        };
        {
            let mut state = self.0.state.lock().unwrap();
            // Recheck under the lock: racing submissions cannot reserve the same path or id.
            if let Some(existing) = state.operations.get(&op.id) {
                return if existing.request == op.request {
                    Ok(existing.clone())
                } else {
                    Err("Operation id already used".into())
                };
            }
            if state
                .operations
                .values()
                .any(|o| o.destination == op.destination)
            {
                return Err("A creation operation already owns this destination".into());
            }
            state.operations.insert(op.id.clone(), op.clone());
        }
        if let Err(e) = self.persist() {
            self.0.state.lock().unwrap().operations.remove(&op.id);
            return Err(e);
        }
        self.launch(op.clone(), false);
        Ok(op)
    }
    fn launch(&self, op: ProjectCreation, publish_only: bool) {
        let cancel = self.0.shutdown.child_token();
        self.0
            .cancels
            .lock()
            .unwrap()
            .insert(op.id.clone(), cancel.clone());
        let service = self.clone();
        tokio::spawn(async move {
            let result = if publish_only {
                service.publish(&op, &cancel).await
            } else {
                service.produce(&op, &cancel).await
            };
            service.0.cancels.lock().unwrap().remove(&op.id);
            if publish_only {
                service.update(&op.id, |o| {
                    o.status = ProjectCreationStatus::Ready;
                    o.stage = "Ready".into();
                    o.publish_error = result.err();
                });
            } else {
                service.update(&op.id, |o| match result {
                    Ok(()) => {
                        o.status = ProjectCreationStatus::Ready;
                        o.stage = "Ready".into();
                        o.percent = Some(100);
                    }
                    Err(error) => {
                        o.status = if cancel.is_cancelled() {
                            ProjectCreationStatus::Cancelled
                        } else {
                            ProjectCreationStatus::Failed
                        };
                        o.error = Some(error);
                        o.percent = None;
                    }
                });
            }
        });
    }
    pub async fn shutdown(&self) {
        self.0.shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self.0.cancels.lock().unwrap().is_empty() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
    }
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let cancels = self.0.cancels.lock().unwrap();
        let cancel = cancels.get(id).ok_or("Operation is no longer running")?;
        cancel.cancel();
        Ok(())
    }
    pub async fn retry(&self, id: &str) -> Result<ProjectCreation, String> {
        let op = self
            .list()
            .into_iter()
            .find(|o| o.id == id)
            .ok_or("Operation not found")?;
        let publishing = op.status == ProjectCreationStatus::Ready && op.publish_error.is_some();
        if op.status == ProjectCreationStatus::Running
            || self.0.cancels.lock().unwrap().contains_key(id)
        {
            return Err("Operation is still running".into());
        }
        if op.status == ProjectCreationStatus::Ready && !publishing {
            return Ok(op);
        }
        if !publishing && tokio::fs::symlink_metadata(&op.destination).await.is_ok() {
            return Err("Destination already exists; open it or choose another name.".into());
        }
        {
            let mut state = self.0.state.lock().unwrap();
            let current = state.operations.get_mut(id).ok_or("Operation not found")?;
            if current.status == ProjectCreationStatus::Running {
                return Err("Operation is still running".into());
            }
            current.status = ProjectCreationStatus::Running;
            current.stage = if publishing {
                "Publishing private GitHub repository"
            } else {
                "Preparing"
            }
            .into();
            current.error = None;
            current.percent = None;
        }
        self.persist()?;
        let current = self.list().into_iter().find(|o| o.id == id).unwrap();
        self.launch(current.clone(), publishing);
        Ok(current)
    }
    async fn produce(
        &self,
        op: &ProjectCreation,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let destination = Path::new(&op.destination);
        let parent = destination.parent().ok_or("Invalid destination")?;
        // TempDir cleans only its own staging folder, including on failure or cancellation.
        let staging = tempfile::Builder::new()
            .prefix(".zuse-project-")
            .tempdir_in(parent)
            .map_err(|_| "Cannot write to the parent folder")?;
        let target = staging.path().join(&op.request.name);
        match &op.request.source {
            ProjectSource::Clone { url } => {
                self.stage(op, "Cloning");
                self.run(
                    op,
                    "git",
                    &[
                        "clone",
                        "--progress",
                        "--",
                        url,
                        target.to_str().ok_or("Invalid destination")?,
                    ],
                    parent,
                    cancel,
                )
                .await?;
            }
            ProjectSource::Create { template, .. } => {
                match template {
                    ProjectTemplate::Empty => {
                        tokio::fs::create_dir(&target)
                            .await
                            .map_err(|_| "Cannot create project folder")?;
                        tokio::fs::write(
                            target.join("README.md"),
                            format!("# {}\n", op.request.name),
                        )
                        .await
                        .map_err(|_| "Cannot write README")?;
                    }
                    ProjectTemplate::Nextjs => {
                        self.stage(op, "Scaffolding Next.js");
                        self.run(
                            op,
                            "bunx",
                            &[
                                "--bun",
                                "create-next-app@latest",
                                &op.request.name,
                                "--ts",
                                "--tailwind",
                                "--app",
                                "--eslint",
                                "--src-dir",
                                "--use-bun",
                                "--import-alias",
                                "@/*",
                                "--skip-install",
                                "--yes",
                                "--disable-git",
                            ],
                            staging.path(),
                            cancel,
                        )
                        .await?;
                    }
                    ProjectTemplate::Turborepo => {
                        self.stage(op, "Scaffolding Turborepo");
                        self.run(
                            op,
                            "bunx",
                            &[
                                "--bun",
                                "create-turbo@latest",
                                &op.request.name,
                                "--package-manager",
                                "bun",
                                "--skip-install",
                                "--no-git",
                            ],
                            staging.path(),
                            cancel,
                        )
                        .await?;
                    }
                }
                if *template != ProjectTemplate::Empty {
                    self.stage(op, "Installing dependencies");
                    self.run(op, "bun", &["install"], &target, cancel).await?;
                }
                self.stage(op, "Initializing Git");
                self.run(op, "git", &["init", "-b", "main"], &target, cancel)
                    .await?;
                self.run(op, "git", &["add", "--all"], &target, cancel)
                    .await?;
                let identity = self
                    .run(op, "git", &["var", "GIT_AUTHOR_IDENT"], &target, cancel)
                    .await;
                if identity.is_ok() {
                    self.run(
                        op,
                        "git",
                        &["commit", "-m", "Initial commit"],
                        &target,
                        cancel,
                    )
                    .await?;
                } else if cancel.is_cancelled() {
                    return Err("Creation cancelled".into());
                } else {
                    self.update(&op.id, |o| o.warning = Some("Created without a first commit. Configure Git user.name and user.email, then commit your files.".into()));
                }
            }
        }
        if cancel.is_cancelled() {
            return Err("Creation cancelled".into());
        }
        // The marker proves ownership if the process dies between rename and journal persistence.
        let marker = handoff_marker(&target, &op.id);
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&marker)
            .await
            .map_err(|_| "Cannot prepare project handoff")?;
        tokio::io::AsyncWriteExt::write_all(&mut file, op.id.as_bytes())
            .await
            .map_err(|_| "Cannot prepare project handoff")?;
        drop(file);
        self.stage(op, "Finishing");
        rename_new(&target, destination)?;
        {
            let mut state = self.0.state.lock().unwrap();
            state.parent = Some(parent.to_string_lossy().into_owned());
            if let Some(current) = state.operations.get_mut(&op.id) {
                current.files_ready = true;
            }
        }
        match self.persist() {
            Ok(()) => {
                let _ = tokio::fs::remove_file(handoff_marker(destination, &op.id)).await;
            }
            Err(error) => tracing::error!(%error, "project handoff journal failed"),
        }
        if matches!(
            op.request.source,
            ProjectSource::Create { publish: true, .. }
        ) {
            if let Err(error) = self.publish(op, cancel).await {
                self.update(&op.id, |o| o.publish_error = Some(error));
            }
        }
        Ok(())
    }
    async fn publish(
        &self,
        op: &ProjectCreation,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        self.stage(op, "Publishing private GitHub repository");
        let root = Path::new(&op.destination);
        // Retry never creates a second repository after a successful create with a failed push.
        if self
            .run(op, "git", &["remote", "get-url", "origin"], root, cancel)
            .await
            .is_err()
        {
            self.run(
                op,
                "gh",
                &[
                    "repo",
                    "create",
                    &op.request.name,
                    "--private",
                    "--source",
                    ".",
                    "--remote",
                    "origin",
                ],
                root,
                cancel,
            )
            .await?;
        }
        if self
            .run(op, "git", &["rev-parse", "--verify", "HEAD"], root, cancel)
            .await
            .is_ok()
        {
            self.run(op, "git", &["push", "-u", "origin", "HEAD"], root, cancel)
                .await?;
        }
        Ok(())
    }
    fn stage(&self, op: &ProjectCreation, stage: &str) {
        self.update(&op.id, |o| {
            o.stage = stage.into();
            o.percent = None;
        });
    }
    async fn run(
        &self,
        op: &ProjectCreation,
        program: &str,
        args: &[&str],
        cwd: &Path,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>, String> {
        let service = self.clone();
        let id = op.id.clone();
        self.0
            .runner
            .run(
                program,
                args,
                cwd,
                cancel,
                Arc::new(move |percent| service.update(&id, |o| o.percent = Some(percent))),
            )
            .await
    }
}

fn home() -> PathBuf {
    crate::repos::home_dir()
}
fn expand_home(path: &str) -> String {
    let path = path.trim();
    if path == "~" {
        home().to_string_lossy().into_owned()
    } else if let Some(tail) = path.strip_prefix("~/") {
        home().join(tail).to_string_lossy().into_owned()
    } else {
        path.into()
    }
}
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 100
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(
            "Use lowercase letters, digits, dashes, and underscores; start with a letter or digit."
                .into(),
        );
    }
    Ok(())
}
fn validate_clone_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 100
        || matches!(name, "." | "..")
        || name.starts_with('-')
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    {
        return Err(
            "Choose a folder name using letters, digits, dashes, dots, or underscores.".into(),
        );
    }
    Ok(())
}
fn validate_url(url: &str) -> Result<(), String> {
    if url.len() > 2048
        || url
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '?' || c == '#')
    {
        return Err("Enter an HTTPS or SSH Git repository URL".into());
    }
    if let Some(scp) = url.strip_prefix("git@") {
        if scp
            .split_once(':')
            .is_some_and(|(host, path)| !host.is_empty() && !host.contains('/') && !path.is_empty())
        {
            return Ok(());
        }
    } else if let Ok(parsed) = reqwest::Url::parse(url) {
        if matches!(parsed.scheme(), "https" | "ssh")
            && parsed.host_str().is_some()
            && parsed.path().trim_matches('/').len() > 0
        {
            if parsed.password().is_some()
                || (parsed.scheme() == "https" && !parsed.username().is_empty())
            {
                return Err("Use Git credentials rather than embedding a token in the URL".into());
            }
            return Ok(());
        }
    }
    Err("Enter an HTTPS or SSH Git repository URL".into())
}

fn handoff_marker(root: &Path, id: &str) -> PathBuf {
    root.join(format!(".zuse-creation-{id}"))
}

// Atomic no-replace handoff: never overwrite a directory created while the job was running.
fn rename_new(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let from =
            std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(|_| "Invalid path")?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(|_| "Invalid path")?;
        if unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } != 0
        {
            return Err("Cannot finish project: destination exists or is not writable".into());
        }
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let from =
            std::ffi::CString::new(from.as_os_str().as_bytes()).map_err(|_| "Invalid path")?;
        let to = std::ffi::CString::new(to.as_os_str().as_bytes()).map_err(|_| "Invalid path")?;
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } != 0 {
            return Err("Cannot finish project: destination exists or is not writable".into());
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // Reserve the destination before moving contents; never remove an existing path.
        std::fs::create_dir(to).map_err(|_| "Destination already exists or is not writable")?;
        for entry in std::fs::read_dir(from).map_err(|_| "Cannot read prepared project")? {
            let entry = entry.map_err(|_| "Cannot read prepared project")?;
            std::fs::rename(entry.path(), to.join(entry.file_name()))
                .map_err(|_| "Cannot finish prepared project")?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
trait CreationRunner: Send + Sync {
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
        cancel: &CancellationToken,
        progress: Arc<dyn Fn(u8) + Send + Sync>,
    ) -> Result<Vec<u8>, String>;
}
struct SystemCreationRunner;
#[async_trait::async_trait]
impl CreationRunner for SystemCreationRunner {
    async fn run(
        &self,
        program: &str,
        args: &[&str],
        cwd: &Path,
        cancel: &CancellationToken,
        progress: Arc<dyn Fn(u8) + Send + Sync>,
    ) -> Result<Vec<u8>, String> {
        command(program, args, cwd, cancel, move |p| progress(p)).await
    }
}

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
async fn command(
    program: &str,
    args: &[&str],
    cwd: &Path,
    cancel: &CancellationToken,
    progress: impl Fn(u8) + Send + Sync,
) -> Result<Vec<u8>, String> {
    if cancel.is_cancelled() {
        return Err("Creation cancelled".into());
    }
    let mut cmd = tokio::process::Command::new(program);
    zuse_providers::compose_login_shell_path(&mut cmd);
    cmd.args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("CI", "1")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_HOST", "github.com")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{program} is not installed on this host. Install it and retry.")
        } else {
            format!("Could not start {program} on this host")
        }
    })?;
    let _group = ProcessGroup(child.id());
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let result = tokio::select! {
        _ = cancel.cancelled() => return Err("Creation cancelled".into()),
        result = tokio::time::timeout(Duration::from_secs(if program == "gh" { 20 } else { 1800 }), async {
            tokio::try_join!(child.wait(), read_output(stdout, |_| {}), read_output(stderr, progress))
        }) => result.map_err(|_| format!("{program} timed out. Check the connection and retry."))?,
    }.map_err(|_| format!("Could not read {program} result"))?;
    if !result.0.success() {
        return Err(command_failure(program, &result.2));
    }
    Ok(result.1)
}
fn command_failure(program: &str, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    let hint = if stderr.contains("authentication failed")
        || stderr.contains("publickey")
        || stderr.contains("could not read username")
        || stderr.contains("gh auth login")
    {
        "could not authenticate. Sign in on the selected computer and check its Git credentials, then retry."
    } else if stderr.contains("repository not found") || stderr.contains("does not exist") {
        "could not find the repository. Check its URL and your access, then retry."
    } else if stderr.contains("could not resolve host")
        || stderr.contains("failed to connect")
        || stderr.contains("network is unreachable")
    {
        "could not reach the server. Check the selected computer's network connection, then retry."
    } else if stderr.contains("no space left") || stderr.contains("enospc") {
        "ran out of disk space. Free space on the selected computer, then retry."
    } else if stderr.contains("permission denied") {
        "was denied permission. Choose a writable parent folder and check repository access."
    } else {
        "failed. Check authentication, network access, and required tools on the selected computer, then retry."
    };
    format!("{program} {hint}")
}

async fn read_output(
    mut reader: impl AsyncRead + Unpin,
    progress: impl Fn(u8),
) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buf = [0; 4096];
    let mut line = Vec::new();
    let mut last = None;
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        output.extend_from_slice(&buf[..n.min((1024 * 1024_usize).saturating_sub(output.len()))]);
        for &byte in &buf[..n] {
            if byte == b'\r' || byte == b'\n' {
                if let Some(p) = progress_percent(&line) {
                    if last != Some(p) {
                        progress(p);
                        last = Some(p);
                    }
                }
                line.clear();
            } else if line.len() < 4096 {
                line.push(byte);
            }
        }
    }
    Ok(output)
}
fn progress_percent(line: &[u8]) -> Option<u8> {
    let text = std::str::from_utf8(line).ok()?;
    let prefix = text.split_once('%')?.0;
    prefix
        .split_whitespace()
        .last()?
        .parse::<u8>()
        .ok()
        .filter(|p| *p <= 100)
}

pub(crate) fn decode_github_repositories(
    bytes: &[u8],
    page: u32,
) -> Result<GithubProjectRepositories, String> {
    #[derive(Deserialize)]
    struct Repo {
        full_name: String,
        description: Option<String>,
        ssh_url: String,
        clone_url: String,
        private: bool,
    }
    let repos: Vec<Repo> =
        serde_json::from_slice(bytes).map_err(|_| "GitHub returned an invalid repository list")?;
    Ok(GithubProjectRepositories {
        next_page: (repos.len() == 50).then_some(page + 1),
        repositories: repos
            .into_iter()
            .map(|r| GithubProjectRepository {
                name_with_owner: r.full_name,
                description: r.description,
                ssh_url: r.ssh_url,
                https_url: r.clone_url,
                private: r.private,
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        fail: Mutex<Option<String>>,
        stall: AtomicBool,
        origin: AtomicBool,
    }
    #[async_trait::async_trait]
    impl CreationRunner for FakeRunner {
        async fn run(
            &self,
            program: &str,
            args: &[&str],
            cwd: &Path,
            cancel: &CancellationToken,
            progress: Arc<dyn Fn(u8) + Send + Sync>,
        ) -> Result<Vec<u8>, String> {
            self.calls
                .lock()
                .unwrap()
                .push((program.into(), args.iter().map(|s| (*s).into()).collect()));
            if self.stall.load(Ordering::SeqCst) {
                cancel.cancelled().await;
                return Err("Creation cancelled".into());
            }
            let failure = self.fail.lock().unwrap().clone();
            if failure.as_deref() == Some(program)
                || (failure.as_deref() == Some("identity") && args.first() == Some(&"var"))
            {
                return Err(format!("{program} unavailable"));
            }
            if program == "git"
                && args.first() == Some(&"remote")
                && !self.origin.load(Ordering::SeqCst)
            {
                return Err("No origin".into());
            }
            if program == "gh" {
                self.origin.store(true, Ordering::SeqCst);
            }
            if program == "bunx" {
                tokio::fs::create_dir(cwd.join(args[2])).await.unwrap();
            }
            if program == "git" && args.first() == Some(&"clone") {
                tokio::fs::create_dir(args.last().unwrap()).await.unwrap();
                progress(50);
            }
            Ok(Vec::new())
        }
    }
    fn service(root: &Path, runner: Arc<FakeRunner>) -> ProjectCreationService {
        ProjectCreationService::open_with_runner(root, "host".into(), runner).unwrap()
    }
    fn request(root: &Path, template: ProjectTemplate) -> StartProjectCreation {
        StartProjectCreation {
            id: uuid::Uuid::new_v4().to_string(),
            name: "test-project".into(),
            parent: root.to_string_lossy().into_owned(),
            source: ProjectSource::Create {
                template,
                publish: false,
            },
        }
    }
    async fn settled(service: &ProjectCreationService) -> ProjectCreation {
        let mut rx = service.subscribe();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let op = rx.borrow_and_update().first().cloned().unwrap();
                if op.status != ProjectCreationStatus::Running {
                    return op;
                }
                rx.changed().await.unwrap();
            }
        })
        .await
        .unwrap()
    }
    #[tokio::test]
    async fn creates_each_template_and_remembers_parent() {
        for template in [
            ProjectTemplate::Empty,
            ProjectTemplate::Nextjs,
            ProjectTemplate::Turborepo,
        ] {
            let root = tempfile::tempdir().unwrap();
            let runner = Arc::new(FakeRunner::default());
            let service = service(root.path(), runner.clone());
            let req = request(root.path(), template);
            service.start(req).await.unwrap();
            let op = settled(&service).await;
            assert_eq!(op.status, ProjectCreationStatus::Ready);
            assert!(Path::new(&op.destination).is_dir());
            assert!(!service.blocks(Path::new(&op.destination)));
            assert_eq!(
                service.default_parent().await,
                root.path().to_string_lossy()
            );
            let calls = runner.calls.lock().unwrap();
            assert!(calls.iter().any(|(p, a)| p == "git" && a[0] == "init"));
            if template != ProjectTemplate::Empty {
                assert!(
                    calls
                        .iter()
                        .any(|(p, a)| p == "bunx" && a.contains(&"--skip-install".into()))
                );
                assert!(calls.iter().any(|(p, a)| p == "bun" && a == &["install"]));
            }
        }
    }
    #[tokio::test]
    async fn duplicates_are_idempotent_and_destinations_are_reserved() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.stall.store(true, Ordering::SeqCst);
        let service = service(root.path(), runner.clone());
        let req = request(root.path(), ProjectTemplate::Nextjs);
        let (a, b) = tokio::join!(service.start(req.clone()), service.start(req.clone()));
        assert_eq!(a.unwrap().id, b.unwrap().id);
        assert_eq!(service.list().len(), 1);
        assert!(service.blocks(&root.path().join("test-project/src")));
        let mut other = req.clone();
        other.id = uuid::Uuid::new_v4().to_string();
        assert!(service.start(other).await.is_err());
        service.cancel(&req.id).unwrap();
        assert_eq!(
            settled(&service).await.status,
            ProjectCreationStatus::Cancelled
        );
    }
    #[tokio::test]
    async fn failure_cleans_only_staging_and_retry_succeeds() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("keep"), "important").unwrap();
        let runner = Arc::new(FakeRunner::default());
        *runner.fail.lock().unwrap() = Some("bun".into());
        let service = service(root.path(), runner.clone());
        let req = request(root.path(), ProjectTemplate::Nextjs);
        service.start(req.clone()).await.unwrap();
        assert_eq!(
            settled(&service).await.status,
            ProjectCreationStatus::Failed
        );
        assert!(!root.path().join("test-project").exists());
        assert!(root.path().join("keep").exists());
        assert!(!std::fs::read_dir(root.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".zuse-project-")
        }));
        *runner.fail.lock().unwrap() = None;
        service.retry(&req.id).await.unwrap();
        assert_eq!(settled(&service).await.status, ProjectCreationStatus::Ready);
    }
    #[tokio::test]
    async fn missing_identity_preserves_project_and_warns() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        *runner.fail.lock().unwrap() = Some("identity".into());
        let service = service(root.path(), runner);
        service
            .start(request(root.path(), ProjectTemplate::Empty))
            .await
            .unwrap();
        let op = settled(&service).await;
        assert_eq!(op.status, ProjectCreationStatus::Ready);
        assert!(op.warning.is_some());
        assert!(Path::new(&op.destination).exists());
    }
    #[tokio::test]
    async fn publishing_failure_preserves_files_and_retry_skips_scaffolding() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        *runner.fail.lock().unwrap() = Some("gh".into());
        // Simulate a missing origin, so publishing reaches gh repo create.
        let service = service(root.path(), runner.clone());
        let mut req = request(root.path(), ProjectTemplate::Empty);
        req.source = ProjectSource::Create {
            template: ProjectTemplate::Empty,
            publish: true,
        };
        service.start(req.clone()).await.unwrap();
        let op = settled(&service).await;
        assert_eq!(op.status, ProjectCreationStatus::Ready);
        assert!(Path::new(&op.destination).is_dir());
        assert!(op.publish_error.is_some());
        assert!(!op.blocks_execution());
        let calls_before = runner.calls.lock().unwrap().len();
        *runner.fail.lock().unwrap() = None;
        service.retry(&req.id).await.unwrap();
        let recovered = settled(&service).await;
        assert!(recovered.publish_error.is_none());
        assert!(
            !runner.calls.lock().unwrap()[calls_before..]
                .iter()
                .any(|(p, a)| p == "git" && a[0] == "init")
        );
    }
    #[tokio::test]
    async fn restart_marks_unfinished_operations_interrupted() {
        let root = tempfile::tempdir().unwrap();
        let req = request(root.path(), ProjectTemplate::Empty);
        let journal = Store {
            operations: BTreeMap::from([(
                req.id.clone(),
                ProjectCreation {
                    id: req.id.clone(),
                    device_id: "host".into(),
                    destination: root
                        .path()
                        .join("test-project")
                        .to_string_lossy()
                        .into_owned(),
                    request: req,
                    stage: "Cloning".into(),
                    status: ProjectCreationStatus::Running,
                    percent: Some(20),
                    files_ready: false,
                    error: None,
                    warning: None,
                    publish_error: None,
                },
            )]),
            parent: None,
        };
        std::fs::write(
            root.path().join("project-creations.json"),
            serde_json::to_vec(&journal).unwrap(),
        )
        .unwrap();
        let service = service(root.path(), Arc::new(FakeRunner::default()));
        assert_eq!(service.list()[0].status, ProjectCreationStatus::Interrupted);
        assert!(service.blocks(&root.path().join("test-project")));
    }
    #[tokio::test]
    async fn rpc_registers_pending_project_streams_progress_and_rejects_early_runs() {
        let root = tempfile::tempdir().unwrap();
        let core = crate::EngineCore::assemble(
            &root.path().join("data"),
            Arc::new(crate::HarnessRegistry::new()),
            zuse_protocol::HarnessId::Mock,
            None,
        )
        .unwrap();
        let jobs = root.path().join("jobs");
        std::fs::create_dir(&jobs).unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.stall.store(true, Ordering::SeqCst);
        let service =
            ProjectCreationService::open_with_runner(&jobs, core.device_id.clone(), runner)
                .unwrap();
        let rpc = core
            .rpc_service()
            .as_ref()
            .clone()
            .with_project_creation(service.clone());
        let client = zuse_rpc::memory_client(Arc::new(rpc));
        let request = request(root.path(), ProjectTemplate::Nextjs);
        let params = serde_json::to_value(&request).unwrap();
        let op: ProjectCreation = client.call_as(START, params.clone()).await.unwrap();
        let again: ProjectCreation = client.call_as(START, params).await.unwrap();
        assert_eq!(again.id, op.id);
        assert!(core.workspace.space(&op.id).unwrap().is_some());
        let mut stream = client
            .subscribe_scoped(WATCH, serde_json::json!({}))
            .await
            .unwrap();
        let first = tokio::time::timeout(Duration::from_secs(2), stream.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first[0]["id"], op.id);
        let result = client.call(zuse_rpc::methods::QUEUE_COMMAND,serde_json::json!({"chatId":"not-yet-created","command":{"kind":"run","messageId":"first","request":{"prompt":"hello","cwd":op.destination,"sandbox":"workspace-write"}}})).await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Project files are not ready")
        );
        client
            .call(CANCEL, serde_json::json!({"id":op.id}))
            .await
            .unwrap();
        assert_eq!(
            settled(&service).await.status,
            ProjectCreationStatus::Cancelled
        );
        core.shutdown().await;
    }
    #[tokio::test]
    async fn clone_progress_finishes_and_cancellation_can_retry() {
        let root = tempfile::tempdir().unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.stall.store(true, Ordering::SeqCst);
        let service = service(root.path(), runner.clone());
        let mut req = request(root.path(), ProjectTemplate::Empty);
        req.source = ProjectSource::Clone {
            url: "https://github.com/org/repo.git".into(),
        };
        service.start(req.clone()).await.unwrap();
        service.cancel(&req.id).unwrap();
        assert_eq!(
            settled(&service).await.status,
            ProjectCreationStatus::Cancelled
        );
        runner.stall.store(false, Ordering::SeqCst);
        service.retry(&req.id).await.unwrap();
        assert_eq!(settled(&service).await.status, ProjectCreationStatus::Ready);
        assert!(runner.calls.lock().unwrap().iter().any(|(p, a)| p == "git"
            && a.starts_with(&["clone".into(), "--progress".into(), "--".into()])));
    }
    #[tokio::test]
    async fn real_empty_project_has_a_git_directory_and_readme() {
        let root = tempfile::tempdir().unwrap();
        let service = ProjectCreationService::open(root.path(), "host".into()).unwrap();
        service
            .start(request(root.path(), ProjectTemplate::Empty))
            .await
            .unwrap();
        let op = settled(&service).await;
        assert_eq!(op.status, ProjectCreationStatus::Ready);
        assert!(Path::new(&op.destination).join(".git").is_dir());
        assert!(Path::new(&op.destination).join("README.md").is_file());
        let output = command(
            "zuse-intentionally-missing-command",
            &[],
            root.path(),
            &CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();
        assert!(output.contains("not installed"));
    }
    #[tokio::test]
    async fn recovery_requires_proof_of_a_completed_handoff() {
        for owned in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let runner = Arc::new(FakeRunner::default());
            runner.stall.store(true, Ordering::SeqCst);
            let active = service(root.path(), runner);
            let req = request(root.path(), ProjectTemplate::Nextjs);
            let op = active.start(req).await.unwrap();
            active.cancel(&op.id).unwrap();
            settled(&active).await;
            std::fs::create_dir(&op.destination).unwrap();
            if owned {
                std::fs::write(handoff_marker(Path::new(&op.destination), &op.id), &op.id).unwrap();
            }
            active.update(&op.id, |o| {
                o.status = ProjectCreationStatus::Running;
                o.stage = "Finishing".into();
            });
            drop(active);
            let recovered = service(root.path(), Arc::new(FakeRunner::default()));
            assert_eq!(recovered.list()[0].files_ready, owned);
            assert_eq!(
                recovered.list()[0].status,
                if owned {
                    ProjectCreationStatus::Ready
                } else {
                    ProjectCreationStatus::Interrupted
                }
            );
            assert!(Path::new(&op.destination).exists());
        }
    }
    #[tokio::test]
    async fn rejects_existing_destinations_and_unsafe_inputs() {
        let root = tempfile::tempdir().unwrap();
        let service = service(root.path(), Arc::new(FakeRunner::default()));
        std::fs::create_dir(root.path().join("test-project")).unwrap();
        assert!(
            service
                .start(request(root.path(), ProjectTemplate::Empty))
                .await
                .is_err()
        );
        for name in ["../escape", "--flag", "", "UPPER", "a/b", "a b"] {
            assert!(validate_name(name).is_err());
        }
        for url in [
            "--upload-pack=evil",
            "file:///tmp/repo",
            "https://token@github.com/a/b",
            "https://github.com/a/b?token=secret",
            "git@host:a\nb",
        ] {
            assert!(validate_url(url).is_err());
        }
    }
    #[test]
    fn progress_and_repository_pages_are_bounded() {
        assert_eq!(
            progress_percent(b"Receiving objects:  42% (4/10)"),
            Some(42)
        );
        assert_eq!(progress_percent(b"Done 999%"), None);
        let error = command_failure(
            "git",
            b"Authentication failed for https://secret@example.test/repo",
        );
        assert!(error.contains("authenticate"));
        assert!(!error.contains("secret"));
        let page = decode_github_repositories(br#"[{"full_name":"org/private","description":null,"ssh_url":"git@github.com:org/private.git","clone_url":"https://github.com/org/private.git","private":true}]"#,1).unwrap();
        assert_eq!(page.repositories[0].name_with_owner, "org/private");
        assert!(page.next_page.is_none());
        assert!(decode_github_repositories(b"not json", 1).is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_stops_the_process_group_and_output_is_capped() {
        let root = tempfile::tempdir().unwrap();
        let token = CancellationToken::new();
        let cancel = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        });
        let result = command(
            "sh",
            &["-c", "sleep 30 & wait"],
            root.path(),
            &token,
            |_| {},
        )
        .await;
        assert_eq!(result.unwrap_err(), "Creation cancelled");
        let data = vec![b'x'; 2 * 1024 * 1024];
        let output = read_output(data.as_slice(), |_| {}).await.unwrap();
        assert_eq!(output.len(), 1024 * 1024);
    }
}
