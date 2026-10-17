//! Browser-compatible GPUI views. Native process/filesystem access stays behind RPC.

mod activity;
pub mod assets;
mod composer_alerts;
mod connection;
pub mod input;
mod model_accounts;
mod model_picker;
mod navigation;
mod notifications;
mod pairing;
pub mod palette;
mod previews;
mod provider_settings;
mod questions;
mod recovery;
pub mod settings_style;
mod settings_view;
mod shell;
pub mod styling;
mod tool_results;
mod workspace;
mod workspace_review;

use futures::{
    StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender},
};
use gpui::{prelude::*, *};
use palette::rgb;
use serde_json::{Value, json};
use std::collections::HashMap;
use zuse_model::{SessionMessageEntry, TranscriptUpdate, apply_transcript_frame};
use zuse_protocol::methods;

actions!(
    zuse_client,
    [Submit, SaveFile, ClosePreview, SearchSessions, NewSession]
);

pub struct Transport {
    pub requests: UnboundedSender<Value>,
    pub events: UnboundedReceiver<Value>,
}

#[derive(Clone, Default)]
struct Draft {
    text: String,
    attachments: Vec<Value>,
    key: String,
    message_id: Option<String>,
    chat_id: Option<String>,
    not_sent: bool,
}

pub struct WorkspaceView {
    workspace_focus: FocusHandle,
    input: Entity<input::TextInput>,
    requests: UnboundedSender<Value>,
    next_id: u64,
    pending: HashMap<u64, String>,
    chats: Vec<Value>,
    providers: Vec<Value>,
    selected: Option<Value>,
    messages: Vec<SessionMessageEntry>,
    transcript_watch: Option<u64>,
    previews: HashMap<String, previews::Preview>,
    preview_pending: HashMap<u64, String>,
    preview_generation: u64,
    preview_open: Option<String>,
    tool_results: HashMap<String, tool_results::ToolResultView>,
    tool_result_requests: HashMap<u64, (String, usize)>,
    transcript_scroll: ScrollHandle,
    input_answers: HashMap<String, zuse_protocol::input_answers::InputAnswers>,
    input_pending: HashMap<u64, String>,
    input_submitted: std::collections::HashSet<String>,
    input_editing: Option<(String, String)>,
    input_text: Entity<input::TextInput>,
    expanded_parts: std::collections::HashSet<String>,
    device_id: Option<String>,
    status: String,
    connected: bool,
    connection_epoch: u64,
    workspace_synced: bool,
    uncertain_drafts: Vec<Draft>,
    connection_notice: Option<String>,
    uncertain_save: Option<workspace::PendingFileSave>,
    file_reconcile: Option<u64>,
    file_reconnect_needed: bool,
    terminal_last_seq: Option<u64>,
    terminal_write: Option<(u64, String)>,
    error: Option<String>,
    show_archive: bool,
    settings: bool,
    provider_expanded: Option<String>,
    provider_actions: HashMap<u64, String>,
    provider_logins: HashMap<String, Value>,
    provider_codes: HashMap<String, Entity<input::TextInput>>,
    provider_error: Option<String>,
    settings_section: settings_view::Section,
    paired: bool,
    pairing_origin: String,
    pairings: Value,
    pairing_name: Entity<input::TextInput>,
    pairing_fragment: Option<String>,
    pairing_fetch: Option<u64>,
    pairing_action: Option<u64>,
    provider_index: usize,
    send_draft: HashMap<u64, Draft>,
    drafts: HashMap<String, Draft>,
    creating: Option<(u64, Value, Value)>,
    queue: Vec<Value>,
    queue_watch: Option<u64>,
    sessions: Vec<Value>,
    notification_baselines: HashMap<String, zuse_protocol::notifications::SessionNotificationState>,
    completion_preset_menu: bool,
    completion_preset_active: usize,
    completion_preset_focus: FocusHandle,
    repos: Vec<Value>,
    spaces: Vec<zuse_protocol::Space>,
    project: Option<Value>,
    project_picker: bool,
    project_input: Entity<input::TextInput>,
    sidebar_open: bool,
    search_open: bool,
    session_search: Entity<input::TextInput>,
    agent_picker: bool,
    preferences: zuse_protocol::presentation::WorkspacePreferences,
    picker_catalog: zuse_protocol::presentation::PickerCatalog,
    picker_scope: Option<String>,
    model_search: Entity<input::TextInput>,
    picker_active: Option<zuse_protocol::presentation::ModelSelection>,
    picker_scroll: ScrollHandle,
    picker_anchors: HashMap<String, ScrollAnchor>,
    reasoning: Option<zuse_protocol::ReasoningLevel>,
    reasoning_slider: Entity<gpui_base::slider::SliderState>,
    reasoning_focus: FocusHandle,
    model_options: serde_json::Map<String, Value>,
    sandbox: zuse_protocol::SandboxLevel,
    access_picker: bool,
    add_menu: bool,
    expanded_alert: Option<String>,
    dismissed_alerts: std::collections::HashSet<String>,
    right_open: bool,
    content_width: f32,
    content_height: f32,
    right_tab: String,
    recovery: Option<zuse_protocol::recovery::RecoverySnapshot>,
    recovery_request: Option<u64>,
    recovery_selected: Option<usize>,
    recovery_error: Option<String>,
    recovery_notice: Option<String>,
    recovery_output: Entity<input::TextInput>,
    directory: String,
    entries: Vec<Value>,
    directory_request: Option<u64>,
    file_request: Option<u64>,
    open_file: Option<Value>,
    file_editor: Entity<input::TextInput>,
    save_request: Option<workspace::PendingFileSave>,
    file_read_baseline: Option<String>,
    directory_cursor: Option<String>,
    files_scroll: UniformListScrollHandle,
    directory_append: bool,
    accounts: Value,
    cloud_account: Value,
    browser_local: bool,
    plugins: Value,
    plugin_attempt: Value,
    plugin_request: Option<u64>,
    model_connections: Value,
    model_login: Value,
    model_storage: &'static str,
    model_fetch: Option<u64>,
    model_sign_in: Option<u64>,
    model_action: Option<u64>,
    model_editing: Option<String>,
    model_name: Entity<input::TextInput>,
    model_callback: Entity<input::TextInput>,
    model_error: Option<String>,
    model: Option<String>,
    model_picker: bool,
    model_picker_default: bool,
    terminal_id: Option<String>,
    terminal_watch: Option<u64>,
    terminal_text: String,
    terminal: vt100::Parser,
    terminal_input: Entity<input::TextInput>,
    changes: Value,
    changes_error: Option<String>,
    review: workspace_review::WorkspaceReview,
    changes_request: Option<u64>,
    attachments: Vec<Value>,
    uploading: usize,
    composer_generation: u64,
    _reader: Task<()>,
}

