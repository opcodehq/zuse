//! Browser-compatible GPUI views. Native process/filesystem access stays behind RPC.

pub mod assets;
pub mod input;
mod shell;
mod workspace;

use futures::{
    StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use std::collections::HashMap;
use zuse_model::{SessionMessageEntry, TranscriptUpdate, apply_transcript_frame};
use zuse_protocol::methods;

actions!(zuse_client, [Submit, SaveFile]);

pub struct Transport {
    pub requests: UnboundedSender<Value>,
    pub events: UnboundedReceiver<Value>,
}

#[derive(Clone, Default)]
struct Draft {
    text: String,
    attachments: Vec<Value>,
    key: String,
}

pub struct WorkspaceView {
    input: Entity<input::TextInput>,
    requests: UnboundedSender<Value>,
    next_id: u64,
    pending: HashMap<u64, String>,
    chats: Vec<Value>,
    providers: Vec<Value>,
    selected: Option<Value>,
    messages: Vec<SessionMessageEntry>,
    transcript_watch: Option<u64>,
    device_id: Option<String>,
    status: String,
    error: Option<String>,
    show_archive: bool,
    settings: bool,
    provider_index: usize,
    send_draft: HashMap<u64, Draft>,
    drafts: HashMap<String, Draft>,
    creating: Option<(u64, Value, Value)>,
    queue: Vec<Value>,
    queue_watch: Option<u64>,
    sessions: Vec<Value>,
    repos: Vec<Value>,
    project: Option<Value>,
    project_picker: bool,
    project_input: Entity<input::TextInput>,
    right_open: bool,
    right_tab: String,
    directory: String,
    entries: Vec<Value>,
    directory_request: Option<u64>,
    file_request: Option<u64>,
    open_file: Option<Value>,
    file_editor: Entity<input::TextInput>,
    save_request: Option<(u64, String)>,
    accounts: Value,
    models: Vec<Value>,
    model: Option<String>,
    model_picker: bool,
    terminal_id: Option<String>,
    terminal_watch: Option<u64>,
    terminal_text: String,
    terminal: vt100::Parser,
    terminal_input: Entity<input::TextInput>,
    changes: Value,
    changes_request: Option<u64>,
    attachments: Vec<Value>,
    uploading: usize,
    composer_generation: u64,
    _reader: Task<()>,
}

impl WorkspaceView {
    pub fn new(transport: Transport, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| input::TextInput::multiline("Do anything…", window, cx));
        let project_input =
            cx.new(|cx| input::TextInput::new("Absolute project path on this machine", window, cx));
        let terminal_input = cx.new(|cx| input::TextInput::new("Run a command…", window, cx));
        cx.subscribe_in(
            &input,
            window,
            |this, _, _: &input::Submitted, window, cx| this.submit(&Submit, window, cx),
        )
        .detach();
        cx.subscribe_in(
            &project_input,
            window,
            |this, _, _: &input::Submitted, _, cx| this.add_project(cx),
        )
        .detach();
        cx.subscribe_in(
            &terminal_input,
            window,
            |this, _, _: &input::Submitted, _, cx| this.run_terminal_command(cx),
        )
        .detach();
        let file_editor = cx.new(|cx| input::TextInput::code(window, cx));
        cx.observe(&file_editor, |_, _, cx| cx.notify()).detach();
        let mut events = transport.events;
        let reader = cx.spawn(async move |this, cx| {
            while let Some(frame) = events.next().await {
                if this.update(cx, |this, cx| this.receive(frame, cx)).is_err() {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.status = "Disconnected".into();
                cx.notify();
            });
        });
        let mut view = Self {
            input,
            requests: transport.requests,
            next_id: 1,
            pending: HashMap::new(),
            chats: vec![],
            providers: vec![],
            selected: None,
            messages: vec![],
            transcript_watch: None,
            device_id: None,
            status: "Connecting to your engine…".into(),
            error: None,
            show_archive: false,
            settings: false,
            provider_index: 0,
            send_draft: HashMap::new(),
            drafts: HashMap::new(),
            creating: None,
            queue: vec![],
            queue_watch: None,
            sessions: vec![],
            repos: vec![],
            project: None,
            project_picker: false,
            project_input,
            right_open: true,
            right_tab: "files".into(),
            directory: String::new(),
            entries: vec![],
            directory_request: None,
            file_request: None,
            open_file: None,
            file_editor,
            save_request: None,
            accounts: Value::Null,
            models: vec![],
            model: None,
            model_picker: false,
            terminal_id: None,
            terminal_watch: None,
            terminal_text: String::new(),
            terminal: vt100::Parser::new(28, 80, 2000),
            terminal_input,
            changes: Value::Null,
            changes_request: None,
            attachments: vec![],
            uploading: 0,
            composer_generation: 0,
            _reader: reader,
        };
        view.call(methods::LOCAL_DEVICE, json!({}));
        view.call(methods::LIST_HARNESSES, json!({}));
        view.call(methods::WATCH_CHATS, json!({}));
        view.call(methods::WATCH_SESSIONS, json!({}));
        view.call(methods::LIST_REPOS, json!({}));
        view
    }

    fn call(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, method.into());
        if self
            .requests
            .unbounded_send(json!({"id":id,"method":method,"params":params}))
            .is_err()
        {
            self.error = Some("The engine connection is closed. Reconnect before sending.".into());
        }
        id
    }

    fn receive(&mut self, frame: Value, cx: &mut Context<Self>) {
        if let Some(connection) = frame.get("connection").and_then(Value::as_str) {
            self.status = connection.into();
            cx.notify();
            return;
        }
        if frame.get("attachment").is_some() || frame.get("uploading").is_some() {
            if frame["generation"].as_u64() != Some(self.composer_generation) {
                return;
            }
        }
        if frame.get("attachment").is_some() {
            self.uploading = self.uploading.saturating_sub(1);
            if let Some(error) = frame["attachment"]["error"].as_str() {
                self.error = Some(error.to_owned());
            } else {
                self.attachments.push(frame["attachment"].clone());
            }
            cx.notify();
            return;
        }
        if frame.get("uploading").is_some() {
            self.uploading += frame["uploading"].as_u64().unwrap_or(0) as usize;
            cx.notify();
            return;
        }
        let Some(id) = frame.get("id").and_then(Value::as_u64) else {
            return;
        };
        let method = self.pending.get(&id).cloned().unwrap_or_default();
        if let Some(error) = frame.get("err").and_then(Value::as_str) {
            self.error = Some(error.into());
            if let Some(draft) = self.send_draft.remove(&id) {
                self.restore_failed_draft(draft, cx);
            }
            self.pending.remove(&id);
            if self
                .save_request
                .as_ref()
                .is_some_and(|(request, _)| *request == id)
            {
                self.save_request = None;
            }
            if self
                .creating
                .as_ref()
                .is_some_and(|(request, _, _)| *request == id)
            {
                self.creating = None;
            }
            cx.notify();
            return;
        }
        if frame.get("ok").is_some()
            && self
                .creating
                .as_ref()
                .is_some_and(|(request, _, _)| *request == id)
        {
            let (_, chat, command) = self.creating.take().unwrap();
            self.pending.remove(&id);
            let mut draft = self.send_draft.remove(&id).unwrap_or_default();
            self.select(chat.clone(), cx);
            draft.key = chat["id"].as_str().unwrap_or_default().into();
            let send = self.call(
                methods::QUEUE_COMMAND,
                json!({"chatId":chat["id"],"command":command}),
            );
            self.send_draft.insert(send, draft);
            cx.notify();
            return;
        }
        let value = frame.get("item").or_else(|| frame.get("ok"));
        if let Some(value) = value {
            match method.as_str() {
                methods::LOCAL_DEVICE => {
                    self.device_id = value
                        .get("deviceId")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    self.status = "Local engine connected".into();
                }
                methods::LIST_HARNESSES => {
                    self.providers = value
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|p| p["id"] != "mock")
                        .collect()
                }
                methods::WATCH_CHATS => {
                    self.chats = value.as_array().cloned().unwrap_or_default();
                    if let Some(selected) = &self.selected {
                        if let Some(chat) =
                            self.chats.iter().find(|chat| chat["id"] == selected["id"])
                        {
                            self.selected = Some(chat.clone());
                        }
                    }
                }
                methods::WATCH_SESSIONS => {
                    self.sessions = value.as_array().cloned().unwrap_or_default()
                }
                methods::LIST_REPOS => self.repos = value.as_array().cloned().unwrap_or_default(),
                methods::ADD_REPO => {
                    self.project = Some(value.clone());
                    self.project_picker = false;
                    self.call(methods::LIST_REPOS, json!({}));
                }
                methods::LIST_MODELS => self.models = value.as_array().cloned().unwrap_or_default(),
                methods::LIST_AGENT_ACCOUNTS => self.accounts = value.clone(),
                methods::WATCH_QUEUE if self.queue_watch == Some(id) => {
                    self.queue = value["items"].as_array().cloned().unwrap_or_default()
                }
                methods::LIST_WORKSPACE_DIRECTORY if self.directory_request == Some(id) => {
                    self.directory = value["directory"].as_str().unwrap_or_default().into();
                    self.entries = value["entries"].as_array().cloned().unwrap_or_default();
                }
                methods::READ_WORKSPACE_FILE if self.file_request == Some(id) => {
                    let text = value["text"].as_str().unwrap_or_default().to_owned();
                    self.file_editor.update(cx, |input, cx| {
                        input.set_readonly(
                            value["readOnlyReason"].is_string() || value["truncated"] == true,
                            cx,
                        );
                        input.set_value(text, cx);
                    });
                    self.open_file = Some(value.clone());
                }
                methods::WRITE_WORKSPACE_FILE
                    if self
                        .save_request
                        .as_ref()
                        .is_some_and(|(request, _)| *request == id) =>
                {
                    let (_, text) = self.save_request.take().unwrap();
                    if value["status"] == "written" {
                        if let Some(file) = &mut self.open_file {
                            file["contentHash"] = value["file"]["contentHash"].clone();
                            file["text"] = json!(text);
                        }
                    } else {
                        self.error=Some("This file changed on disk. Reload it before saving; your edits are still in the editor.".into());
                    }
                }
                methods::GET_CHECKOUT_DIFF if self.changes_request == Some(id) => {
                    self.changes = value.clone()
                }
                methods::OPEN_TERMINAL => {
                    if let Some(terminal) = value["id"].as_str() {
                        self.terminal_id = Some(terminal.into());
                        let watch =
                            self.call(methods::SUBSCRIBE_TERMINAL, json!({"terminalId":terminal}));
                        self.terminal_watch = Some(watch);
                    }
                }
                methods::SUBSCRIBE_TERMINAL if self.terminal_watch == Some(id) => {
                    self.receive_terminal(value)
                }
                methods::QUEUE_MESSAGE => {
                    self.send_draft.remove(&id);
                }
                methods::WATCH_DOC_MESSAGES if self.transcript_watch == Some(id) => {
                    match serde_json::from_value::<TranscriptUpdate>(value.clone()) {
                        Ok(update) => {
                            if let Err(error) =
                                apply_transcript_frame(&mut self.messages, update.frame)
                            {
                                self.error =
                                    Some(format!("Transcript needs resynchronization: {error}"));
                            }
                        }
                        Err(error) => {
                            self.error = Some(format!("Invalid transcript update: {error}"))
                        }
                    }
                }
                methods::QUEUE_COMMAND => {
                    self.send_draft.remove(&id);
                }
                _ => {}
            }
        }
        if frame.get("ok").is_some() || frame["done"] == true {
            self.pending.remove(&id);
        }
        cx.notify();
    }

    fn select(&mut self, chat: Value, cx: &mut Context<Self>) {
        if self.file_dirty(cx) {
            self.error = Some("Save or discard file edits before switching sessions.".into());
            cx.notify();
            return;
        }
        self.stash_draft(cx);
        self.composer_generation += 1;
        self.attachments.clear();
        self.uploading = 0;
        if let Some(id) = self.transcript_watch.take() {
            let _ = self.requests.unbounded_send(json!({"id":id,"cancel":true}));
            self.pending.remove(&id);
        }
        self.clear_workspace();
        let queue_watch = self.call(methods::WATCH_QUEUE, json!({"chatId":chat["id"]}));
        self.queue_watch = Some(queue_watch);
        let id = self.call(methods::WATCH_DOC_MESSAGES, json!({"chatId":chat["id"]}));
        self.transcript_watch = Some(id);
        self.messages.clear();
        self.selected = Some(chat);
        self.restore_draft(cx);
        if let Some(chat) = &self.selected {
            if let Some(harness) = chat["config"]["harness"].as_str() {
                if let Some(index) = self.providers.iter().position(|p| p["id"] == harness) {
                    self.provider_index = index;
                }
            }
            self.model = chat["config"]["model"].as_str().map(str::to_owned);
        }
        self.load_directory("");
        self.settings = false;
        self.error = None;
        cx.notify();
    }

    fn new_chat(&mut self, cx: &mut Context<Self>) {
        if self.file_dirty(cx) {
            self.error = Some("Save or discard file edits before starting another session.".into());
            cx.notify();
            return;
        }
        self.stash_draft(cx);
        self.composer_generation += 1;
        self.attachments.clear();
        self.uploading = 0;
        if let Some(id) = self.transcript_watch.take() {
            self.cancel(id);
        }
        self.clear_workspace();
        self.selected = None;
        self.messages.clear();
        self.settings = false;
        self.restore_draft(cx);
        cx.notify();
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_picker
            && self
                .project_input
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
        {
            self.add_project(cx);
            return;
        }
        if self
            .terminal_input
            .read(cx)
            .focus_handle(cx)
            .is_focused(window)
        {
            self.run_terminal_command(cx);
            return;
        }
        if self.creating.is_some() || self.uploading > 0 {
            return;
        }
        let text = self.input.read(cx).value(cx);
        if text.trim().is_empty() {
            return;
        }
        let Some(device) = self.device_id.clone() else {
            self.error = Some("Connect to an engine before sending.".into());
            cx.notify();
            return;
        };
        let Some(provider) = self.providers.get(self.provider_index).cloned() else {
            self.error = Some("Set up an agent in Settings first.".into());
            cx.notify();
            return;
        };
        if provider["installed"] == false {
            self.error = Some(format!(
                "{} is not installed on the engine host.",
                provider["name"].as_str().unwrap_or("This agent")
            ));
            cx.notify();
            return;
        }
        let draft = Draft {
            text: text.clone(),
            attachments: self.attachments.clone(),
            key: self.draft_key(),
        };
        self.drafts.remove(&draft.key);
        if self.is_working() {
            let chat = self.selected.as_ref().unwrap();
            let id = self.call(methods::QUEUE_MESSAGE, json!({"chatId":chat["id"], "text":text, "holdForTurnEnd":true, "attachments":self.attachment_paths()}));
            self.send_draft.insert(id, draft);
            self.input
                .update(cx, |input, cx| input.set_value(String::new(), cx));
            self.attachments.clear();
            cx.notify();
            return;
        }
        let command = json!({"kind":"run","messageId":uuid::Uuid::new_v4().to_string(),"request":{"prompt":text,"harness":provider["id"],"model":self.model,"reasoning":null,"cwd":self.cwd(),"sandbox":"workspace-write","resume":null,"attachments":self.attachment_paths()}});
        let send = if let Some(chat) = &self.selected {
            self.call(
                methods::QUEUE_COMMAND,
                json!({"chatId":chat["id"],"command":command}),
            )
        } else {
            // IDs are client-owned, allowing the durable send to refer to the same chat.
            let id = uuid::Uuid::new_v4().to_string();
            let request = self.call(methods::MUTATE, json!({"op":"createChat","chatId":id,"deviceId":device,"cwd":self.project.as_ref().and_then(|p|p["path"].as_str()),"config":{"harness":provider["id"],"model":self.model,"reasoning":null,"sandbox":"workspace-write"}}));
            let chat = json!({"id":id,"title":text.chars().take(70).collect::<String>(),"deviceId":device,"cwd":self.cwd()});
            self.creating = Some((request, chat, command));
            request
        };
        self.attachments.clear();
        self.send_draft.insert(send, draft);
        self.input
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.error = None;
        cx.notify();
    }
}

pub fn initialize(cx: &mut App) {
    input::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-s", SaveFile, Some("Workspace")),
        KeyBinding::new("ctrl-s", SaveFile, Some("Workspace")),
    ]);

    cx.text_system()
        .add_fonts(vec![std::borrow::Cow::Borrowed(include_bytes!(
            "../../ui/assets/fonts/GeistMono.ttf"
        ))])
        .expect("bundled monospace font");
    cx.text_system()
        .add_fonts(vec![std::borrow::Cow::Borrowed(include_bytes!(
            "../../ui/assets/fonts/Geist.ttf"
        ))])
        .expect("bundled UI font");
}
