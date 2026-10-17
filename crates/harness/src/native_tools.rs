//! Native coding tools share checkout containment, permissions and process ownership.
mod files;
pub mod permission;
mod schema;
use crate::{
    processes::{ProcessOptions, ProcessStart, Processes},
    *,
};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use files::{CheckoutFiles, check};
use permission::{RuntimeMode, Verdict};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{OwnedRwLockReadGuard, RwLock};

pub struct ApprovalRequest {
    pub tool: String,
    pub input: Value,
    pub path: Option<PathBuf>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PlanStep {
    pub step: String,
    pub status: String,
}
#[async_trait]
pub trait NativeCallbacks: Send + Sync {
    fn approvals(&self) -> bool {
        false
    }
    fn questions(&self) -> bool {
        false
    }
    fn plans(&self) -> bool {
        false
    }
    async fn approve(&self, _: ApprovalRequest, _: ToolContext) -> Result<bool, Error> {
        Ok(false)
    }
    async fn question(
        &self,
        _: String,
        _: Option<Vec<String>>,
        _: ToolContext,
    ) -> Result<String, Error> {
        Err(Error::Tool("Questions are unavailable".into()))
    }
    async fn plan(&self, _: Vec<PlanStep>, _: ToolContext) -> Result<(), Error> {
        Err(Error::Tool("Plans are unavailable".into()))
    }
}
pub struct NativeOptions {
    pub cwd: PathBuf,
    pub lock_directory: PathBuf,
    pub processes: ProcessOptions,
    pub runtime_mode: Arc<dyn Fn() -> RuntimeMode + Send + Sync>,
    pub callbacks: Option<Arc<dyn NativeCallbacks>>,
    pub ripgrep: PathBuf,
}
pub struct NativeTools {
    files: Arc<CheckoutFiles>,
    pub processes: Processes,
    definitions: Tools,
    directory: PathBuf,
    runtime_mode: Arc<dyn Fn() -> RuntimeMode + Send + Sync>,
    callbacks: Option<Arc<dyn NativeCallbacks>>,
    ripgrep: PathBuf,
    lifecycle: Arc<RwLock<()>>,
    shutdown: CancellationToken,
    closed: AtomicBool,
}
struct CancelOnDrop {
    token: CancellationToken,
    armed: bool,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.token.cancel();
        }
    }
}
type Lease = Arc<OwnedRwLockReadGuard<()>>;
fn text<'a>(input: &'a Value, name: &str) -> &'a str {
    input[name].as_str().expect("validated native tool string")
}
fn number(input: &Value, name: &str) -> u64 {
    input[name].as_u64().expect("validated native tool integer")
}
fn error(message: &str) -> Error {
    Error::Tool(message.into())
}
impl NativeTools {
    /// Call inside Engine::open_native_with, while the root lease is held.
    pub async fn open(options: NativeOptions) -> Result<Self, Error> {
        let files = Arc::new(CheckoutFiles::open(&options.cwd, options.lock_directory)?);
        let directory = options.processes.directory.clone();
        let definitions = schema::definitions(
            options.callbacks.as_ref().is_some_and(|c| c.questions()),
            options.callbacks.as_ref().is_some_and(|c| c.plans()),
        );
        let processes = Processes::open(options.processes).await?;
        Ok(Self {
            files,
            processes,
            definitions,
            directory,
            runtime_mode: options.runtime_mode,
            callbacks: options.callbacks,
            ripgrep: options.ripgrep,
            lifecycle: Arc::new(RwLock::new(())),
            shutdown: CancellationToken::new(),
            closed: AtomicBool::new(false),
        })
    }
    pub fn definitions(&self) -> Tools {
        self.definitions.clone()
    }
    pub fn checkout(&self) -> &Path {
        self.files.root()
    }
    async fn filesystem<T: Send + 'static>(
        &self,
        lease: Lease,
        cancel: CancellationToken,
        operation: impl FnOnce(&CheckoutFiles) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            check(&cancel)?;
            operation(&files)
        })
        .await
        .map_err(|_| error("Native file operation failed"))?
    }
    pub async fn execute(
        &self,
        name: &str,
        mut input: Value,
        context: ToolContext,
    ) -> Result<ToolOutput, Error> {
        let lease = Arc::new(self.lifecycle.clone().read_owned().await);
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Interrupted);
        }
        if !self.definitions.contains_key(name) {
            return Err(error("Unknown or unavailable native tool"));
        }
        schema::validate(name, &mut input)?;
        check(&context.cancel)?;
        let operation = context.cancel.child_token();
        let mut guard = CancelOnDrop {
            token: operation.clone(),
            armed: true,
        };
        let parent_cancel = context.cancel.clone();
        let mut context = context;
        context.cancel = operation;
        let result = tokio::select! {biased;_=self.shutdown.cancelled()=>Err(Error::Interrupted),_=parent_cancel.cancelled()=>Err(Error::Interrupted),result=self.execute_validated(name,input,context,lease)=>result};
        // Successful exec_command may intentionally leave an owned background
        // process. Its token still inherits interruption from the agent runner.
        if result.is_ok() {
            guard.armed = false;
        }
        result
    }
    async fn execute_validated(
        &self,
        name: &str,
        input: Value,
        context: ToolContext,
        lease: Lease,
    ) -> Result<ToolOutput, Error> {
        let path = if let Some(path) = input
            .get("path")
            .or_else(|| input.get("cwd"))
            .and_then(Value::as_str)
        {
            let path = path.to_owned();
            Some(
                self.filesystem(lease.clone(), context.cancel.clone(), move |files| {
                    files.resolve(&path)
                })
                .await?,
            )
        } else {
            None
        };
        let sensitive = path.as_ref().is_some_and(|p| permission::sensitive(p))
            || input
                .get("path")
                .or_else(|| input.get("cwd"))
                .and_then(Value::as_str)
                .is_some_and(|p| permission::sensitive(Path::new(p)));
        let mode = if context.read_only {
            PermissionMode::Plan
        } else {
            context.permission_mode
        };
        let can_prompt = self.callbacks.as_ref().is_some_and(|c| c.approvals());
        match permission::decide(
            (self.runtime_mode)(),
            mode,
            self.definitions[name].category,
            sensitive,
            can_prompt,
        ) {
            Verdict::Deny => return Err(error("Tool permission denied")),
            Verdict::Prompt => {
                let allowed = self
                    .callbacks
                    .as_ref()
                    .ok_or_else(|| error("Tool approval is unavailable"))?
                    .approve(
                        ApprovalRequest {
                            tool: name.into(),
                            input: input.clone(),
                            path: path.clone(),
                        },
                        context.clone(),
                    )
                    .await?;
                if !allowed {
                    return Err(error("Tool permission denied"));
                }
            }
            Verdict::Allow => {}
        }
        check(&context.cancel)?;
        match name {
            "read_file" => {
                let path = path.unwrap();
                let offset = number(&input, "offset");
                let limit = number(&input, "limit") as usize;
                let value=self.filesystem(lease.clone(),context.cancel.clone(),move |files|{
                    let data=files.read(&path,true)?;let content=std::str::from_utf8(&data).map_err(|_|error("Expected UTF-8 text"))?;
                    let total=content.split('\n').count();let start=(offset.min(total as u64)) as usize;
                    Ok(json!({"path":path,"hash":files::digest(&data),"total_lines":total,"offset":offset,"content":content.split('\n').skip(start).take(limit).collect::<Vec<_>>().join("\n"),"has_more":start.saturating_add(limit)<total}))
                }).await?;
                self.bounded(value, lease, context.cancel).await
            }
            "read_image" => {
                let path = path.unwrap();
                self.filesystem(lease, context.cancel, move |files| {
                    let data = files.read(&path, false)?;
                    let media_type = if data.starts_with(b"\x89PNG\r\n\x1a\n") {
                        "image/png"
                    } else if data.starts_with(b"\xff\xd8\xff") {
                        "image/jpeg"
                    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
                        "image/gif"
                    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
                        "image/webp"
                    } else {
                        return Err(error("Unsupported image format"));
                    };
                    Ok(ToolOutput::Content {
                        r#type: "content".into(),
                        value: vec![ImageContent {
                            r#type: "file".into(),
                            media_type: media_type.into(),
                            data: ImageData {
                                r#type: "data".into(),
                                data: STANDARD.encode(data),
                            },
                        }],
                    })
                })
                .await
            }
            "list_directory" => {
                let path = path.unwrap();
                let value = self
                    .filesystem(lease.clone(), context.cancel.clone(), move |files| {
                        files.list(&path)
                    })
                    .await?;
                self.bounded(value, lease, context.cancel).await
            }
            "write_file" | "edit_file" | "multi_edit" => {
                let path = path.unwrap();
                let name = name.to_owned();
                let cancel = context.cancel.clone();
                let value = self
                    .filesystem(lease, context.cancel, move |files| {
                        files.change(
                            &path,
                            input["expected_hash"].as_str(),
                            |current| match name.as_str() {
                                "write_file" => Ok(text(&input, "content").into()),
                                "edit_file" => files::replace(current, &input),
                                _ => {
                                    let mut next = current.to_owned();
                                    for edit in input["edits"].as_array().unwrap() {
                                        next = files::replace(&next, edit)?;
                                    }
                                    Ok(next)
                                }
                            },
                            &cancel,
                        )
                    })
                    .await?;
                Ok(ToolOutput::Text(value.to_string()))
            }
            "exec_command" => {
                let process = self
                    .processes
                    .start(
                        ProcessStart {
                            file: "/bin/bash".into(),
                            args: vec!["-lc".into(), text(&input, "command").into()],
                            cwd: path.unwrap(),
                            label: input["label"].as_str().unwrap_or("Bash command").into(),
                            timeout_ms: number(&input, "timeout_ms"),
                            notify_on_output: input["notify_on_output"].as_str().map(str::to_owned),
                            notify: true,
                        },
                        &context,
                    )
                    .await?;
                self.processes
                    .wait(
                        &process.id,
                        &context.agent_id,
                        number(&input, "yield_time_ms"),
                        &context.cancel,
                    )
                    .await?;
                self.process_output(&process.id, &context.agent_id, 0, 16000)
                    .await
            }
            "grep" | "glob" => {
                let pattern = text(&input, "pattern");
                let args = if name == "glob" {
                    vec!["--files".into(), "--glob".into(), pattern.into()]
                } else {
                    let mut args = vec![
                        "--line-number".into(),
                        "--no-heading".into(),
                        "--color".into(),
                        "never".into(),
                        "--max-columns".into(),
                        "1000".into(),
                    ];
                    if input["literal"] == true {
                        args.push("--fixed-strings".into());
                    }
                    if input["ignore_case"] == true {
                        args.push("--ignore-case".into());
                    }
                    if let Some(glob) = input["glob"].as_str() {
                        args.extend(["--glob".into(), glob.into()]);
                    }
                    args.extend(["--".into(), pattern.into(), ".".into()]);
                    args
                };
                let process = self
                    .processes
                    .start(
                        ProcessStart {
                            file: self.ripgrep.clone(),
                            args,
                            cwd: path.unwrap(),
                            label: name.into(),
                            timeout_ms: 30000,
                            notify_on_output: None,
                            notify: false,
                        },
                        &context,
                    )
                    .await?;
                self.processes
                    .wait(&process.id, &context.agent_id, 30000, &context.cancel)
                    .await?;
                self.process_output(&process.id, &context.agent_id, 0, 16000)
                    .await
            }
            "process_output" => {
                self.process_output(
                    text(&input, "process_id"),
                    &context.agent_id,
                    number(&input, "cursor"),
                    number(&input, "max_bytes") as usize,
                )
                .await
            }
            "wait_process" => {
                let id = text(&input, "process_id");
                self.processes
                    .wait(
                        id,
                        &context.agent_id,
                        number(&input, "timeout_ms"),
                        &context.cancel,
                    )
                    .await?;
                self.process_output(id, &context.agent_id, 0, 16000).await
            }
            "write_stdin" => {
                self.processes
                    .write(
                        text(&input, "process_id"),
                        &context.agent_id,
                        text(&input, "data"),
                        input["eof"] == true,
                        &context.cancel,
                    )
                    .await?;
                Ok(ToolOutput::Text("Input sent".into()))
            }
            "stop_process" => {
                self.processes
                    .stop(text(&input, "process_id"), &context.agent_id)
                    .await?;
                Ok(ToolOutput::Text("Process stopped".into()))
            }
            "list_processes" => {
                self.bounded(
                    serde_json::to_value(self.processes.list(&context.agent_id))
                        .map_err(|_| error("Could not encode process list"))?,
                    lease,
                    context.cancel,
                )
                .await
            }
            "update_plan" => {
                let steps: Vec<PlanStep> = serde_json::from_value(input["steps"].clone())
                    .map_err(|_| error("Invalid plan"))?;
                if steps.iter().filter(|s| s.status == "in_progress").count() > 1 {
                    return Err(error("Only one plan step can be in progress"));
                }
                self.callbacks
                    .as_ref()
                    .ok_or_else(|| error("Plans are unavailable"))?
                    .plan(steps, context)
                    .await?;
                Ok(ToolOutput::Text("Plan updated".into()))
            }
            "request_user_input" => {
                let options = input["options"].as_array().map(|options| {
                    options
                        .iter()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect()
                });
                let answer = self
                    .callbacks
                    .as_ref()
                    .ok_or_else(|| error("Questions are unavailable"))?
                    .question(text(&input, "question").into(), options, context)
                    .await?;
                Ok(ToolOutput::Text(answer))
            }
            _ => Err(error("Unknown native tool")),
        }
    }
    async fn process_output(
        &self,
        id: &str,
        agent: &str,
        cursor: u64,
        max: usize,
    ) -> Result<ToolOutput, Error> {
        let value = self.processes.output(id, agent, cursor, max).await?;
        Ok(ToolOutput::Text(
            serde_json::to_string(&value).map_err(|_| error("Could not encode process output"))?,
        ))
    }
    async fn bounded(
        &self,
        value: Value,
        lease: Lease,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, Error> {
        let full =
            serde_json::to_string(&value).map_err(|_| error("Could not encode tool output"))?;
        if full.len() <= 32000 {
            return Ok(ToolOutput::Text(full));
        }
        let path = self
            .directory
            .join(format!("{}.result.json", uuid::Uuid::new_v4()));
        self.filesystem(lease, cancel, move |_| {
            use std::{io::Write, os::unix::fs::OpenOptionsExt};
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|_| Error::Storage)?;
            file.write_all(full.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|_| Error::Storage)?;
            let mut end = 12000.min(full.len());
            while !full.is_char_boundary(end) {
                end -= 1;
            }
            let mut result = json!({"preview":&full[..end],"truncated":true,"artifact":path});
            for key in ["path", "hash", "total_lines", "offset", "has_more"] {
                if let Some(field) = value.get(key) {
                    result[key] = field.clone();
                }
            }
            Ok(ToolOutput::Text(result.to_string()))
        })
        .await
    }
    pub async fn interrupt_agents(&self, ids: &[String]) {
        self.processes.interrupt_agents(ids).await;
    }
    pub async fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.shutdown.cancel();
        let _closing = self.lifecycle.write().await;
        self.processes.close().await;
    }
}
impl Drop for NativeTools {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

struct ToolHost {
    native: Arc<NativeTools>,
    application: Arc<dyn Host>,
    definitions: Tools,
}
impl NativeTools {
    /// Combine the native tools with application model transport, events and
    /// orchestration tools. Native names cannot be silently replaced by app tools.
    pub fn attach(self: Arc<Self>, application: Arc<dyn Host>) -> Result<Arc<dyn Host>, Error> {
        let mut definitions = self.definitions();
        for (name, definition) in application.tools() {
            if definitions.insert(name, definition).is_some() {
                return Err(error(
                    "An application tool conflicts with a native coding tool",
                ));
            }
        }
        Ok(Arc::new(ToolHost {
            native: self,
            application,
            definitions,
        }))
    }
}
#[async_trait]
impl Host for ToolHost {
    fn tools(&self) -> Tools {
        self.definitions.clone()
    }
    async fn load(&self) -> Result<Option<Checkpoint>, Error> {
        self.application.load().await
    }
    async fn save(&self, state: &Checkpoint) -> Result<(), Error> {
        self.application.save(state).await
    }
    async fn instructions(&self) -> Result<String, Error> {
        self.application.instructions().await
    }
    async fn model(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        self.application.model(input).await
    }
    async fn execute(
        &self,
        name: &str,
        input: Value,
        context: ToolContext,
    ) -> Result<ToolOutput, Error> {
        if self.native.definitions.contains_key(name) {
            self.native.execute(name, input, context).await
        } else {
            self.application.execute(name, input, context).await
        }
    }
    fn notify(&self, event: Event) {
        self.application.notify(event);
    }
    async fn interrupt_agents(&self, ids: &[String]) -> Result<(), Error> {
        self.native.interrupt_agents(ids).await;
        self.application.interrupt_agents(ids).await
    }
    async fn close(&self) -> Result<(), Error> {
        self.native.close().await;
        self.application.close().await
    }
}