impl WorkspaceView {
    pub fn new(transport: Transport, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace_focus = cx.focus_handle();
        window.focus(&workspace_focus, cx);
        let input = cx.new(|cx| {
            input::TextInput::multiline("Ask to make changes or ask a question…", window, cx)
        });
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        let project_input =
            cx.new(|cx| input::TextInput::new("Absolute project path on this machine", window, cx));
        let session_search = cx.new(|cx| input::TextInput::new("Search sessions…", window, cx));
        cx.observe(&session_search, |_, _, cx| cx.notify()).detach();
        let model_search = cx.new(|cx| input::TextInput::new("Search models…", window, cx));
        cx.observe(&model_search, |this, _, cx| {
            this.picker_active = None;
            cx.notify();
        })
        .detach();
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
        let reasoning_slider = cx.new(|_| {
            gpui_base::slider::SliderState::new()
                .min(0.)
                .max(1.)
                .step(1.)
        });
        cx.observe(&reasoning_slider, |this, state, cx| {
            let index = state.read(cx).value().end().round().max(0.) as usize;
            this.reasoning = if index == 0 {
                None
            } else {
                this.selected_model()
                    .and_then(|m| m.reasoning_levels.get(index - 1))
                    .copied()
            };
            cx.notify();
        })
        .detach();
        cx.subscribe_in(
            &reasoning_slider,
            window,
            |this, state, event: &gpui_base::slider::SliderEvent, window, cx| {
                if let gpui_base::slider::SliderEvent::Change(value) = event {
                    state.update(cx, |state, cx| {
                        state.set_value(value.end().round(), window, cx)
                    });
                    window.focus(&this.reasoning_focus, cx);
                }
            },
        )
        .detach();
        let reasoning_focus = cx.focus_handle();
        let file_editor = cx.new(|cx| input::TextInput::code(window, cx));
        cx.observe(&file_editor, |_, _, cx| cx.notify()).detach();
        let model_name = cx.new(|cx| input::TextInput::new("Connection name", window, cx));
        let model_callback =
            cx.new(|cx| input::TextInput::new("Paste the localhost callback URL", window, cx));
        let recovery_output = cx.new(|cx| {
            input::TextInput::multiline("Describe the result you inspected…", window, cx)
        });
        let input_text = cx.new(|cx| input::TextInput::multiline("Write an answer…", window, cx));
        let pairing_name = cx.new(|cx| input::TextInput::new("Device name", window, cx));
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
        let view = Self {
            workspace_focus,
            input,
            requests: transport.requests,
            next_id: 1,
            pending: HashMap::new(),
            chats: vec![],
            providers: vec![],
            selected: None,
            messages: vec![],
            transcript_watch: None,
            previews: HashMap::new(),
            preview_pending: HashMap::new(),
            preview_generation: 0,
            preview_open: None,
            tool_results: HashMap::new(),
            tool_result_requests: HashMap::new(),
            transcript_scroll: ScrollHandle::new(),
            input_answers: HashMap::new(),
            input_pending: HashMap::new(),
            input_submitted: Default::default(),
            input_editing: None,
            input_text,
            expanded_parts: Default::default(),
            device_id: None,
            status: "Connecting to your engine…".into(),
            connected: false,
            connection_epoch: 0,
            workspace_synced: false,
            uncertain_drafts: vec![],
            connection_notice: None,
            uncertain_save: None,
            file_reconcile: None,
            file_reconnect_needed: false,
            terminal_last_seq: None,
            terminal_write: None,
            error: None,
            show_archive: false,
            settings: false,
            provider_expanded: None,
            provider_actions: HashMap::new(),
            provider_logins: HashMap::new(),
            provider_codes: HashMap::new(),
            provider_error: None,
            settings_section: settings_view::Section::General,
            paired: false,
            pairing_origin: String::new(),
            pairings: Value::Null,
            pairing_name,
            pairing_fragment: None,
            pairing_fetch: None,
            pairing_action: None,
            provider_index: 0,
            send_draft: HashMap::new(),
            drafts: HashMap::new(),
            creating: None,
            queue: vec![],
            queue_watch: None,
            sessions: vec![],
            notification_baselines: HashMap::new(),
            completion_preset_menu: false,
            completion_preset_active: 0,
            completion_preset_focus: cx.focus_handle(),
            repos: vec![],
            spaces: vec![],
            project: None,
            project_picker: false,
            project_input,
            sidebar_open: true,
            search_open: false,
            session_search,
            agent_picker: false,
            preferences: Default::default(),
            picker_catalog: Default::default(),
            picker_scope: None,
            model_search,
            picker_active: None,
            picker_scroll: ScrollHandle::new(),
            picker_anchors: HashMap::new(),
            reasoning: None,
            reasoning_slider,
            reasoning_focus,
            model_options: Default::default(),
            sandbox: zuse_protocol::SandboxLevel::WorkspaceWrite,
            access_picker: false,
            add_menu: false,
            expanded_alert: None,
            dismissed_alerts: Default::default(),
            right_open: false,
            content_width: 768.,
            content_height: 880.,
            right_tab: "files".into(),
            recovery: None,
            recovery_request: None,
            recovery_selected: None,
            recovery_error: None,
            recovery_notice: None,
            recovery_output,
            directory: String::new(),
            entries: vec![],
            directory_request: None,
            file_request: None,
            open_file: None,
            file_editor,
            save_request: None,
            file_read_baseline: None,
            directory_cursor: None,
            files_scroll: UniformListScrollHandle::new(),
            directory_append: false,
            accounts: Value::Null,
            cloud_account: Value::Null,
            browser_local: false,
            plugins: Value::Null,
            plugin_attempt: Value::Null,
            plugin_request: None,
            model_connections: Value::Null,
            model_login: Value::Null,
            model_storage: "local",
            model_fetch: None,
            model_sign_in: None,
            model_action: None,
            model_editing: None,
            model_name,
            model_callback,
            model_error: None,
            model: None,
            model_picker: false,
            model_picker_default: false,
            terminal_id: None,
            terminal_watch: None,
            terminal_text: String::new(),
            terminal: vt100::Parser::new(28, 80, 2000),
            terminal_input,
            changes: Value::Null,
            changes_error: None,
            review: Default::default(),
            changes_request: None,
            attachments: vec![],
            uploading: 0,
            composer_generation: 0,
            _reader: reader,
        };
        view
    }

