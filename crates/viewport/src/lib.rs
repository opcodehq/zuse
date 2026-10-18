//! Browser-compatible GPUI views. Native process/filesystem access stays behind RPC.

mod access;
mod activity;
pub mod assets;
#[cfg(test)]
mod attachment_tests;
mod composer_alerts;
mod connection;
pub mod context_status;
mod draft_journal;
pub mod input;
mod model_accounts;
mod model_catalog;
mod model_config;
mod model_picker;
mod navigation;
mod notifications;
mod pairing;
pub mod palette;
mod plugin_management;
pub mod plugin_setup;
mod previews;
mod provider_settings;
mod provider_updates;
mod questions;
mod queue;
mod queue_edit;
mod recovery;
pub mod settings_style;
mod settings_view;
mod shell;
pub mod styling;
mod tool_results;
mod transcript;
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

use zuse_protocol::browser_drafts::BrowserDraft as Draft;

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
    transcript_state: transcript::TranscriptState,
    context_usage: Option<zuse_protocol::ContextUsage>,
    context_menu: bool,
    context_confirmed: bool,
    context_focus: FocusHandle,
    context_trigger: model_picker::TriggerBounds,
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
    settings_nav_focus: FocusHandle,
    settings_nav_active: usize,
    provider_updates: Vec<zuse_protocol::HarnessUpdateStatus>,
    provider_updates_watch: Option<u64>,
    provider_updates_action: Option<(u64, &'static str)>,
    provider_updates_cancel: Option<u64>,
    provider_updates_confirmed: bool,
    provider_updates_error: Option<String>,
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
    draft_journal_ready: bool,
    draft_journal_last: Option<Value>,
    draft_journal_error: Option<String>,
    creating: Option<(u64, Value, Value)>,
    queue: Vec<Value>,
    queue_watch: Option<u64>,
    queue_edit: Option<queue_edit::QueueEdit>,
    queue_actions: HashMap<u64, queue::PendingQueueAction>,
    queue_unconfirmed: std::collections::HashSet<String>,
    sessions: Vec<Value>,
    notification_baselines: HashMap<String, zuse_protocol::notifications::SessionNotificationState>,
    completion_preset_menu: bool,
    completion_preset_active: usize,
    completion_preset_focus: FocusHandle,
    completion_switch_focus: FocusHandle,
    provider_notice_focus: FocusHandle,
    provider_policy_focus: FocusHandle,
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
    preferences_loaded: bool,
    default_permission_menu: bool,
    default_permission_focus: FocusHandle,
    default_model_focus: FocusHandle,
    default_permission_active: usize,
    picker_catalog: zuse_protocol::presentation::PickerCatalog,
    model_host: model_catalog::ModelHost,
    picker_scope: Option<String>,
    model_search: Entity<input::TextInput>,
    picker_active: Option<zuse_protocol::presentation::ModelSelection>,
    picker_scroll: ScrollHandle,
    picker_anchors: HashMap<String, ScrollAnchor>,
    reasoning: Option<zuse_protocol::ReasoningLevel>,
    reasoning_slider: Entity<gpui_base::slider::SliderState>,
    reasoning_focus: FocusHandle,
    model_options: serde_json::Map<String, Value>,
    model_save: Option<model_config::PendingModelSave>,
    model_watch: Option<u64>,
    model_confirmed: bool,
    model_dirty: bool,
    model_unconfirmed: std::collections::HashSet<String>,
    sandbox: zuse_protocol::SandboxLevel,
    access_picker: bool,
    access_focus: FocusHandle,
    access_active: usize,
    access_save: Option<access::PendingAccessSave>,
    access_unconfirmed: Option<String>,
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
    plugin_state: zuse_protocol::plugin_state::PluginState,
    plugin_request: Option<u64>,
    plugin_setup: Option<Entity<plugin_setup::PluginSetup>>,
    plugin_setup_events: Option<Subscription>,
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
        cx.observe_self(|this, cx| this.persist_drafts(cx)).detach();
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
            this.select_effort(index, cx);
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
            transcript_state: Default::default(),
            context_usage: None,
            context_menu: false,
            context_confirmed: false,
            context_focus: cx.focus_handle(),
            context_trigger: Default::default(),
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
            settings_nav_focus: cx.focus_handle(),
            settings_nav_active: 0,
            provider_updates: vec![],
            provider_updates_watch: None,
            provider_updates_action: None,
            provider_updates_cancel: None,
            provider_updates_confirmed: false,
            provider_updates_error: None,
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
            draft_journal_ready: false,
            draft_journal_last: None,
            draft_journal_error: None,
            creating: None,
            queue: vec![],
            queue_watch: None,
            queue_edit: None,
            queue_actions: HashMap::new(),
            queue_unconfirmed: Default::default(),
            sessions: vec![],
            notification_baselines: HashMap::new(),
            completion_preset_menu: false,
            completion_preset_active: 0,
            completion_preset_focus: cx.focus_handle(),
            completion_switch_focus: cx.focus_handle(),
            provider_notice_focus: cx.focus_handle(),
            provider_policy_focus: cx.focus_handle(),
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
            preferences_loaded: false,
            default_permission_menu: false,
            default_permission_focus: cx.focus_handle(),
            default_model_focus: cx.focus_handle(),
            default_permission_active: 0,
            picker_catalog: Default::default(),
            model_host: Default::default(),
            picker_scope: None,
            model_search,
            picker_active: None,
            picker_scroll: ScrollHandle::new(),
            picker_anchors: HashMap::new(),
            reasoning: None,
            reasoning_slider,
            reasoning_focus,
            model_options: Default::default(),
            model_save: None,
            model_watch: None,
            model_confirmed: false,
            model_dirty: false,
            model_unconfirmed: Default::default(),
            sandbox: zuse_protocol::SandboxLevel::WorkspaceWrite,
            access_picker: false,
            access_focus: cx.focus_handle(),
            access_active: 0,
            access_save: None,
            access_unconfirmed: None,
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
            plugin_state: Default::default(),
            plugin_request: None,
            plugin_setup: None,
            plugin_setup_events: None,
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
        if !self.connected {
            return;
        }
        let request = match self.plugin_state.begin(request) {
            Ok(request) => request,
            Err(error) => {
                self.plugin_state.notice = Some(error.into());
                return;
            }
        };
        self.plugin_setup = None;
        self.plugin_setup_events = None;
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
        if self.receive_draft_journal(&frame, cx) {
            return;
        }
        if let Some(value) = frame.get("preferences") {
            if let Ok(mut preferences) = serde_json::from_value::<
                zuse_protocol::presentation::WorkspacePreferences,
            >(value.clone())
            {
                preferences.normalize();
                if !self.preferences_loaded {
                    self.sandbox = preferences.default_permission_mode.sandbox();
                    self.preferences_loaded = true;
                }
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
            if frame["generation"].as_u64() != Some(self.composer_generation)
                || frame["epoch"].as_u64() != Some(self.connection_epoch)
                || (!self.connected && frame["uploadRejected"] != true)
            {
                return;
            }
        }
        if frame.get("attachment").is_some() {
            if frame["uploadRejected"] != true {
                self.uploading = self.uploading.saturating_sub(1);
            }
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
        if self.receive_queue_edit(id, &frame, cx) {
            return;
        }
        if self.receive_queue_action(id, &frame, cx) {
            return;
        }
        if self.receive_model_config(id, &frame, cx) {
            return;
        }
        if self.receive_access(id, &frame, cx) {
            return;
        }
        self.receive_input(id, &frame);
        if self.receive_recovery(id, &frame, cx) {
            return;
        }
        if self.receive_pairing(id, &frame, cx) {
            return;
        }
        if self.receive_provider_updates(id, &frame, cx) {
            return;
        }
        if self.receive_provider_action(id, &frame, cx) {
            return;
        }
        if self.receive_model_accounts(id, &frame, cx) {
            return;
        }
        if method == methods::WATCH_DOC_MESSAGES {
            self.receive_transcript(id, &frame, cx);
            return;
        }
        if method == methods::PLUGINS {
            self.receive_plugins(id, &frame, cx);
            return;
        }
        if let Some(error) = frame.get("err").and_then(Value::as_str) {
            self.error = Some(error.into());
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
                zuse_protocol::presentation::chat_request(&chat, json!({"command":command})),
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
                        self.plugin_setup = None;
                        self.plugin_setup_events = None;
                        self.plugin_state = Default::default();
                        self.plugin_request = None;
                        if value["user"].is_object() {
                            self.plugin_action(json!({"action":"list"}));
                        }
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
                    let preferred = self
                        .selected
                        .as_ref()
                        .and_then(|c| c["config"]["harness"].as_str())
                        .map(str::to_owned)
                        .or(previous);
                    if self.model_host.target.is_none() {
                        if let Some(index) = preferred
                            .and_then(|id| self.providers.iter().position(|p| p["id"] == id))
                        {
                            self.provider_index = index;
                        } else {
                            self.provider_index = 0;
                        }
                        if initial
                            || (self.selected.is_none()
                                && self
                                    .providers
                                    .get(self.provider_index)
                                    .is_none_or(|provider| {
                                        provider["enabled"] == false
                                            || provider["installed"] == false
                                    }))
                        {
                            self.apply_default_model();
                        }
                    }
                    self.prefetch_model_catalogs();
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
                            let host_changed = chat["deviceId"] != selected["deviceId"];
                            let mut chat = chat;
                            if !host_changed && self.model_confirmed {
                                chat["config"] = selected["config"].clone();
                            }
                            let recovery_changed = host_changed
                                || chat["harnessSessionId"] != selected["harnessSessionId"];
                            if chat["config"]["sandbox"] != selected["config"]["sandbox"]
                                || self.access_unconfirmed.as_deref() == chat["id"].as_str()
                            {
                                self.sandbox =
                                    serde_json::from_value(chat["config"]["sandbox"].clone())
                                        .unwrap_or_default();
                            }
                            self.selected = Some(chat.clone());
                            if host_changed {
                                self.interrupt_model_config(cx);
                                self.refresh_model_config_watch();
                            }
                            self.reconcile_model_config(&chat);
                            if host_changed {
                                self.rebind_chat_host(cx);
                            }
                            if recovery_changed {
                                self.reset_recovery(cx);
                            }
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
                    if let Some(items) = value["items"].as_array() {
                        self.queue = items.clone();
                        self.reconcile_queue_edit(cx);
                        if let Some(chat) =
                            self.selected.as_ref().and_then(|chat| chat["id"].as_str())
                        {
                            self.queue_unconfirmed.remove(chat);
                        }
                    } else {
                        if let Some(chat) =
                            self.selected.as_ref().and_then(|chat| chat["id"].as_str())
                        {
                            self.queue_unconfirmed.insert(chat.to_owned());
                        }
                        self.error = Some("The host returned an invalid queue snapshot. Reopen the chat to refresh it.".into());
                    }
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

    fn select(&mut self, chat: Value, cx: &mut Context<Self>) -> bool {
        if !self.require_connection(cx) {
            return false;
        }
        if self.file_navigation_blocked(cx)
            || self.queue_edit_navigation_blocked(cx)
            || self.model_navigation_blocked(cx)
        {
            return false;
        }
        self.stash_draft(cx);
        self.composer_generation += 1;
        self.attachments.clear();
        self.uploading = 0;
        self.stop_transcript();
        self.clear_workspace();
        let queue_watch = self.call(
            methods::WATCH_QUEUE,
            zuse_protocol::presentation::chat_request(&chat, json!({})),
        );
        self.queue_watch = Some(queue_watch);
        self.messages.clear();
        self.reset_context_status();
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
        self.prefetch_model_catalogs();
        self.begin_transcript(cx);
        self.refresh_model_config_watch();
        self.restore_draft(cx);
        if let Some(chat) = &self.selected {
            if let Some(harness) = chat["config"]["harness"].as_str() {
                if let Some(index) = self
                    .composer_providers()
                    .iter()
                    .position(|p| p["id"] == harness)
                {
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
        true
    }

    fn new_chat(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.require_connection(cx) {
            return false;
        }
        if self.file_navigation_blocked(cx)
            || self.queue_edit_navigation_blocked(cx)
            || self.model_navigation_blocked(cx)
        {
            return false;
        }
        self.stash_draft(cx);
        self.composer_generation += 1;
        self.attachments.clear();
        self.uploading = 0;
        self.stop_transcript();
        self.clear_workspace();
        self.reset_recovery(cx);
        self.reset_inputs(cx);
        self.reset_tool_results();
        self.reset_previews(cx);
        self.selected = None;
        self.prefetch_model_catalogs();
        self.refresh_model_config_watch();
        self.sandbox = self.preferences.default_permission_mode.sandbox();
        self.model = None;
        self.reasoning = None;
        self.model_options.clear();
        self.apply_default_model();
        self.messages.clear();
        self.reset_context_status();
        self.transcript_scroll.set_offset(point(px(0.), px(0.)));
        self.settings = false;
        self.restore_draft(cx);
        cx.notify();
        true
    }

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings {
            return;
        }
        if self.queue_edit.is_some() {
            self.commit_queue_edit(cx);
            return;
        }
        if self.model_change_pending_for_selection() {
            self.error = Some("Wait for the model choices to finish saving before sending.".into());
            cx.notify();
            return;
        }
        if self.access_change_pending_for_selection() {
            self.error = Some("Wait for the saved permission mode before sending.".into());
            cx.notify();
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
        if self.transcript_blocked() {
            self.transcript_state
                .notice
                .get_or_insert_with(|| "Wait for chat history to refresh before sending.".into());
            cx.notify();
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
        let Some(provider) = self.composer_provider().cloned() else {
            self.error = Some(if self.model_host.target.is_some() && self.model_host.confirmed {"This chat’s agent is unavailable on its execution host. Check that computer’s agent setup before sending."} else if self.model_host.target.is_some() {"Wait for agents on this chat’s execution host to refresh. Open the model picker to retry."} else {"Set up an agent in Settings first."}.into());
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
            host_id: self.draft_host(),
            not_sent: false,
        };
        self.drafts.remove(&draft.key);
        if self.is_working() {
            self.persist_outgoing_draft(draft.clone(), cx);
            let chat = self.selected.as_ref().unwrap();
            let id = self.call(methods::QUEUE_MESSAGE, zuse_protocol::presentation::chat_request(chat, json!({"text":text, "holdForTurnEnd":true, "attachments":self.attachment_paths()})));
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
        if self.selected.is_none() {
            draft.chat_id = Some(uuid::Uuid::new_v4().to_string());
        }
        self.persist_outgoing_draft(draft.clone(), cx);
        let command = json!({"kind":"run","messageId":message_id,"request":{"prompt":prompt,"harness":provider["id"],"model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options,"cwd":self.cwd(),"sandbox":self.sandbox,"autoApprove":self.sandbox==zuse_protocol::SandboxLevel::DangerFullAccess,"resume":null,"attachments":self.attachment_paths()}});
        let send = if let Some(chat) = &self.selected {
            self.call(
                methods::QUEUE_COMMAND,
                zuse_protocol::presentation::chat_request(&chat, json!({"command":command})),
            )
        } else {
            // IDs are client-owned, allowing the durable send to refer to the same chat.
            let id = draft.chat_id.clone().unwrap();
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
