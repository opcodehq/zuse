use super::{Models, failure};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use zuse_harness::{
    native_tools::{
        ApprovalRequest, NativeCallbacks, PlanStep,
        permission::{self, RuntimeMode, Verdict},
    },
    processes::{ProcessNotice, ProcessNotifier},
    *,
};
use zuse_protocol::{RunRequest, SandboxLevel, UserInputAnswer, UserInputQuestion};
use zuse_providers::{HarnessError, RunControls};
pub(super) fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
pub(super) type Input =
    Arc<dyn Fn(Vec<UserInputQuestion>) -> oneshot::Receiver<Vec<UserInputAnswer>> + Send + Sync>;
pub(super) struct Callbacks {
    pub input: Input,
}
impl Callbacks {
    async fn ask(
        &self,
        question: UserInputQuestion,
        context: &ToolContext,
    ) -> Result<Vec<String>, Error> {
        let id = question.id.clone();
        let reply = (self.input)(vec![question]);
        tokio::select! {biased;_=context.cancel.cancelled()=>Err(Error::Interrupted),reply=reply=>{
            reply.map_err(|_| Error::Interrupted)?.into_iter().find(|answer| answer.question_id == id).map(|answer| answer.labels).ok_or(Error::Interrupted)
        }}
    }
}
#[async_trait]
impl NativeCallbacks for Callbacks {
    fn approvals(&self) -> bool {
        true
    }
    fn questions(&self) -> bool {
        true
    }
    fn plans(&self) -> bool {
        true
    }
    async fn approve(&self, request: ApprovalRequest, context: ToolContext) -> Result<bool, Error> {
        let question = UserInputQuestion {
            id: format!("{}:{}", context.agent_id, context.call_id),
            header: "Permission".into(),
            question: format!(
                "Allow {}?\n{}",
                request.tool,
                request
                    .input
                    .to_string()
                    .chars()
                    .take(8000)
                    .collect::<String>()
            ),
            options: vec!["Allow once".into(), "Deny".into()],
            multi_select: false,
            prefill: None,
            multiline: false,
        };
        Ok(self.ask(question, &context).await? == ["Allow once"])
    }
    async fn question(
        &self,
        question: String,
        options: Option<Vec<String>>,
        context: ToolContext,
    ) -> Result<String, Error> {
        self.ask(
            UserInputQuestion {
                id: format!("{}:{}", context.agent_id, context.call_id),
                header: "Question".into(),
                question,
                options: options.unwrap_or_default(),
                multi_select: false,
                prefill: None,
                multiline: true,
            },
            &context,
        )
        .await
        .map(|labels| labels.join("\n"))
    }
    async fn secret(&self, input: Value, context: ToolContext) -> Result<String, Error> {
        crate::secret_broker::execute(input, context.clone(), |question| {
            self.ask(question, &context)
        })
        .await
    }
    async fn plan(&self, _: Vec<PlanStep>, _: ToolContext) -> Result<(), Error> {
        Ok(())
    }
}
pub(super) struct Notices(pub OnceLock<WeakEngine>);
#[async_trait]
impl ProcessNotifier for Notices {
    async fn notify(&self, notice: ProcessNotice) -> Result<(), Error> {
        let engine = self
            .0
            .get()
            .and_then(WeakEngine::upgrade)
            .ok_or(Error::Storage)?;
        engine
            .notify_process(
                notice.id,
                &notice.agent_id,
                format!("{}: {} ({:?})", notice.kind, notice.label, notice.status),
            )
            .await
    }
}
pub(super) fn mode(request: &RunRequest) -> RuntimeMode {
    if request.auto_approves_tools() {
        RuntimeMode::FullAccess
    } else {
        RuntimeMode::ApprovalRequired
    }
}
pub(super) fn permission_mode(request: &RunRequest) -> PermissionMode {
    if request.sandbox == SandboxLevel::ReadOnly {
        PermissionMode::Plan
    } else {
        PermissionMode::Default
    }
}
fn category(name: &str) -> ToolCategory {
    if matches!(
        name,
        "whoami"
            | "list_devices"
            | "list_projects"
            | "list_harnesses"
            | "list_models"
            | "list_chats"
            | "get_chat"
            | "wait_for_turn"
            | "plugins_list"
            | "plugins_search"
            | "plugins_schema"
    ) {
        ToolCategory::Read
    } else {
        ToolCategory::Delegate
    }
}
pub(super) struct Application {
    pub models: Arc<dyn Models>,
    pub events: mpsc::Sender<Event>,
    pub stop: CancellationToken,
    pub overflow: AtomicBool,
    pub callbacks: Arc<Callbacks>,
    pub mode: Arc<Mutex<RuntimeMode>>,
    pub tools: Option<Arc<zuse_mcp::Tools>>,
    pub cwd: PathBuf,
    pub uploads: crate::Uploads,
}
#[async_trait]
impl Host for Application {
    fn tools(&self) -> Tools {
        let mut definitions = self
            .tools
            .as_ref()
            .map(|tools| {
                tools
                    .list()
                    .into_iter()
                    .map(|tool| {
                        (
                            format!("zuse_{}", tool.name),
                            ToolDefinition {
                                description: tool.description.into(),
                                parameters: tool.input_schema,
                                category: category(tool.name),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_else(BTreeMap::new);
        definitions.insert("zuse_read_upload".into(), ToolDefinition { description: "Read a file uploaded to this profile. Text is decoded; binary data is a bounded base64 chunk. Continue with nextOffset when done is false.".into(), parameters: json!({"type":"object","properties":{"path":{"type":"string"},"offset":{"type":"integer","minimum":0}},"required":["path"],"additionalProperties":false}), category: ToolCategory::Read });
        definitions
    }
    async fn load(&self) -> Result<Option<Checkpoint>, Error> {
        Err(Error::Storage)
    }
    async fn save(&self, _: &Checkpoint) -> Result<(), Error> {
        Err(Error::Storage)
    }
    async fn instructions(&self) -> Result<String, Error> {
        let mut result = String::new();
        for name in ["AGENTS.md", "CLAUDE.md"] {
            let path = self.cwd.join(name);
            let mut options = tokio::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            let Ok(file) = options.open(path).await else {
                continue;
            };
            if !file.metadata().await.map_err(|_| Error::Storage)?.is_file() {
                continue;
            }
            use tokio::io::AsyncReadExt;
            let mut text = String::new();
            file.take(128 * 1024)
                .read_to_string(&mut text)
                .await
                .map_err(|_| Error::Storage)?;
            result.push_str(&format!("\nRepository instructions ({name}):\n{text}\n"));
        }
        Ok(result)
    }
    async fn model(&self, input: ModelInput) -> Result<ModelOutput, Error> {
        self.models.request(input).await
    }
    async fn execute(
        &self,
        name: &str,
        input: Value,
        context: ToolContext,
    ) -> Result<ToolOutput, Error> {
        let name = name
            .strip_prefix("zuse_")
            .ok_or_else(|| Error::Tool("Unknown application tool".into()))?;
        if name == "read_upload" {
            let path = input["path"]
                .as_str()
                .filter(|path| !path.is_empty())
                .ok_or_else(|| Error::Tool("Missing upload path".into()))?
                .to_owned();
            let offset = input
                .get("offset")
                .map(Value::as_u64)
                .unwrap_or(Some(0))
                .ok_or_else(|| Error::Tool("Invalid upload offset".into()))?;
            if context.cancel.is_cancelled() {
                return Err(Error::Interrupted);
            }
            let uploads = self.uploads.clone();
            let chunk = tokio::task::spawn_blocking(move || uploads.read_chunk(&path, offset, &[]))
                .await
                .map_err(|_| Error::Storage)?
                .map_err(|error| Error::Tool(error.to_string()))?;
            if context.cancel.is_cancelled() {
                return Err(Error::Interrupted);
            }
            let bytes = STANDARD.decode(&chunk.data).map_err(|_| Error::Storage)?;
            let text = std::str::from_utf8(&bytes)
                .ok()
                .filter(|_| !bytes.contains(&0));
            let value = match text {
                Some(text) => {
                    json!({"name":chunk.name,"mimeType":chunk.mime_type,"encoding":"utf8","text":text,"nextOffset":chunk.next_offset,"done":chunk.done})
                }
                None => {
                    json!({"name":chunk.name,"mimeType":chunk.mime_type,"encoding":"base64","data":chunk.data,"nextOffset":chunk.next_offset,"done":chunk.done})
                }
            };
            return Ok(ToolOutput::Text(value.to_string()));
        }
        let tools = self
            .tools
            .as_ref()
            .filter(|tools| tools.has(name))
            .ok_or_else(|| Error::Tool("Unavailable application tool".into()))?;
        let mode = if context.read_only {
            PermissionMode::Plan
        } else {
            context.permission_mode
        };
        let runtime_mode = *lock(&self.mode);
        match permission::decide(runtime_mode, mode, category(name), false, true) {
            Verdict::Deny => {
                return Err(Error::Tool(
                    "Application tool is not allowed in read-only mode".into(),
                ));
            }
            Verdict::Prompt => {
                if !self
                    .callbacks
                    .approve(
                        ApprovalRequest {
                            tool: name.into(),
                            input: input.clone(),
                            path: None,
                        },
                        context.clone(),
                    )
                    .await?
                {
                    return Err(Error::Tool("Application tool permission denied".into()));
                }
            }
            Verdict::Allow => {}
        }
        tokio::select! {biased;_=context.cancel.cancelled()=>Err(Error::Interrupted),_=self.stop.cancelled()=>Err(Error::Interrupted),result=tools.call(name,input)=>result.map(|value| ToolOutput::Text(value.to_string())).map_err(Error::Tool)}
    }
    fn notify(&self, event: Event) {
        if let Err(error) = self.events.try_send(event) {
            if matches!(error, mpsc::error::TrySendError::Full(_)) {
                self.overflow.store(true, Ordering::Release);
            }
            self.stop.cancel();
        }
    }
}
pub(super) fn tools(
    request: &RunRequest,
    device_id: &str,
) -> Result<Option<Arc<zuse_mcp::Tools>>, HarnessError> {
    let Some(mcp) = &request.mcp else {
        return Ok(None);
    };
    let port = mcp
        .env
        .get("ZUSE_IPC_PORT")
        .and_then(|port| port.parse::<u16>().ok())
        .filter(|port| *port > 0)
        .ok_or_else(|| failure("Missing engine tool endpoint"))?;
    if mcp.name != "zuse"
        || mcp
            .env
            .get("ZUSE_DEVICE_ID")
            .is_none_or(|id| id != device_id)
    {
        return Err(failure("Invalid engine tool identity"));
    }
    let chat_id = mcp
        .env
        .get("ZUSE_CHAT_ID")
        .filter(|id| !id.is_empty())
        .cloned()
        .ok_or_else(|| failure("Missing originating chat"))?;
    Ok(Some(Arc::new(zuse_mcp::Tools::new(Arc::new(
        zuse_mcp::Zuse::new(
            format!("ws://127.0.0.1:{port}"),
            zuse_mcp::Origin {
                chat_id: Some(chat_id),
                device_id: Some(device_id.into()),
            },
        ),
    )))))
}
pub(super) fn callbacks(controls: &mut RunControls) -> Arc<Callbacks> {
    let input = std::mem::replace(
        &mut controls.request_input,
        Box::new(|_| {
            let (_, rx) = oneshot::channel();
            rx
        }),
    );
    Arc::new(Callbacks {
        input: Arc::from(input),
    })
}