    fn plugin_action(&mut self, request: Value) {
        if self.plugin_request.is_some() {
            return;
        }
        self.plugin_request = Some(self.call(methods::PLUGINS, request));
    }

    fn call(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(id, method.into());
        if self
            .requests
            .unbounded_send(json!({"id":id,"method":method,"params":params,"epoch":if self.connected||method==methods::LOCAL_DEVICE {self.connection_epoch}else{0}}))
            .is_err()
        {
            self.error = Some("The engine connection is closed. Reconnect before sending.".into());
        }
        id
    }

    fn receive(&mut self, frame: Value, cx: &mut Context<Self>) {
        if let Some(value) = frame.get("preferences") {
            if let Ok(mut preferences) = serde_json::from_value::<
                zuse_protocol::presentation::WorkspacePreferences,
            >(value.clone())
            {
                preferences.normalize();
                self.preferences = preferences;
                cx.notify();
            }
            return;
        }
        if let Some(paired) = frame["paired"].as_bool() {
            self.paired = paired;
        }
        if let Some(origin) = frame["pairingOrigin"].as_str() {
            self.pairing_origin = origin.into();
        }
        if let Some(local) = frame["browserLocal"].as_bool() {
            self.browser_local = local;
            return;
        }
        if frame.get("connection").is_some() {
            self.receive_connection(&frame, cx);
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
                self.sync_previews(true, cx);
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
        if method.is_empty() {
            return;
        }
        if self.receive_picker_catalog(id, &frame, cx) {
            return;
        }
        if self.receive_file_reconcile(id, &frame, cx) {
            return;
        }
        if self.receive_review(id, &frame, cx) {
            return;
        }
        if self.receive_tool_result(id, &frame, cx) {
            return;
        }
        if self.receive_preview(id, &frame, cx) {
            return;
        }
        self.receive_input(id, &frame);
        if self.receive_recovery(id, &frame, cx) {
            return;
        }
        if self.receive_pairing(id, &frame, cx) {
            return;
        }
        if self.receive_provider_action(id, &frame, cx) {
            return;
        }
        if self.receive_model_accounts(id, &frame, cx) {
            return;
        }
        if let Some(error) = frame.get("err").and_then(Value::as_str) {
            self.error = Some(error.into());
            if self.plugin_request == Some(id) {
                self.plugin_request = None;
            }
            if let Some(draft) = self.send_draft.remove(&id) {
                self.restore_failed_draft(draft, cx);
            }
            self.pending.remove(&id);
            if self.terminal_watch == Some(id) {
                self.terminal_watch = None;
                self.terminal_id = None;
                self.terminal_last_seq = None;
                self.terminal_text
                    .push_str("\n[Terminal unavailable. Open a new terminal to continue.]\n");
            }
            if self
                .terminal_write
                .as_ref()
                .is_some_and(|(request, _)| *request == id)
            {
                self.terminal_write = None;
            }
            if self.file_request == Some(id) {
                self.file_request = None;
                self.file_read_baseline = None;
            }
            if self.directory_request == Some(id) {
                self.directory_request = None;
            }
            if self
                .save_request
                .as_ref()
                .is_some_and(|request| request.id == id)
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
                methods::CLOUD_ACCOUNT_STATUS => {
                    let changed = self.cloud_account["user"]["id"] != value["user"]["id"];
                    self.cloud_account = value.clone();
                    if changed {
                        self.reset_model_connections();
                        self.model_callback
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                        self.refresh_model_connections();
                        self.plugins = Value::Null;
                        self.plugin_attempt = Value::Null;
                        self.plugin_request = None;
                        if value["user"].is_object() {
                            self.plugin_action(json!({"action":"list"}));
                        }
                    }
                }
                methods::PLUGINS if self.plugin_request == Some(id) => {
                    self.plugin_request = None;
                    if value["kind"] == "snapshot" {
                        self.plugins = value.clone();
                    } else if value["kind"] == "attempt" {
                        self.plugin_attempt = value.clone();
                        if value["state"] == "connected" {
                            self.plugin_action(json!({"action":"list"}));
                        }
                    } else {
                        self.plugin_attempt = Value::Null;
                        self.plugin_action(json!({"action":"list"}));
                    }
                }
                methods::LOCAL_DEVICE => self.verify_engine(value, cx),
                methods::LIST_HARNESSES
                | methods::SET_HARNESS_ENABLED
                | methods::INSTALL_HARNESS => {
                    let previous = self
                        .providers
                        .get(self.provider_index)
                        .and_then(|p| p["id"].as_str())
                        .map(str::to_owned);
                    let initial = previous.is_none();
                    self.providers = value
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|p| p["id"] != "mock")
                        .collect();
                    let preferred = previous.or_else(|| {
                        self.selected
                            .as_ref()
                            .and_then(|c| c["config"]["harness"].as_str())
                            .map(str::to_owned)
                    });
                    if let Some(index) =
                        preferred.and_then(|id| self.providers.iter().position(|p| p["id"] == id))
                    {
                        self.provider_index = index;
                    } else {
                        self.provider_index = 0;
                    }
                    if initial {
                        self.apply_default_model();
                    }
                }
                methods::WATCH_CHATS => {
                    self.workspace_synced = true;
                    self.chats = value.as_array().cloned().unwrap_or_default();
                    if let Some(selected) = &self.selected {
                        if let Some(chat) = self
                            .chats
                            .iter()
                            .find(|chat| chat["id"] == selected["id"])
                            .cloned()
                        {
                            if chat["deviceId"] != selected["deviceId"]
                                || chat["harnessSessionId"] != selected["harnessSessionId"]
                            {
                                self.reset_recovery(cx);
                            }
                            self.selected = Some(chat);
                        }
                    }
                }
                methods::WATCH_SESSIONS => {
                    self.receive_session_notifications(value);
                    self.sessions = value.as_array().cloned().unwrap_or_default()
                }
                methods::LIST_REPOS => self.repos = value.as_array().cloned().unwrap_or_default(),
                methods::WATCH_SPACES => {
                    self.spaces = serde_json::from_value(value.clone()).unwrap_or_default()
                }
                methods::ADD_REPO => {
                    self.project = Some(value.clone());
                    self.project_picker = false;
                    self.call(methods::LIST_REPOS, json!({}));
                }

                methods::LIST_AGENT_ACCOUNTS => self.accounts = value.clone(),
                methods::WATCH_QUEUE if self.queue_watch == Some(id) => {
                    self.queue = value["items"].as_array().cloned().unwrap_or_default()
                }
                methods::LIST_WORKSPACE_DIRECTORY if self.directory_request == Some(id) => {
                    self.directory_request = None;
                    let directory = value["directory"].as_str().unwrap_or_default();
                    if self.directory_append && directory != self.directory {
                        self.error = Some(
                            "The folder changed while loading more files. Refresh it to continue."
                                .into(),
                        );
                    } else {
                        if !self.directory_append {
                            self.entries.clear();
                        }
                        for entry in value["entries"].as_array().into_iter().flatten() {
                            if !self.entries.iter().any(|old| old["path"] == entry["path"]) {
                                self.entries.push(entry.clone());
                            }
                        }
                        self.directory = directory.into();
                        self.directory_cursor = value["nextCursor"].as_str().map(str::to_owned);
                    }
                    self.directory_append = false;
                }
                methods::READ_WORKSPACE_FILE if self.file_request == Some(id) => {
                    self.file_request = None;
                    let baseline = self.file_read_baseline.take();
                    if baseline.as_deref() != Some(self.file_editor.read(cx).value(cx).as_str()) {
                        self.error=Some("Your editor changed while the file was loading. Your edits are still here; open the file again when ready.".into());
                        self.pending.remove(&id);
                        cx.notify();
                        return;
                    }
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
                        .is_some_and(|request| request.id == id) =>
                {
                    let saved = self.save_request.take().unwrap();
                    if value["status"] == "written" {
                        if let Some(file) = &mut self.open_file {
                            if file["path"] == saved.file["path"]
                                && file["checkoutId"] == saved.file["checkoutId"]
                                && file["contentHash"] == saved.file["contentHash"]
                            {
                                file["contentHash"] = value["file"]["contentHash"].clone();
                                file["text"] = json!(saved.text);
                                file["size"] = value["file"]["size"].clone();
                            }
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
                methods::WRITE_TERMINAL
                    if self
                        .terminal_write
                        .as_ref()
                        .is_some_and(|(request, _)| *request == id) =>
                {
                    if let Some((_, text)) = self.terminal_write.take() {
                        if self.terminal_input.read(cx).value(cx) == text {
                            self.terminal_input
                                .update(cx, |input, cx| input.set_value(String::new(), cx));
                        }
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
                            let follow = self.transcript_scroll.max_offset().y
                                + self.transcript_scroll.offset().y
                                <= px(24.);
                            if let Err(error) =
                                apply_transcript_frame(&mut self.messages, update.frame)
                            {
                                self.error =
                                    Some(format!("Transcript needs resynchronization: {error}"));
                            }
                            if follow {
                                self.transcript_scroll.scroll_to_bottom();
                            }
                            self.sync_previews(true, cx);
                            self.confirm_deliveries();
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
        if !self.require_connection(cx) {
            return;
        }
        if self.file_navigation_blocked(cx) {
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
        let queue_watch = self.call(methods::WATCH_QUEUE, json!({"chatId":chat["id"]}));
        self.queue_watch = Some(queue_watch);
        let id = self.call(methods::WATCH_DOC_MESSAGES, json!({"chatId":chat["id"]}));
        self.transcript_watch = Some(id);
        self.messages.clear();
        self.transcript_scroll.scroll_to_bottom();
        self.reset_recovery(cx);
        self.reset_inputs(cx);
        self.reset_tool_results();
        self.reset_previews(cx);
        let projects = self.known_projects();
        let roots = projects
            .iter()
            .filter_map(|p| p["path"].as_str())
            .collect::<Vec<_>>();
        let identity = zuse_protocol::presentation::project_identity(
            chat["cwd"].as_str(),
            chat["spaceId"].as_str(),
            &self.spaces,
            &roots,
        );
        self.project =
            (!identity.path.is_empty()).then(|| json!({"path":identity.path,"name":identity.name}));
        self.selected = Some(chat);
        self.restore_draft(cx);
        if let Some(chat) = &self.selected {
            if let Some(harness) = chat["config"]["harness"].as_str() {
                if let Some(index) = self.providers.iter().position(|p| p["id"] == harness) {
                    self.provider_index = index;
                }
            }
            self.model = chat["config"]["model"].as_str().map(str::to_owned);
            self.reasoning = serde_json::from_value(chat["config"]["reasoning"].clone()).ok();
            self.model_options = chat["config"]["modelOptions"]
                .as_object()
                .cloned()
                .unwrap_or_default();
            self.sandbox = serde_json::from_value(chat["config"]["sandbox"].clone())
                .unwrap_or(zuse_protocol::SandboxLevel::WorkspaceWrite);
        }
        self.load_directory("");
        self.settings = false;
        self.error = None;
        cx.notify();
    }

    fn new_chat(&mut self, cx: &mut Context<Self>) {
        if !self.require_connection(cx) {
            return;
        }
        if self.file_navigation_blocked(cx) {
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
        self.reset_recovery(cx);
        self.reset_inputs(cx);
        self.reset_tool_results();
        self.reset_previews(cx);
        self.selected = None;
        self.model = None;
        self.reasoning = None;
        self.model_options.clear();
        self.apply_default_model();
        self.messages.clear();
        self.transcript_scroll.set_offset(point(px(0.), px(0.)));
        self.settings = false;
        self.restore_draft(cx);
        cx.notify();
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings {
            return;
        }
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
        if !self.require_connection(cx) {
            return;
        }
        if !self.workspace_synced
            || self
                .selected
                .as_ref()
                .is_some_and(|chat| !self.chats.iter().any(|row| row["id"] == chat["id"]))
        {
            self.error = Some("Wait for the conversation list to refresh before sending.".into());
            cx.notify();
            return;
        }
        if self.creating.is_some() || self.uploading > 0 {
            return;
        }
        let text = self.input.read(cx).value(cx);
        if text.trim().is_empty() && self.attachments.is_empty() {
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
        let mut draft = Draft {
            text: text.clone(),
            attachments: self.attachments.clone(),
            key: self.draft_key(),
            chat_id: self
                .selected
                .as_ref()
                .and_then(|chat| chat["id"].as_str())
                .map(str::to_owned),
            message_id: None,
            not_sent: false,
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
        let paths: Vec<String> = self
            .attachment_paths()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let prompt = zuse_protocol::message_attachments::with_attachments(&text, &paths);
        let message_id = uuid::Uuid::new_v4().to_string();
        draft.message_id = Some(message_id.clone());
        let command = json!({"kind":"run","messageId":message_id,"request":{"prompt":prompt,"harness":provider["id"],"model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options,"cwd":self.cwd(),"sandbox":self.sandbox,"autoApprove":self.sandbox==zuse_protocol::SandboxLevel::DangerFullAccess,"resume":null,"attachments":self.attachment_paths()}});
        let send = if let Some(chat) = &self.selected {
            self.call(
                methods::QUEUE_COMMAND,
                json!({"chatId":chat["id"],"command":command}),
            )
        } else {
            // IDs are client-owned, allowing the durable send to refer to the same chat.
            let id = uuid::Uuid::new_v4().to_string();
            draft.chat_id = Some(id.clone());
            let request = self.call(methods::MUTATE, json!({"op":"createChat","chatId":id,"deviceId":device,"cwd":self.project.as_ref().and_then(|p|p["path"].as_str()),"config":{"harness":provider["id"],"model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options,"sandbox":self.sandbox}}));
            let chat = json!({"id":id,"title":text.chars().take(70).collect::<String>(),"deviceId":device,"cwd":self.cwd(),"config":{"harness":provider["id"],"model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options,"sandbox":self.sandbox}});
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
        KeyBinding::new("cmd-k", SearchSessions, Some("Workspace")),
        KeyBinding::new("ctrl-k", SearchSessions, Some("Workspace")),
        KeyBinding::new("cmd-n", NewSession, Some("Workspace")),
        KeyBinding::new("ctrl-n", NewSession, Some("Workspace")),
        KeyBinding::new("escape", ClosePreview, Some("Workspace")),
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
