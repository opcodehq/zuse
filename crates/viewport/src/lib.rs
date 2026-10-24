//! Browser-compatible GPUI views. Native process/filesystem access stays behind RPC.

mod access;
pub mod account_settings;
pub mod activity;
pub mod assets;
#[cfg(test)]
mod attachment_tests;
pub mod branding;
mod browser;
mod cloud;
mod cloud_billing;
pub mod cloud_billing_view;
mod cloud_history;
pub mod cloud_repositories;
pub mod cloud_settings;
pub mod cloud_queue;
mod composer_alerts;
pub mod composer_style;
pub mod creation_style;
pub mod creation_sources;
pub mod import_chat;
mod import_chat_adapter;
mod creation_checkout;
mod creation_source_adapter;
pub mod dither;
mod glass;
pub mod sidebar_controls;
mod sidebar_host;
pub mod workspace_style;
mod workspace_layout;
mod connection;
pub mod context_status;
mod custom_acp;
mod draft_journal;
mod environment;
pub mod git_controls;
mod history;
pub mod input;
mod managed_access;
pub mod remote_access_style;
mod model_catalog;
mod model_config;
mod model_picker;
pub mod model_picker_style;
mod navigation;
mod notifications;
mod pairing;
pub mod palette;
pub mod plugin_catalog;
pub mod plugin_detail;
mod plugin_management;
pub mod plugin_setup;
pub mod plugin_removal;
mod plugin_views;
mod previews;
mod project_actions;
pub mod project_archive;
pub mod usage_dashboard;
mod project_archive_host;
mod usage_host;
mod provider_settings;
mod provider_updates;
mod pull_requests;
mod questions;
mod queue;
mod queue_edit;
mod queue_render;
mod recovery;
pub mod settings_style;
mod settings_view;
mod shell;
pub mod styling;
mod terminals;
mod thread_actions;
mod title_settings;
mod tool_results;
mod transcript;
mod visuals;
mod voice;
mod voice_orb;
mod workspace;
mod workspace_review;
mod worktree_settings;

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
    cloud_archive: cloud_history::ArchiveView,
    cloud: cloud::Cloud,
    cloud_models: cloud::models::State,
    repository_panel: Entity<cloud_repositories::RepositoryPanel>,
    repository_requests: HashMap<u64, u64>,
    billing: cloud_billing::Billing,
    cloud_settings_focus: [FocusHandle; 2],
    cloud_credit_focus: [FocusHandle; 4],
    billing_cap: Entity<input::TextInput>,
    cloud_branch: Entity<input::TextInput>,
    cloud_sync_path: Entity<input::TextInput>,
    cached_only: bool,
    managed_access: managed_access::ManagedAccess,
    managed_name: Entity<input::TextInput>,
    voice: voice::Voice,
    environment: environment::Environment,
    browser: browser::Browser,
    visuals: visuals::Visuals,
    git_controls: Entity<git_controls::GitControls>,
    pull_requests: pull_requests::PullRequests,
    git_requests: HashMap<u64, u64>,
    _git_events: Subscription,
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
    terminal_host: Option<String>,
    terminal_tabs: Vec<terminals::TerminalTab>,
    terminal_open: Option<terminals::OpeningTerminal>,
    terminal_closing: HashMap<u64, terminals::TerminalTab>,
    project_actions: project_actions::ProjectActionsView,
    terminal_write: Option<(u64, String)>,
    error: Option<String>,
    show_archive: bool,
    thread_dialog: Option<thread_actions::ThreadDialog>,
    thread_action: Option<thread_actions::PendingThreadAction>,
    thread_title: Entity<input::TextInput>,
    thread_dialog_focus: FocusHandle,
    settings: bool,
    worktree_settings: worktree_settings::WorktreeSettingsView,
    title_settings: title_settings::TitleSettingsView,
    provider_expanded: Option<String>,
    custom_acp: custom_acp::Settings,
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
    account_remote: bool,
    pairing_origin: String,
    pairings: Value,
    pairing_name: Entity<input::TextInput>,
    pairing_fragment: Option<String>,
    pairing_fetch: Option<u64>,
    relay_status: Value,
    relay_fetch: Option<u64>,
    relay_action: Option<u64>,
    relay_origin: Entity<input::TextInput>,
    relay_credential: Entity<input::TextInput>,
    pairing_action: Option<u64>,
    remote_ui: pairing::RemoteUi,
    provider_index: usize,
    send_draft: HashMap<u64, Draft>,
    drafts: HashMap<String, Draft>,
    draft_journal_ready: bool,
    draft_journal_last: Option<Value>,
    draft_journal_error: Option<String>,
    creating: Option<(u64, Value, Value)>,
    creation_checkout: creation_checkout::State,
    creation_source_picker: Entity<creation_sources::SourcePicker>,
    import_picker: Entity<import_chat::ImportMenu>,
    import_requests: HashMap<u64,u64>,
    creation_source_requests: HashMap<u64,u64>,
    creation_source_checkout: Option<(String,String,String)>,
    queue: Vec<Value>,
    queue_watch: Option<u64>,
    queue_edit: Option<queue_edit::QueueEdit>,
    queue_actions: HashMap<u64, queue::PendingQueueAction>,
    queue_unconfirmed: std::collections::HashSet<String>,
    sessions: Vec<Value>,
    notification_baselines: HashMap<String, zuse_protocol::notifications::SessionNotificationState>,
    completion_preset_menu: bool,
    completion_preset_closed: Option<web_time::Instant>,
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
    sidebar_controls: Entity<sidebar_controls::SidebarControls>,
    sidebar_control_requests: HashMap<u64, u64>,
    sidebar_open: bool,
    sidebar_resize: Option<Option<u16>>,
    sidebar_resize_focus: FocusHandle,
    search_open: bool,
    session_search: Entity<input::TextInput>,
    archive_search: Entity<input::TextInput>,
    project_archive: project_archive_host::ArchiveRoute,
    usage_dashboard: Option<Entity<usage_dashboard::UsageDashboard>>,
    usage_open: bool,
    usage_pending: bool,
    usage_project: Option<String>,
    usage_project_scope: bool,
    usage_requests: std::collections::BTreeMap<u64,u64>,
    plugin_search: Entity<input::TextInput>,
    plugin_installed_search: Entity<input::TextInput>,
    plugins_browse: bool,
    plugins_return_right: Option<bool>,
    plugins_add_menu: bool,
    plugins_add_closed: Option<web_time::Instant>,
    plugin_detail: Option<String>,
    plugin_detail_menu: plugin_detail::Menu,
    plugin_connected_only: bool,
    plugin_catalog_page: usize,
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
    picker_menu: model_picker::view::MenuState,
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
    viewport_width: f32,
    content_height: f32,
    right_tab: String,
    history: history::History,
    review_scope: workspace_review::ReviewScope,
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
    account_action_error: Option<String>,
    account_card: Entity<account_settings::AccountCard>,
    _account_card_events: Subscription,
    browser_local: bool,
    plugin_state: zuse_protocol::plugin_state::PluginState,
    plugin_request: Option<u64>,
    plugin_poll: Option<Task<()>>,
    plugin_setup: Option<Entity<plugin_setup::PluginSetup>>,
    plugin_setup_events: Option<Subscription>,
    model: Option<String>,
    model_picker: bool,
    picker_target: model_picker::PickerTarget,
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
            input::TextInput::multiline(
                "Ask to make changes at the @ mentioned files or run slash commands…",
                window,
                cx,
            )
        });
        cx.observe(&input, |_, _, cx| cx.notify()).detach();
        let project_input =
            cx.new(|cx| input::TextInput::new("Absolute project path on this machine", window, cx));
        let archive_search =
            cx.new(|cx| input::TextInput::new("Filter archived chats", window, cx));
        cx.observe(&archive_search, |_, _, cx| cx.notify()).detach();
        let plugin_installed_search = cx.new(|cx| input::TextInput::new("Search", window, cx));
        cx.observe(&plugin_installed_search, |_, _, cx| cx.notify())
            .detach();
        let plugin_search = cx.new(|cx| input::TextInput::new("Search plugins", window, cx));
        cx.observe(&plugin_search, |this: &mut Self, _, cx| {
            this.plugin_catalog_page = 0;
            cx.notify();
        })
        .detach();
        let creation_source_picker=Self::new_creation_sources(cx);
        let import_picker=Self::new_import_menu(cx);
        let repository_panel = cx.new(|cx| cloud_repositories::RepositoryPanel::new(window, cx));
        cx.observe(&repository_panel, |_,_,cx|cx.notify()).detach();
        cx.subscribe(
            &repository_panel,
            |this, _, event: &cloud_repositories::Request, _cx| match event {
                cloud_repositories::Request::Call { id, method, params } => {
                    let remote = this.call(method, params.clone());
                    this.repository_requests.insert(remote, *id);
                }
                cloud_repositories::Request::Cancel { id } => {
                    if let Some(remote) = this
                        .repository_requests
                        .iter()
                        .find_map(|(remote, local)| (*local == *id).then_some(*remote))
                    {
                        this.repository_requests.remove(&remote);
                        this.cancel(remote);
                    }
                }
            },
        )
        .detach();
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
            |this, _, event: &input::Submitted, window, cx| {
                this.submit_composer(event.secondary, window, cx)
            },
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
        let colors = plugin_setup::SetupColors {
            text: palette::rgb(0xe8e9e7).into(),
            muted: palette::rgb(0xa4a8a3).into(),
            background: palette::rgb(0x141414).into(),
            border: palette::rgb(0x343434).into(),
            danger: palette::rgb(0xe5a0a0).into(),
        };
        let git_controls = cx.new(|cx| git_controls::GitControls::new(colors, window, cx));
        let git_events = cx.subscribe(
            &git_controls,
            |this, _, event: &git_controls::Request, cx| {
                if !this.connected {
                    this.git_controls.update(cx, |panel, cx| {
                        panel.respond(
                            event.id,
                            Err("Reconnect before using Git controls.".into()),
                            cx,
                        )
                    });
                    return;
                }
                let id = this.call(event.method, event.params.clone());
                this.git_requests.insert(id, event.id);
            },
        );
        let pull_requests = pull_requests::PullRequests::new(window, cx);
        let reasoning_focus = cx.focus_handle();
        let file_editor = cx.new(|cx| input::TextInput::code(window, cx));
        cx.observe(&file_editor, |_, _, cx| cx.notify()).detach();

        let recovery_output = cx.new(|cx| {
            input::TextInput::multiline("Describe the result you inspected…", window, cx)
        });
        let input_text = cx.new(|cx| input::TextInput::multiline("Write an answer…", window, cx));
        let thread_title = cx.new(|cx| input::TextInput::new("Thread name", window, cx));
        cx.subscribe_in(
            &thread_title,
            window,
            |this, _, _: &input::Submitted, _, cx| {
                this.commit_thread_rename(cx);
            },
        )
        .detach();
        let relay_origin =
            cx.new(|cx| input::TextInput::new("https://relay.example.com", window, cx));
        let relay_credential = cx.new(|cx| {
            input::TextInput::new("Host credential file on the engine computer", window, cx)
        });
        let managed_name = cx.new(|cx| input::TextInput::new("Computer name", window, cx));
        let pairing_name = cx.new(|cx| input::TextInput::new("Device name", window, cx));
        let remote_ui = pairing::RemoteUi::new(window, cx);
        let mut events = transport.events;
        let reader = cx.spawn(async move |this, cx| {
            while let Some(frame) = events.next().await {
                if this.update(cx, |this, cx| this.receive(frame, cx)).is_err() {
                    break;
                }
            }
            let _ = this.update(cx, |this, cx| {
                this.status = "Disconnected".into();
                this.connected = false;
                let failed = std::mem::take(&mut this.usage_requests);
                for (remote,local) in failed {
                    this.pending.remove(&remote);
                    if let Some(dashboard) = this.usage_dashboard.clone() { dashboard.update(cx,|dashboard,cx| dashboard.receive(local,Err("Computer disconnected. Reconnect and refresh usage.".into()),cx)); }
                }
                this.sidebar_controls
                    .update(cx, |controls, cx| controls.interrupted(cx));
                for remote in this
                    .repository_requests
                    .drain()
                    .map(|(remote, _)| remote)
                    .collect::<Vec<_>>()
                {
                    this.pending.remove(&remote);
                }
                this.repository_panel
                    .update(cx, |panel, cx| panel.interrupted(cx));
                cx.notify();
            });
        });
        let worktree_settings = worktree_settings::WorktreeSettingsView::new(window, cx);
        let browser = browser::Browser::new(window, cx);
        let account_card=cx.new(|cx|account_settings::AccountCard::new(window,cx));
        let account_card_events=cx.subscribe(&account_card,|this,_,event,cx|{
            use account_settings::Event;
            match event {
                Event::SignIn|Event::SignOut=>{
                    if !this.connected || this.pending.values().any(|method|matches!(method.as_str(),methods::CLOUD_ACCOUNT_SIGN_IN|methods::CLOUD_ACCOUNT_SIGN_OUT)) {return;}
                    this.account_action_error=None;
                    this.call(if matches!(event,Event::SignIn){methods::CLOUD_ACCOUNT_SIGN_IN}else{methods::CLOUD_ACCOUNT_SIGN_OUT},json!({}));
                }
                Event::DisplayName{account,value}=>{
                    if this.cloud_account["user"]["id"]!=account.as_str() {return;}
                    this.preferences.display_name=value.clone();this.save_preferences();
                }
            }
            cx.notify();
        });
        let cloud_sync_path = cx.new(|cx| input::TextInput::new("Absolute folder on host", window, cx));
        let cloud_branch = cx.new(|cx| input::TextInput::new("Branch (main)", window, cx));
        let sidebar_controls = Self::create_sidebar_controls(cx);
        let view = Self {
            environment: Default::default(),
            browser,
            workspace_focus,
            input,
            cloud_archive: Default::default(),
            cloud: Default::default(),
            cloud_models: Default::default(),
            repository_panel,
            repository_requests: HashMap::new(),
            billing: Default::default(),
            cloud_settings_focus: std::array::from_fn(|_| cx.focus_handle()),
            cloud_credit_focus: std::array::from_fn(|_| cx.focus_handle()),
            billing_cap: cx.new(|cx| input::TextInput::new("Monthly overage cap in dollars", window, cx)),
            cloud_branch,
            cloud_sync_path,
            cached_only: false,
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
            terminal_host: None,
            terminal_tabs: vec![],
            terminal_open: None,
            terminal_closing: HashMap::new(),
            project_actions: project_actions::ProjectActionsView::default(),
            terminal_write: None,
            error: None,
            show_archive: false,
            thread_dialog: None,
            thread_action: None,
            thread_title,
            thread_dialog_focus: cx.focus_handle(),
            settings: false,
            worktree_settings,
            title_settings: title_settings::TitleSettingsView::new(window, cx),
            provider_expanded: None,
            custom_acp: Default::default(),
            provider_actions: HashMap::new(),
            provider_logins: HashMap::new(),
            provider_codes: HashMap::new(),
            provider_error: None,
            settings_section: settings_view::Section::General,
            settings_nav_focus: cx.focus_handle().tab_stop(true),
            settings_nav_active: 0,
            provider_updates: vec![],
            provider_updates_watch: None,
            provider_updates_action: None,
            provider_updates_cancel: None,
            provider_updates_confirmed: false,
            provider_updates_error: None,
            paired: false,
            account_remote: false,
            managed_access: Default::default(),
            managed_name,
            pairing_origin: String::new(),
            pairings: Value::Null,
            pairing_name,
            pairing_fragment: None,
            pairing_fetch: None,
            relay_status: Value::Null,
            relay_fetch: None,
            relay_action: None,
            relay_origin,
            relay_credential,
            pairing_action: None,
            remote_ui,
            provider_index: 0,
            send_draft: HashMap::new(),
            drafts: HashMap::new(),
            draft_journal_ready: false,
            draft_journal_last: None,
            draft_journal_error: None,
            creating: None,
            creation_checkout: Default::default(),
            creation_source_picker,
            creation_source_requests: Default::default(),
            import_picker,
            import_requests: Default::default(),
            creation_source_checkout: None,
            queue: vec![],
            queue_watch: None,
            queue_edit: None,
            queue_actions: HashMap::new(),
            queue_unconfirmed: Default::default(),
            sessions: vec![],
            notification_baselines: HashMap::new(),
            completion_preset_menu: false,
            completion_preset_closed: None,
            completion_preset_active: 0,
            completion_preset_focus: cx.focus_handle().tab_stop(true),
            completion_switch_focus: cx.focus_handle().tab_stop(true),
            provider_notice_focus: cx.focus_handle().tab_stop(true),
            provider_policy_focus: cx.focus_handle(),
            repos: vec![],
            spaces: vec![],
            project: None,
            project_picker: false,
            project_input,
            sidebar_controls,
            sidebar_control_requests: Default::default(),
            sidebar_open: true,
            sidebar_resize: None,
            sidebar_resize_focus: cx.focus_handle().tab_stop(true),
            search_open: false,
            session_search,
            plugin_search,
            plugin_installed_search,
            plugins_browse: false,
            plugins_return_right: None,
            plugins_add_menu: false,
            plugins_add_closed: None,
            plugin_detail: None,
            plugin_detail_menu: Default::default(),
            archive_search,
            project_archive: Default::default(),
            usage_dashboard: None,
            usage_open: false,
            usage_pending: false,
            usage_project: None,
            usage_project_scope: false,
            usage_requests: Default::default(),
            plugin_connected_only: false,
            plugin_catalog_page: 0,
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
            picker_menu: Default::default(),
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
            viewport_width: 1280.,
            content_height: 880.,
            right_tab: "files".into(),
            history: history::History::new(window, cx),
            review_scope: Default::default(),
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
            account_action_error: None,
            account_card,
            _account_card_events: account_card_events,
            browser_local: false,
            plugin_state: Default::default(),
            plugin_request: None,
            plugin_poll: None,
            plugin_setup: None,
            plugin_setup_events: None,
            model: None,
            model_picker: false,
            picker_target: model_picker::PickerTarget::Composer,
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
            voice: voice::Voice::default(),
            visuals: visuals::Visuals::default(),
            git_controls,
            pull_requests,
            git_requests: HashMap::new(),
            _git_events: git_events,
            _reader: reader,
        };
        view
    }

    fn plugin_action(&mut self, request: Value) {
        self.plugin_request_with(methods::PLUGINS, request);
    }
    // Kept for the organization workspace picker, hidden for now.
    #[cfg_attr(not(test), allow(dead_code))]
    fn plugin_select_workspace(&mut self, tenant: Value) {
        self.plugin_request_with(
            methods::SELECT_PLUGIN_WORKSPACE,
            json!({"action":"list","tenantId":tenant}),
        );
    }
    fn plugin_request_with(&mut self, method: &'static str, request: Value) {
        if self.plugin_request.is_some() {
            return;
        }
        if !self.connected {
            return;
        }
        let prepared = if method == methods::SELECT_PLUGIN_WORKSPACE {
            self.plugin_state.begin_workspace_selection(request)
        } else {
            self.plugin_state.begin(request)
        };
        let request = match prepared {
            Ok(request) => request,
            Err(error) => {
                self.plugin_state.notice = Some(error.into());
                return;
            }
        };
        self.plugin_setup = None;
        self.plugin_setup_events = None;
        self.plugin_request = Some(self.call(method, request));
    }

    fn call(&mut self, method: &str, params: Value) -> u64 {
        if self.cached_only {
            return 0;
        }
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
        if self.receive_creation_source(&frame,cx){return;}
        if self.receive_import_chat(&frame,cx){return;}
        if frame["id"]
            .as_u64()
            .and_then(|id| self.pending.get(&id))
            .is_some_and(|m| m == methods::CLOUD_ACCOUNT_STATUS)
        {
            if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
                let owner = value["user"]["id"]
                    .as_str()
                    .map(|id| (id.to_owned(), value["orgId"].as_str().map(str::to_owned)));
                self.repository_panel
                    .update(cx, |panel, cx| panel.bind_owner(owner, cx));
            }
        }
        if let Some((remote, local)) = frame["id"].as_u64().and_then(|id| {
            self.repository_requests
                .remove(&id)
                .map(|local| (id, local))
        }) {
            self.pending.remove(&remote);
            let result = if let Some(error) = frame["err"].as_str() {
                Err(error.to_owned())
            } else {
                frame
                    .get("ok")
                    .or_else(|| frame.get("item"))
                    .cloned()
                    .ok_or_else(|| "Repository response was interrupted.".into())
            };
            self.repository_panel
                .update(cx, |panel, cx| panel.receive(local, result, cx));
            return;
        }
        if self.receive_billing(&frame, cx) {
            return;
        }
        if self.receive_cloud(&frame, cx) {
            return;
        }
        if self.receive_project_archives(&frame,cx) {return;}
        if self.receive_cloud_archive(&frame, cx) {
            return;
        }
        if self.receive_environment(&frame, cx) {
            return;
        }
        if self.receive_browser(&frame, cx) {
            return;
        }
        if self.receive_voice_media(&frame, cx) {
            return;
        }
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
        if let Some(message) = frame["accountConnectionError"].as_str() {
            self.account_connection_error(message);
            cx.notify();
            return;
        }
        if let Some(account) = frame["accountRemote"].as_bool() {
            self.account_remote = account;
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
        if self.receive_browser_previews(id, &frame, cx) {
            return;
        }
        if self.receive_visual(id, &frame, cx) {
            return;
        }
        if self.receive_voice_rpc(id, &frame, cx) {
            return;
        }
        if self.receive_project_actions(id, &frame, cx) {
            return;
        }
        if self.receive_terminal_close(id, &frame, cx) {
            return;
        }
        if self.receive_terminal_open(id, &frame, cx) {
            return;
        }
        if self.receive_title_settings(id, &frame, cx) {
            return;
        }
        if self.receive_worktree_settings(id, &frame, cx) {
            return;
        }
        if let Some(panel_id) = self.git_requests.get(&id).copied() {
            if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true {
                self.git_requests.remove(&id);
                self.pending.remove(&id);
                let result = frame.get("ok").cloned().ok_or_else(|| {
                    frame["err"]
                        .as_str()
                        .unwrap_or("Git response was interrupted.")
                        .to_owned()
                });
                self.git_controls
                    .update(cx, |panel, cx| panel.respond(panel_id, result, cx));
                if method == methods::GIT_WORKSPACE_ACTION && frame.get("ok").is_some() {
                    self.refresh_changes(cx);
                    self.reset_history(cx);
                }
            }
            return;
        }
        if self.receive_usage(id, &frame, cx) { return; }
        if self.receive_sidebar_controls(id, &frame, cx) {
            return;
        }
        if self.receive_pull_requests(id, &frame, cx) {
            return;
        }
        if self.receive_thread_action(id, &frame, cx) {
            return;
        }
        if self.receive_picker_catalog(id, &frame, cx) {
            return;
        }
        if self.receive_file_reconcile(id, &frame, cx) {
            return;
        }
        if self.receive_history(id, &frame, cx) {
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
        if self.receive_managed_access(id, &frame, cx) {
            return;
        }
        if self.receive_pairing(id, &frame, cx) {
            return;
        }
        if self.receive_provider_updates(id, &frame, cx) {
            return;
        }
        if self.receive_custom_acp(id, &frame, cx) {
            return;
        }
        if self.receive_provider_action(id, &frame, cx) {
            return;
        }
        if method == methods::WATCH_DOC_MESSAGES {
            self.receive_transcript(id, &frame, cx);
            return;
        }
        if matches!(
            method.as_str(),
            methods::PLUGINS | methods::SELECT_PLUGIN_WORKSPACE
        ) {
            self.receive_plugins(id, &frame, cx);
            return;
        }
        if let Some(error) = frame.get("err").and_then(Value::as_str) {
            if matches!(method.as_str(),methods::CLOUD_ACCOUNT_SIGN_IN|methods::CLOUD_ACCOUNT_SIGN_OUT|methods::CLOUD_ACCOUNT_STATUS) {
                self.account_action_error=Some(error.into());
            }
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
                methods::CREATE_THREAD_SESSION => {
                    if let Some(chat) = frame.get("ok").filter(|chat| chat["id"].is_string()) {
                        if !self.chats.iter().any(|row| row["id"] == chat["id"]) {
                            self.chats.push(chat.clone());
                        }
                        self.select(chat.clone(), cx);
                    }
                }

                methods::CLOUD_ACCOUNT_SIGN_IN => {
                    self.account_action_error=None;
                    if let Some(url) = value["url"]
                        .as_str()
                        .filter(|url| url.starts_with("https://"))
                    {
                        cx.open_url(url);
                    }
                }
                methods::CLOUD_ACCOUNT_STATUS => {
                    let changed = self.cloud_account["user"]["id"] != value["user"]["id"];
                    self.cloud_account = value.clone();
                    self.update_sidebar_controls(cx);
                    if changed || value["login"].is_object() {self.account_action_error=None;}
                    self.managed_account_updated(changed);
                    if changed {
                        self.plugin_detail = None;
                        self.plugin_detail_menu = Default::default();
                        self.plugins_add_menu = false;
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
                    if let Some(chat) = self
                        .cloud
                        .initial_session
                        .as_deref()
                        .and_then(|id| self.chats.iter().find(|c| c["id"] == id))
                        .cloned()
                    {
                        if self.select(chat, cx) {
                            self.cloud.initial_session = None;
                        }
                    }
                    if let Some(selected) = &self.selected {
                        if let Some(chat) = self
                            .chats
                            .iter()
                            .find(|chat| chat["id"] == selected["id"])
                            .cloned()
                        {
                            let host_changed = chat["deviceId"] != selected["deviceId"];
                            let checkout_changed = host_changed || chat["cwd"] != selected["cwd"];
                            let review_changed =
                                checkout_changed || chat["archived"] != selected["archived"];
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
                            if review_changed {
                                if checkout_changed {
                                    self.review_scope = Default::default();
                                }
                                self.rebind_history_review(cx);
                            }
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
            || self.project_action_navigation_blocked(cx)
        {
            return false;
        }
        self.close_usage(cx);
        self.close_project_archives();
        self.close_plugins_browse();
        self.stash_draft(cx);
        self.creation_source_picker.update(cx,|picker,cx|picker.set_target(None,cx));
        self.creation_source_checkout=None;
        self.composer_generation += 1;
        self.attachments.clear();
        self.uploading = 0;
        self.stop_transcript();
        self.clear_workspace(cx);
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
        self.reset_visuals();
        self.reset_browser(cx);
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
        if self.right_open && self.right_tab == "browser" {
            self.watch_browser_previews();
        }
        if self.right_open && self.right_tab == "history" {
            self.refresh_history(cx);
        }
        if self.right_open && self.right_tab == "changes" {
            self.refresh_changes(cx);
        }
        self.settings = false;
        self.error = None;
        cx.notify();
        true
    }

    fn new_chat(&mut self, cx: &mut Context<Self>) -> bool {
        self.creation_checkout.dismiss();
        self.close_usage(cx);
        self.close_project_archives();
        if !self.require_connection(cx) {
            return false;
        }
        if self.file_navigation_blocked(cx)
            || self.queue_edit_navigation_blocked(cx)
            || self.model_navigation_blocked(cx)
            || self.project_action_navigation_blocked(cx)
        {
            return false;
        }
        self.close_plugins_browse();
        self.stash_draft(cx);
        self.creation_source_picker.update(cx,|picker,cx|picker.set_target(None,cx));
        self.creation_source_checkout=None;
        self.composer_generation += 1;
        self.attachments.clear();
        self.uploading = 0;
        self.stop_transcript();
        self.clear_workspace(cx);
        self.reset_recovery(cx);
        self.reset_inputs(cx);
        self.reset_tool_results();
        self.reset_visuals();
        self.reset_browser(cx);
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
        if self.import_picker.read(cx).busy() {
            return;
        }
        if self.project_archive.project.is_some() || self.usage_open {return;}
        if self.cloud.cached {
            if !self.input.read(cx).value(cx).trim().is_empty() || !self.attachments.is_empty() {
                let _ = self.requests.unbounded_send(json!({"ui":"resumeCloud","send":true}));
            }
            return;
        }
        if self.submit_cloud_delete(window, cx) {
            return;
        }
        if self.browser_input_focused(window, cx) {
            return;
        }
        if self.browser_voice_replaces_composer() {
            return;
        }
        if self.thread_dialog.is_some() {
            return;
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|chat| chat["archived"] == true)
        {
            self.error = Some("Unarchive this thread before sending another message.".into());
            cx.notify();
            return;
        }
        if self.settings {
            return;
        }
        if self.cloud_startup_pending() {
            return;
        }
        if self.cloud.mode && self.selected.is_none() {
            if self.creation_source_picker.read(cx).is_preparing() {
                return;
            }
            self.stage_creation_issue(cx);
            self.submit_cloud(cx);
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
        if self.selected.is_none() && self.creation_source_picker.read(cx).is_preparing() { return; }
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
        self.stage_creation_issue(cx);
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
        let command = json!({"kind":"run","messageId":message_id,"request":{"prompt":prompt,"harness":provider["id"],"model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options,"cwd":self.cwd(),"sandbox":self.sandbox,"autoApprove":self.sandbox==zuse_protocol::SandboxLevel::DangerFullAccess,"resume":null,"attachments":self.attachment_paths(),"worktree":self.creation_worktree()}});
        let send = if let Some(chat) = &self.selected {
            self.call(
                methods::QUEUE_COMMAND,
                zuse_protocol::presentation::chat_request(&chat, json!({"command":command})),
            )
        } else {
            // IDs are client-owned, allowing the durable send to refer to the same chat.
            let id = draft.chat_id.clone().unwrap();
            let request = self.call(methods::MUTATE, json!({"op":"createChat","chatId":id,"deviceId":device,"cwd":self.cwd(),"branch":self.selected_creation_checkout().map(|(_,branch)|branch),"spaceId":self.project.as_ref().and_then(|p|self.spaces.iter().find(|s|s.path==p["path"])).map(|s|&s.id),"config":{"harness":provider["id"],"model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options,"sandbox":self.sandbox}}));
            let chat = json!({"id":id,"title":text.chars().take(70).collect::<String>(),"deviceId":device,"cwd":self.cwd(),"branch":self.selected_creation_checkout().map(|(_,branch)|branch),"spaceId":self.project.as_ref().and_then(|p|self.spaces.iter().find(|s|s.path==p["path"])).map(|s|&s.id),"config":{"harness":provider["id"],"model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options,"sandbox":self.sandbox}});
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

struct WorkspaceInitialized;
impl Global for WorkspaceInitialized {}

pub fn initialize(cx: &mut App) {
    if cx.has_global::<WorkspaceInitialized>() {
        return;
    }
    cx.set_global(WorkspaceInitialized);
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

    let fonts: &[&[u8]] = &[
        include_bytes!("../../ui/assets/fonts/Geist.ttf"),
        include_bytes!("../../ui/assets/fonts/Geist-Italic.ttf"),
        include_bytes!("../../ui/assets/fonts/Geist-Medium.ttf"),
        include_bytes!("../../ui/assets/fonts/Geist-MediumItalic.ttf"),
        include_bytes!("../../ui/assets/fonts/Geist-SemiBold.ttf"),
        include_bytes!("../../ui/assets/fonts/Geist-SemiBoldItalic.ttf"),
        include_bytes!("../../ui/assets/fonts/Geist-Bold.ttf"),
        include_bytes!("../../ui/assets/fonts/Geist-BoldItalic.ttf"),
        include_bytes!("../../ui/assets/fonts/GeistMono.ttf"),
        include_bytes!("../../ui/assets/fonts/GeistMono-Italic.ttf"),
        include_bytes!("../../ui/assets/fonts/GeistMono-Medium.ttf"),
        include_bytes!("../../ui/assets/fonts/GeistMono-MediumItalic.ttf"),
        include_bytes!("../../ui/assets/fonts/GeistMono-SemiBold.ttf"),
        include_bytes!("../../ui/assets/fonts/GeistMono-SemiBoldItalic.ttf"),
        include_bytes!("../../ui/assets/fonts/GeistMono-Bold.ttf"),
        include_bytes!("../../ui/assets/fonts/GeistMono-BoldItalic.ttf"),
    ];
    cx.text_system()
        .add_fonts(
            fonts.iter().map(|font| std::borrow::Cow::Borrowed(*font)).collect(),
        )
        .expect("bundled interface fonts");
}
