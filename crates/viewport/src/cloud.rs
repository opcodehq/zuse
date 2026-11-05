//! Cloud launch and environment selection. Only interactive connection acquires compute.
mod account_events;
mod agent_caption;
mod allocation_retry;
mod commands;
mod computer_controls;
mod computer_menu;
mod creation;
pub(super) mod models;
pub(crate) mod preparation;
mod project_start;
mod recreation;
mod repair;
mod retained_history;
mod sync;
mod transport;
mod uploads;
use super::*;
use crate::assets::icon;
use crate::shell::{BG, BORDER, MUTED, column, control, icon_button, label, row};

#[derive(Clone)]
pub(super) struct LocalNavigation {
    pub chats: Vec<Value>,
    pub spaces: Vec<zuse_protocol::Space>,
    pub device: String,
    pub epoch: u64,
}
#[derive(Default)]
pub(super) struct Cloud {
    pub mode: bool,
    command_api_ready: bool,
    pub(super) command_admissions: Vec<Value>,
    pub(super) response_wait: Option<Value>,
    queue_paused: Option<bool>,
    reviewed_commands: std::collections::HashSet<(String, String)>,
    registered_conversations: Vec<Value>,
    admission_overlay: std::collections::HashSet<String>,
    command_api_version: Option<u64>,
    account_events: account_events::State,
    sync: sync::SyncUi,
    repair: repair::State,
    allocation_retry: allocation_retry::State,
    recreation: recreation::State,
    pub guest: bool,
    pub guest_status: Value,
    pub preparation_open: bool,
    pub cached: bool,
    pub cached_run_selection: Option<(
        String,
        String,
        zuse_protocol::cloud_task_commands::RunSelection,
    )>,
    pub local_navigation: Option<LocalNavigation>,
    cached_drafts: HashMap<String, Draft>,
    refresh_cached: bool,
    pub resume_draft: Option<(String, String, Vec<Value>, bool)>,
    resume_send: bool,
    pub(super) local_terminals: bool,
    pub picker: Option<&'static str>,
    creation: creation::State,
    pub target: Option<Value>,
    pub(super) account: Option<String>,
    pub(super) organization: Option<String>,
    namespace: Option<String>,
    heads: HashMap<String, (zuse_model::cloud::Head, web_time::Instant)>,
    head_verified: Option<web_time::Instant>,
    pub(super) projects: Vec<Value>,
    provider_catalogs: Value,
    capabilities: Value,
    compute_provider: Option<String>,
    compute_size: Option<String>,
    authority: bool,
    connections: Vec<Value>,
    connection: Option<String>,
    pub workspaces: Vec<Value>,
    project: usize,
    project_id: Option<String>,
    context: Option<u64>,
    list: Option<u64>,
    create: Option<u64>,
    pause: Option<u64>,
    pause_command: Option<String>,
    pause_session: Option<String>,
    metadata: Option<u64>,
    metadata_failures: usize,
    poll: Option<Task<()>>,
    pub guest_workspace: Option<String>,
    launch: Option<Value>,
    accepted: Option<Value>,
    queued_prompt: Option<crate::cloud_queue::Prompt>,
    status_request: Option<u64>,
    status: Value,
    history_request: Option<u64>,
    cache_request: Option<u64>,
    head: Option<zuse_model::cloud::Head>,
    pub older_before: Option<i64>,
    pub older_loading: bool,
    prepended_history: bool,
    retained_history_refresh: bool,
    child: Option<Entity<WorkspaceView>>,
    events: Option<UnboundedSender<Value>>,
    cached_events: Option<UnboundedSender<Value>>,
    read_forwards: HashMap<u64, u64>,
    pump: Option<Task<()>>,
    forwards: HashMap<u64, (u64, String)>,
    uploads: HashMap<u64, Value>,
    viewport: String,
    connected_epoch: Option<u64>,
    reconnect_transport: bool,
    pub initial_session: Option<String>,
    pub metadata_thread: Option<String>,
    pub admitted_title: Option<(String, String)>,
    notice: Option<String>,
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use std::prelude::v1::test;

    #[gpui::test]
    fn durable_admission_opens_history_before_the_runtime_and_never_requeues_the_prompt(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx, |view, window, cx| {
            let request = json!({"firstMessage":"Inspect the repository", "agent":"codex", "model":"selected-model",
                "attachmentTransfers":[{"fileName":"notes.txt"}]});
            view.cloud.create = Some(42);
            view.cloud.launch = Some(request.clone());
            view.cloud.queued_prompt = Some(crate::cloud_queue::Prompt::from_request(&request));
            view.receive_cloud(&json!({"id":42,"ok":{"workspace":{"workspaceId":"computer-a",
                "chatId":"thread-a","initialSessionId":"session-a","state":"starting","createdAt":12}}}), cx);
            assert!(view.cloud.head.is_none());
            view.connected = true;
            view.device_id = Some("local-device".into());
            view.connection_epoch = 7;
            view.chats = vec![json!({"id":"local-chat","deviceId":"local-device","title":"Local conversation"})];
            view.cached_cloud_view(window, cx);
            let first_child = view.cloud.child.clone().unwrap();
            view.cached_cloud_view(window, cx);
            assert!(!first_child.read(cx).requests.is_closed(), "History refresh must preserve the child's parent-authority request channel");
            let child = view.cloud.child.as_ref().expect("durable admission must open a conversation before a guest checkpoint");
            let child = child.read(cx);
            assert_eq!(child.selected.as_ref().unwrap()["id"], "session-a");
            assert_eq!(child.selected.as_ref().unwrap()["threadId"], "thread-a");
            assert!(child.cloud.cached);
            assert!(!child.cached_only);
            assert!(!child.transcript_blocked());
            let navigation = child.cloud.local_navigation.as_ref().unwrap();
            assert_eq!(navigation.chats[0]["id"], "local-chat");
            assert_eq!(navigation.device, "local-device");
            assert_eq!(navigation.epoch, 7);
            assert!(!child.chats.iter().any(|chat| chat["id"] == "local-chat"), "Local navigation is presentation only, outside the cloud execution document");
            assert_eq!(child.messages.len(), 1);
            assert!(matches!(&child.messages[0].parts[0], zuse_model::MessagePart::Text { text, .. }
                if text == "Inspect the repository\n\nAttachments: notes.txt"));
        }).unwrap();
        window.update(cx, |view, _, cx| {
            view.connected = false;
            assert!(view.handle_parent_navigation(&json!({"ui":"selectLocalConversation","chatId":"local-chat","deviceId":"local-device","epoch":7,"accountId":null,"organizationId":null}), cx));
            assert!(view.cloud.child.is_some(), "Offline local selection must leave cloud history open");
            assert!(view.cloud.child.as_ref().unwrap().read(cx).error.as_deref().unwrap().contains("local computer is unavailable"));
            assert!(view.selected.as_ref().is_none_or(|chat| chat["id"] != "local-chat"));
        }).unwrap();
        while let Ok(frame) = sent.try_recv() {
            assert_ne!(
                frame["method"],
                methods::QUEUE_MESSAGE,
                "the first prompt belongs to the admitted server command"
            );
            assert_ne!(
                frame["method"],
                methods::CLOUD_CREATE,
                "rendering must not repeat allocation"
            );
        }
    }

    #[gpui::test]
    fn reopening_running_history_attaches_passively_but_saved_or_sleeping_history_does_not(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx, |view, _, cx| {
            let head = json!({"schemaVersion":1,"payloadFormat":"zuse-cloud-admission-v1",
                "workspaceId":"workspace","sessionId":"initial","runtimeGeneration":1,
                "cursor":{"epoch":"epoch","version":1},
                "chat":{"id":"initial","deviceId":"cloud:workspace","archived":false,"createdAt":"2026-01-01T00:00:00Z","threadId":"thread"},
                "conversations":[],"transcript":{"reset":[]},"queue":[],"queuePaused":false});
            assert!(serde_json::from_value::<zuse_model::cloud::Head>(head.clone()).is_ok());
            for variant in ["running","cached","provisional","paused","foreign-workspace","foreign-account","older-page"] {
                view.cloud=Cloud::default();
                view.cloud.account=Some("owner".into());
                view.cloud.target=Some(json!({"workspaceId":"workspace","initialSessionId":"initial","state":"ready"}));
                view.cloud.history_request=Some(900);
                let mut value=json!({"accountId":"owner","workspace":{"workspaceId":"workspace","state":"ready"},"head":head});
                match variant {
                    "provisional"=>value["provisional"]=json!(true),
                    "paused"=>value["workspace"]["state"]=json!("paused"),
                    "foreign-workspace"=>value["workspace"]["workspaceId"]=json!("another-workspace"),
                    "foreign-account"=>value["accountId"]=json!("another-owner"),
                    "older-page"=>value["historyPage"]=json!(true),
                    _=>{}
                }
                assert!(view.receive_cloud(&json!({"id":900,"ok":value,"cachedHistory":variant=="cached"}),cx));
                let mut attachments=vec![];
                while let Ok(frame)=sent.try_recv() {
                    if frame["method"]==methods::CLOUD_ENVIRONMENT {attachments.push(frame);}
                    else {assert_ne!(frame["method"],methods::CLOUD_CREATE);assert_ne!(frame["method"],methods::CLOUD_TASK_COMMAND);}
                }
                if variant=="running" {
                    assert_eq!(attachments.len(),1);
                    assert_eq!(attachments[0]["params"]["interactiveResume"],false);
                    assert_eq!(attachments[0]["params"]["workspaceId"],"workspace");
                    view.cloud.history_request=Some(901);
                    view.receive_cloud(&json!({"id":901,"ok":value}),cx);
                    assert!(sent.try_recv().is_err(),"Repeated history updates keep one attachment");
                } else {
                    assert!(attachments.is_empty(),"{variant} history must not acquire a runtime connection");
                }
            }
            view.cloud.status_request=None;
            view.connect_cloud();
            let explicit=sent.try_recv().unwrap();
            assert_eq!(explicit["method"],methods::CLOUD_ENVIRONMENT);
            assert_eq!(explicit["params"]["interactiveResume"],true,"Only an explicit user action can request resume");
        }).unwrap();
    }
}
impl WorkspaceView {
    fn open_selected_cloud_agent(&mut self, cx: &mut Context<Self>) {
        self.settings = true;
        self.billing.tab = crate::cloud_settings::Tab::Setup;
        self.cloud.sync.open = false;
        self.select_settings_section(crate::settings_view::Section::Cloud, cx);
        if let (Some(account), Some(target)) =
            (self.cloud.account.clone(), self.cloud.target.clone())
        {
            if let (Some(agent), Some(connection)) = (
                target["agent"].as_str(),
                target["agentConnectionId"].as_str(),
            ) {
                let owner = (account, self.cloud.organization.clone());
                let (agent, connection) = (agent.to_owned(), connection.to_owned());
                self.repository_panel.update(cx, |panel, cx| {
                    panel.focus_agent_connection(owner, agent, connection, cx)
                });
            } else {
                self.error=Some("This computer's selected coding agent could not be verified. Refresh its details; no other account was selected.".into());
            }
        }
        cx.notify();
    }
    fn cloud_guest_status(&self) -> Value {
        let mut state = preparation::merge_status(
            self.cloud.target.as_ref().unwrap_or(&Value::Null),
            &self.cloud.status,
        );
        state["commandApiAuthorized"] = json!(self.cloud.command_api_ready);
        state["preparationLog"] = self.cloud.sync.logs["log"].clone();
        state["preparationLogError"] = json!(self.cloud.sync.log_error);
        state
    }
    fn pause_cloud(&mut self, session: Option<String>) {
        if self.cloud.pause.is_some() {
            return;
        }
        let Some(target) = &self.cloud.target else {
            return;
        };
        let command = self
            .cloud
            .pause_command
            .get_or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        let body = json!({"workspaceId":target["workspaceId"],"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization,"commandId":command});
        self.cloud.pause_session = session;
        self.cloud.pause = Some(self.call(methods::CLOUD_PAUSE, body));
        self.cloud.notice = Some("Pausing cloud workspace…".into());
    }
    fn watch_cloud_pause(&mut self, cx: &mut Context<Self>) {
        if self.cloud.account_events.live {
            return;
        }
        self.cloud.poll=Some(cx.spawn(async move|this,cx| {
            for _ in 0..120 {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                if !this.update(cx,|this,_| {
                    if this.cloud.account_events.live || this.cloud.target.as_ref().is_none_or(|t|t["state"]!="pausing") {return false;}
                    if this.cloud.metadata.is_none() {
                        let body=json!({"workspaceId":this.cloud.target.as_ref().unwrap()["workspaceId"],"expectedAccountId":this.cloud.account,"expectedOrganizationId":this.cloud.organization});
                        this.cloud.metadata=Some(this.call(methods::CLOUD_WORKSPACE,body));
                    }
                    true
                }).unwrap_or(false) {return;}
            }
        }));
    }
    pub(super) fn refresh_cloud(&mut self) {
        if self.cloud.context.is_none() {
            self.cloud.context = Some(self.call(methods::CLOUD_CONTEXT, json!({})));
        }
        if self.cloud.list.is_none() {
            self.cloud.list = Some(self.call(methods::CLOUD_WORKSPACES, json!({})));
        }
    }
    fn connect_cloud(&mut self) {
        self.connect_cloud_with_intent(true);
    }
    fn connect_cloud_with_intent(&mut self, interactive_resume: bool) {
        if matches!(
            self.cloud.status["state"].as_str(),
            Some("paused" | "blocked")
        ) {
            if let Some(id) = self.cloud.status_request.take() {
                self.cancel(id);
            }
        }
        if self.cloud.status_request.is_none() {
            if let Some(target) = &self.cloud.target {
                let id = target["workspaceId"].clone();
                self.cloud.status_request =
                    Some(self.call(methods::CLOUD_ENVIRONMENT, json!({"workspaceId":id,"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization,"interactiveResume":interactive_resume})));
            }
        }
    }
    fn attach_running_cloud_history(&mut self, frame: &Value, value: &Value) {
        if frame["cachedHistory"] == true
            || value["provisional"] == true
            || value["historyPage"] == true
            || value["workspace"]["state"] != "ready"
            || self.cloud.account.is_none()
            || value["accountId"].as_str() != self.cloud.account.as_deref()
        {
            return;
        }
        let Some(workspace) = value["workspace"]["workspaceId"].as_str() else {
            return;
        };
        if self
            .cloud
            .target
            .as_ref()
            .is_some_and(|target| target["workspaceId"] == workspace)
            && self
                .cloud
                .head
                .as_ref()
                .is_some_and(|head| head.workspace_id == workspace)
            && self.cloud.status_request.is_none()
        {
            // Reattach to a running computer, even if it sleeps after this read.
            // The controller must not turn that race into a resume operation.
            self.connect_cloud_with_intent(false);
        }
    }
    fn remember_cloud_head(&mut self) {
        if let (Some(head), Some(at)) = (&self.cloud.head, self.cloud.head_verified) {
            let small = match &head.transcript.frame {
                zuse_model::TranscriptFrame::Reset { reset } => {
                    reset.len() <= 2048
                        && reset
                            .iter()
                            .flat_map(|entry| entry.parts.iter())
                            .try_fold(0usize, |n, part| {
                                n.checked_add(part.byte_len())
                                    .filter(|n| *n <= 2 * 1024 * 1024)
                            })
                            .is_some()
                }
                _ => false,
            };
            if small
                && at.elapsed() < std::time::Duration::from_secs(30)
                && self.cloud.namespace.is_some()
                && self.cloud.account.is_some()
            {
                let key = json!([
                    self.cloud.namespace,
                    self.cloud.account,
                    self.cloud.organization,
                    head.workspace_id,
                    head.session_id
                ])
                .to_string();
                self.cloud.heads.insert(key, (head.clone(), at));
                self.cloud
                    .heads
                    .retain(|_, (_, at)| at.elapsed() < std::time::Duration::from_secs(30));
                if self.cloud.heads.len() > 16 {
                    if let Some(old) = self
                        .cloud
                        .heads
                        .iter()
                        .min_by_key(|(_, (_, at))| *at)
                        .map(|(key, _)| key.clone())
                    {
                        self.cloud.heads.remove(&old);
                    }
                }
            }
        }
    }
    fn close_cloud(&mut self) {
        self.close_cloud_repair();
        self.close_cloud_recreation();
        self.remember_cloud_head();
        self.cloud.head_verified = None;
        self.cloud.queued_prompt = None;
        for (id, _) in std::mem::take(&mut self.cloud.read_forwards) {
            self.cancel(id);
            self.pending.remove(&id);
        }
        self.cloud.cached_events = None;
        self.cloud.accepted = None;
        if let Some(id) = self.cloud.pause.take() {
            self.cancel(id);
        }
        self.cloud.pause_command = None;
        self.cloud.pause_session = None;
        self.cloud.metadata_failures = 0;
        self.cloud.poll = None;
        if let Some(id) = self.cloud.metadata.take() {
            self.cancel(id);
        }
        self.close_cloud_uploads();
        if let Some(id) = self.cloud.status_request.take() {
            self.cancel(id);
        }
        if let Some(id) = self.cloud.cache_request.take() {
            self.cancel(id);
            self.pending.remove(&id);
        }
        if let Some(id) = self.cloud.history_request.take() {
            self.cancel(id);
        }
        for (id, _) in self.cloud.forwards.drain().collect::<Vec<_>>() {
            self.cancel(id);
        }
        for id in [
            self.cloud.sync.request,
            self.cloud.sync.metadata,
            self.cloud.sync.delete,
            self.cloud.sync.log_request,
        ]
        .into_iter()
        .flatten()
        {
            self.cancel(id);
            self.pending.remove(&id);
        }
        self.cloud.sync = sync::SyncUi::default();
        self.cloud.child = None;
        self.cloud.events = None;
        self.cloud.pump = None;
        self.cloud.head = None;
        self.cloud.older_before = None;
        self.cloud.older_loading = false;
        self.cloud.prepended_history = false;
        self.cloud.retained_history_refresh = false;
        self.cloud.target = None;
        self.cloud.command_api_ready = false;
        self.cloud.command_admissions.clear();
        self.cloud.response_wait = None;
        self.cloud.queue_paused = None;
        self.cloud.reviewed_commands.clear();
        self.cloud.registered_conversations.clear();
        self.cloud.connected_epoch = None;
        self.cloud.reconnect_transport = false;
        self.cloud.initial_session = None;
        self.cloud.status = Value::Null;
    }
    /// Native and web use this same account setup form before allocating compute.
    pub fn open_cloud_setup(
        &mut self,
        account: String,
        organization: Option<String>,
        origin: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.cloud.account = Some(account);
        self.cloud.organization = organization;
        self.cloud.namespace = origin;
        self.settings = true;
        self.select_settings_section(crate::settings_view::Section::Cloud, cx);
        self.refresh_cloud();
        self.watch_cloud_account_events();
        if !self
            .pending
            .values()
            .any(|method| method == methods::CLOUD_ACCOUNT_STATUS)
        {
            self.call(methods::CLOUD_ACCOUNT_STATUS, json!({}));
        }
    }
    /// Native cloud windows share the web viewport. Seed only from a fresh,
    /// account-bound API read; this never opens an execution connection.
    pub fn open_cloud_computer(
        &mut self,
        account: String,
        organization: Option<String>,
        target: Value,
        seed: Value,
    ) {
        self.close_cloud();
        self.cloud.account = Some(account);
        self.cloud.organization = organization;
        self.cloud.namespace = seed["apiOrigin"].as_str().map(str::to_owned);
        self.cloud.command_api_ready = seed["commandDeliveryMode"] == "server-admitted-v2"
            && seed["commandAdmissionVersion"] == 2;
        let mut target = target;
        for key in [
            "commandDeliveryMode",
            "commandAdmissionVersion",
            "taskCommandCanEdit",
        ] {
            target[key] = seed[key].clone();
        }
        self.cloud.target = Some(target);
        self.cloud.command_admissions = seed["commandAdmissions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        self.cloud.registered_conversations = seed["registeredConversations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        self.cloud.head = serde_json::from_value(seed["head"].clone()).ok();
        if self.cloud.head.is_some() {
            self.cloud.head_verified = Some(web_time::Instant::now());
            self.cloud.refresh_cached = true;
        }
        self.refresh_cloud();
        self.watch_cloud_account_events();
    }
    pub(super) fn open_cloud(&mut self, target: Value) {
        if self.cloud.guest {
            let _=self.requests.unbounded_send(json!({"ui":"openCloudConversation","workspaceId":target["workspaceId"],"accountId":self.cloud.account,"organizationId":self.cloud.organization}));
            return;
        }
        self.close_cloud();
        let key = json!([
            self.cloud.namespace,
            self.cloud.account,
            self.cloud.organization,
            target["workspaceId"],
            target["initialSessionId"]
        ])
        .to_string();
        if let Some((head, at)) = self
            .cloud
            .heads
            .get(&key)
            .filter(|(_, at)| at.elapsed() < std::time::Duration::from_secs(30))
        {
            self.cloud.head = Some(head.clone());
            self.cloud.head_verified = Some(*at);
            self.cloud.refresh_cached = true;
        }
        let id = target["workspaceId"].clone();
        let session = target["initialSessionId"].clone();
        self.cloud.target = Some(target);
        self.cloud.cache_request=Some(self.call(methods::CLOUD_HISTORY,json!({"workspaceId":id,"sessionId":session,"cacheOnly":true,"incremental":true,"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization})));
        self.cloud.notice = Some("Opening saved history…".into());
        self.cloud.history_request =
            Some(self.call(methods::CLOUD_HISTORY, json!({"workspaceId":id,"incremental":true,"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization})));
    }
    pub(super) fn submit_cloud(&mut self, cx: &mut Context<Self>) {
        if self.cloud.create.is_some() {
            return;
        }
        let text = self.input.read(cx).value(cx);
        if self.cloud.launch.is_none() && text.trim().is_empty() && self.attachments.is_empty() {
            return;
        }
        if self.uploading > 0 {
            self.error = Some("Wait for attachments to finish uploading.".into());
            cx.notify();
            return;
        }
        let Some(project) = self.cloud.projects.get(self.cloud.project) else {
            self.error = Some("Select a cloud repository before starting.".into());
            cx.notify();
            return;
        };
        if self.cloud.launch.is_none() && !self.cloud_model_ready() {
            self.error = Some(
                "Choose an available Cloud model before starting. Your draft is preserved.".into(),
            );
            cx.notify();
            return;
        }
        if self.cloud.launch.is_none() {
            if self.cloud.authority && self.cloud_selected_connection().is_none() {
                self.error = Some("Choose a connected account for the selected coding agent. Your draft is preserved.".into());
                cx.notify();
                return;
            }
            let (base_ref, branch) = self
                .creation_source_picker
                .read(cx)
                .cloud_ref(project["defaultBranch"].as_str().unwrap_or("main"));
            let model = self.selected_model().unwrap().id.clone();
            let configured = self.cloud_model_configuration_available();
            let mode = match self.sandbox {
                zuse_protocol::SandboxLevel::WorkspaceWrite => "approval-required",
                zuse_protocol::SandboxLevel::Auto => "auto",
                zuse_protocol::SandboxLevel::DangerFullAccess => "full-access",
                zuse_protocol::SandboxLevel::ReadOnly => {
                    self.error =
                        Some("Choose a supported permission mode before starting Cloud.".into());
                    cx.notify();
                    return;
                }
            };
            self.cloud.launch = Some(json!({
                "projectId":project["projectId"],"providerId":self.cloud.compute_provider.as_ref().map(|id|json!(id)).unwrap_or_else(||project["providerId"].clone()),"sizeId":self.cloud.compute_size,
                "baseRef":base_ref,"branch":branch,
                "agent":self.composer_provider().map(|provider| provider["id"].clone()), "model":model,
                "reasoning":if configured { self.reasoning } else { None },
                "modelOptions":if configured { self.model_options.clone() } else { Default::default() },
                "runtimeMode":mode,
                "idempotencyKey":uuid::Uuid::new_v4().to_string(),"firstMessage":text,
                "expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization,
                "attachmentTransfers":self.attachments.iter().map(|a| json!({"uploadId":a["uploadId"],"fileName":a["fileName"]})).collect::<Vec<_>>()
            }));
            let selected_provider = self.cloud.launch.as_ref().unwrap()["providerId"]
                .as_str()
                .unwrap_or("boxd");
            if self.cloud.command_api_version == Some(2)
                && self.cloud.capabilities["providers"]
                    .as_array()
                    .is_some_and(|rows| {
                        rows.iter().any(|p| {
                            p["id"] == selected_provider && p["commandAdmissionVersion"] == 2
                        })
                    })
            {
                self.cloud.launch.as_mut().unwrap()["initialMessageDelivery"] =
                    json!("server-admitted-v2");
            }
            if self.cloud.authority {
                let selected = self
                    .cloud_selected_connection()
                    .and_then(|row| row["connectionId"].as_str())
                    .map(str::to_owned);
                // Resolve the visible default once, then retain that exact
                // account across admission retries and later revocation.
                self.cloud.connection = selected.clone();
                self.cloud.launch.as_mut().unwrap()["agentConnectionId"] = json!(selected);
            }
            self.cloud.queued_prompt = self
                .cloud
                .launch
                .as_ref()
                .map(crate::cloud_queue::Prompt::from_request);
            self.input
                .update(cx, |input, cx| input.set_value(String::new(), cx));
            self.attachments.clear();
            self.sync_previews(true, cx);
        }
        let body = self.cloud.launch.clone().unwrap();
        self.cloud.create = Some(self.call(methods::CLOUD_CREATE, body));
        self.cloud.notice = Some(
            if self.attachments.is_empty() {
                "Accepting first message…"
            } else {
                "Uploading cloud attachments…"
            }
            .into(),
        );
        self.error = None;
        cx.notify();
    }
    pub(super) fn receive_cloud(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        if self.receive_cloud_account_events(frame, cx) {
            return true;
        }
        let missing = frame["err"].as_str().is_some_and(|error| {
            matches!(
                error,
                "capability error: workspace_not_found" | "workspace_not_found"
            )
        }) && frame["id"]
            .as_u64()
            .and_then(|id| self.pending.get(&id))
            .is_some_and(|method| method == methods::CLOUD_HISTORY);
        let denied = frame.get("err").is_some_and(|error| {
            let text = error.to_string().to_lowercase();
            [
                "denied",
                "unauthorized",
                "you do not have access to this workspace.",
                "cloud could not verify your session.",
                "account changed",
                "sign in",
            ]
            .iter()
            .any(|needle| text.contains(needle))
                || error.as_str().is_some_and(|message| {
                    matches!(
                        message.trim().to_ascii_lowercase().as_str(),
                        "401" | "403" | "http 401" | "http 403"
                    )
                })
        }) && frame["id"]
            .as_u64()
            .and_then(|id| self.pending.get(&id))
            .is_some_and(|method| method == methods::CLOUD_HISTORY);
        if frame["cacheRevoked"] == true || denied || missing {
            self.revoke_cloud_archive_history();
            if let Some(id) = frame["id"].as_u64() {
                self.pending.remove(&id);
            }
            self.close_cloud();
            self.cloud.heads.clear();
            self.cloud.notice =
                Some("Cloud access changed. Reopen an authorized workspace to continue.".into());
            // Cloud mode is closed above, so its notice is no longer rendered.
            // Keep the recovery action visible in the main composer instead.
            self.error = Some(if missing {
                "This conversation is no longer available. Choose another conversation or start a new chat.".into()
            } else {
                "Sign in with the account that has access to this workspace, then reopen the conversation.".into()
            });
            cx.notify();
            return true;
        }
        if self.receive_cloud_upload(frame, cx) {
            return true;
        }
        if let Some(child) = &self.cloud.child {
            if frame["browserDrafts"]["deviceId"]
                .as_str()
                .is_some_and(|id| Some(id) == child.read(cx).device_id.as_deref())
            {
                child.update(cx, |view, cx| {
                    view.receive(frame.clone(), cx);
                });
                return true;
            }
        }
        if self
            .pending
            .get(&frame["id"].as_u64().unwrap_or(0))
            .is_some_and(|m| m == methods::CLOUD_ACCOUNT_STATUS)
        {
            if let Some(value) = frame.get("ok").or_else(|| frame.get("item")) {
                let owner = value["user"]["id"].as_str().map(str::to_owned);
                let organization = value["orgId"].as_str().map(str::to_owned);
                if self.cloud.account.is_some()
                    && (owner != self.cloud.account || self.cloud.organization != organization)
                {
                    if let Some(id) = self.cloud.account_events.request.take() {
                        self.cancel(id);
                        self.pending.remove(&id);
                    }
                    if self.cloud.guest {
                        // An unexpected guest/account stream must not downgrade this
                        // document into a local workspace or lose its admitted SID.
                        self.cloud.guest_status["commandApiAuthorized"] = json!(false);
                        self.cloud.guest_status["taskCommandCanEdit"] = json!(false);
                        self.cloud.local_navigation = None;
                        self.cloud.command_admissions.clear();
                        self.cloud.response_wait = None;
                        self.cloud.heads.clear();
                        self.messages.clear();
                        self.chats.clear();
                        self.selected = None;
                        self.error = Some("Your Cloud account changed. Return to the controller and reconnect this computer.".into());
                    } else {
                        self.close_cloud();
                        self.cloud = Cloud::default();
                        self.cloud.mode = self.cloud_controller;
                        self.cloud_models = Default::default();
                    }
                }
            }
        }
        if let Some((id, child_id)) = frame["id"]
            .as_u64()
            .and_then(|id| self.cloud.read_forwards.get(&id).map(|child| (id, *child)))
        {
            if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true {
                if let Some(events) = &self.cloud.cached_events {
                    let response = if let Some(error) = frame.get("err") {
                        json!({"id":child_id,"err":error})
                    } else if frame["ok"]["toolOutput"].is_object() {
                        json!({"id":child_id,"ok":frame["ok"]["toolOutput"]})
                    } else {
                        json!({"id":child_id,"err":"Tool output was not returned by the history service."})
                    };
                    let _ = events.unbounded_send(response);
                }
                self.cloud.read_forwards.remove(&id);
                self.pending.remove(&id);
            }
            return true;
        }
        if self.receive_cloud_recreation(frame, cx) {
            return true;
        }
        if self.receive_allocation_retry(frame, cx) {
            return true;
        }
        if self.receive_cloud_repair(frame, cx) {
            return true;
        }
        if self.receive_cloud_sync(frame) {
            self.flush_cloud_preparation_logs(cx);
            let status = self.cloud_guest_status();
            if let Some(child) = &self.cloud.child {
                child.update(cx, |view, cx| {
                    view.cloud.guest_status = status;
                    cx.notify();
                });
            }
            self.flush_cloud_account_events();
            cx.notify();
            return true;
        }
        let Some(id) = frame["id"].as_u64() else {
            return false;
        };
        if let Some((child_id, method)) = self.cloud.forwards.get(&id) {
            let direct = method == methods::CLOUD_TASK_COMMAND
                || project_start::account_control_method(method);
            if let Some(events) = self
                .cloud
                .events
                .as_ref()
                .or(self.cloud.cached_events.as_ref())
            {
                if let Some(inner) = frame.get("item") {
                    let _ = events.unbounded_send(inner.clone());
                }
                if direct && let Some(value) = frame.get("ok") {
                    let _ = events.unbounded_send(json!({"id":child_id,"ok":value}));
                }
                if let Some(error) = frame.get("err") {
                    let _ = events.unbounded_send(json!({"id":child_id,"err":error}));
                }
            }
            let admitted = matches!(
                method.as_str(),
                methods::QUEUE_COMMAND | methods::CLOUD_TASK_COMMAND
            )
            .then(|| {
                frame
                    .get("item")
                    .and_then(|inner| inner.get("ok"))
                    .or_else(|| frame.get("ok"))
                    .cloned()
            })
            .flatten();
            if frame["done"] == true
                || frame.get("err").is_some()
                || (direct && frame.get("ok").is_some())
            {
                self.cloud.forwards.remove(&id);
                self.pending.remove(&id);
            }
            if let Some(reply) = admitted {
                self.attach_admitted_cloud_run(&reply, cx);
            }
            return true;
        }
        if self.cloud.cache_request == Some(id) {
            if let Some(value) = frame.get("ok") {
                if self.cloud.head_verified.is_none()
                    && value["accountId"].as_str() == self.cloud.account.as_deref()
                    && value["organizationId"].as_str() == self.cloud.organization.as_deref()
                {
                    if let Ok(head) =
                        serde_json::from_value::<zuse_model::cloud::Head>(value["head"].clone())
                    {
                        if self.cloud.target.as_ref().is_some_and(|target| {
                            target["workspaceId"] == head.workspace_id
                                && target["initialSessionId"] == head.session_id
                        }) {
                            self.cloud.head = Some(head);
                            self.cloud.refresh_cached = true;
                            cx.notify();
                        }
                    }
                }
            }
            self.cloud.cache_request = None;
            self.pending.remove(&id);
            return true;
        }
        let kind = if self.cloud.context == Some(id) {
            1
        } else if self.cloud.list == Some(id) {
            2
        } else if self.cloud.create == Some(id) {
            3
        } else if self.cloud.status_request == Some(id) {
            4
        } else if self.cloud.history_request == Some(id) {
            5
        } else if self.cloud.pause == Some(id) {
            6
        } else if self.cloud.metadata == Some(id) {
            7
        } else {
            return false;
        };
        if let Some(error) = frame.get("err") {
            if kind == 5 {
                if let Some(child) = &self.cloud.child {
                    child.update(cx, |view, cx| {
                        view.cloud.older_loading = false;
                        view.error = Some(
                            error
                                .as_str()
                                .or_else(|| error["message"].as_str())
                                .unwrap_or("Saved history could not be loaded. Try again.")
                                .into(),
                        );
                        cx.notify();
                    });
                }
            }
            if kind == 1 {
                self.cloud_models.error = Some(
                    error
                        .as_str()
                        .or_else(|| error["message"].as_str())
                        .unwrap_or("Couldn't load Cloud models. Retry when connected.")
                        .to_owned(),
                );
                self.invalidate_picker_menu();
            }
            if kind == 7 {
                self.cloud.metadata_failures += 1;
                if self.cloud.metadata_failures >= 8 {
                    self.cloud.poll = None;
                }
            }
            self.cloud.notice = Some(
                error
                    .as_str()
                    .or_else(|| error["message"].as_str())
                    .unwrap_or("Cloud request failed. Retry to check its current state.")
                    .into(),
            );
        }
        if let Some(value) = frame.get("ok").or_else(|| frame.get("item")) {
            match kind {
                1 => {
                    let account = value["accountId"].as_str().map(str::to_owned);
                    let organization = value["organizationId"].as_str().map(str::to_owned);
                    if (self.cloud.account.is_some() && self.cloud.account != account)
                        || self.cloud.organization != organization
                        || (self.cloud.namespace.is_some()
                            && self.cloud.namespace.as_deref() != value["apiOrigin"].as_str())
                    {
                        self.stash_draft(cx);
                        self.input
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                        self.attachments.clear();
                        // Retain the previous project identity as an invalid
                        // choice until explicit reselection; never retarget it.
                        self.cloud.project = usize::MAX;
                        self.cloud.projects.clear();
                        self.error = Some("Cloud scope changed. Choose a repository again; your previous draft is preserved.".into());
                    }
                    if self.cloud.organization != organization {
                        self.close_cloud();
                        self.cloud.heads.clear();
                        self.cloud.connection = None;
                    }
                    let namespace = value["apiOrigin"].as_str().map(str::to_owned);
                    if self.cloud.namespace.is_some() && self.cloud.namespace != namespace {
                        self.close_cloud();
                        self.cloud.heads.clear();
                    }
                    if self.cloud.account.is_some() && self.cloud.account != account {
                        self.close_cloud();
                        self.cloud.workspaces.clear();
                        self.cloud.connection = None;
                        self.cloud.connections.clear();
                        self.cloud.heads.clear();
                    }
                    self.cloud.account = account;
                    self.cloud.organization = organization;
                    self.cloud.namespace = namespace;
                    if self.cloud.launch.is_none() && self.cloud.target.is_none() {
                        self.cloud.notice = None;
                    }
                    let selected_project = self.cloud.project_id.clone().or_else(|| {
                        self.cloud
                            .projects
                            .get(self.cloud.project)
                            .and_then(|row| row["projectId"].as_str())
                            .map(str::to_owned)
                    });
                    self.cloud.projects = value["projects"].as_array().cloned().unwrap_or_default();
                    self.cloud.project = selected_project
                        .as_deref()
                        .and_then(|id| {
                            self.cloud
                                .projects
                                .iter()
                                .position(|row| row["projectId"] == id)
                        })
                        .unwrap_or_else(|| {
                            if selected_project.is_some() {
                                usize::MAX
                            } else {
                                0
                            }
                        });
                    self.cloud.project_id = selected_project.or_else(|| {
                        self.cloud
                            .projects
                            .first()
                            .and_then(|row| row["projectId"].as_str())
                            .map(str::to_owned)
                    });
                    self.cloud.provider_catalogs = value["providerAgentCatalogs"].clone();
                    self.cloud.capabilities = value["computerCapabilities"].clone();
                    self.cloud.command_api_version = value["commandAdmissionVersion"].as_u64();
                    self.reconcile_cloud_computer();
                    self.cloud.authority = value["agentConnections"].is_array();
                    self.cloud.connections = value["agentConnections"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    let catalog = self
                        .cloud_computer_catalog()
                        .unwrap_or_else(|| value["agentCatalog"].clone());
                    self.receive_cloud_models(catalog, cx);

                    // Keep an explicit revoked account selected. Reconnect/reselection is
                    // required; a fresh sole account must never replace that identity.
                    self.watch_cloud_account_events();
                    self.sync_cloud_navigation_catalog(cx);
                }
                2 => {
                    if self.cloud.account.is_none() {
                        self.cloud.account = value["accountId"].as_str().map(str::to_owned);
                        self.cloud.organization =
                            value["organizationId"].as_str().map(str::to_owned);
                        self.cloud.namespace = value["apiOrigin"].as_str().map(str::to_owned);
                    }
                    self.cloud.workspaces = value["workspaces"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|w| {
                            w["desiredState"] != "archived"
                                && w["desiredState"] != "deleted"
                                && w["state"] != "deleted"
                        })
                        .collect();
                    self.sync_cloud_navigation_catalog(cx);
                }
                3 => {
                    self.creation_source_picker
                        .update(cx, |picker, cx| picker.clear_selection(cx));
                    let accepted = self.cloud.launch.take();
                    let queued = self.cloud.queued_prompt.take();
                    // Text typed after Send is a different, unsent draft. Move
                    // it into the admitted conversation without submitting it.
                    let followup = (self.input.read(cx).value(cx), self.attachments.clone());
                    self.close_cloud();
                    if !followup.0.is_empty() || !followup.1.is_empty() {
                        self.cloud.account_events.draft = Some(followup);
                        self.input
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                        self.attachments.clear();
                    }
                    self.cloud.queued_prompt = queued;
                    self.cloud.accepted = accepted;
                    let launch = &value["workspace"];
                    let mut row = launch.get("workspace").unwrap_or(launch).clone();
                    self.remember_cloud_admitted_workspace(&mut row);
                    self.cloud.target = Some(row);
                    self.sync_cloud_navigation_catalog(cx);
                    self.cloud.notice = Some("Connecting to cloud workspace…".into());
                    self.connect_cloud();
                }
                4 => {
                    if self.cloud.status != *value && self.cloud.sync.metadata.is_none() {
                        if let Some(target) = &self.cloud.target {
                            let body = json!({"workspaceId":target["workspaceId"],"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization});
                            self.cloud.sync.metadata =
                                Some(self.call(methods::CLOUD_WORKSPACE, body));
                        }
                    }
                    if let Some(child) = &self.cloud.child {
                        child.update(cx, |view, cx| {
                            let mut status = preparation::merge_status(
                                self.cloud.target.as_ref().unwrap_or(&Value::Null),
                                value,
                            );
                            status["commandApiAuthorized"] = json!(self.cloud.command_api_ready);
                            view.cloud.guest_status = status;
                            cx.notify();
                        });
                    }
                    let ready = value["state"] == "ready";
                    let epoch = value["epoch"].as_u64().unwrap_or(0);
                    if !ready && self.cloud.connected_epoch.take().is_some() {
                        let interrupted: Vec<_> = self.cloud.forwards.values().map(|(id,method)| json!({"id":id,"mutation":!methods::retryable_read(method,&Value::Null)})).collect();
                        if let Some(events) = &self.cloud.events {
                            let _ = events.unbounded_send(
                                json!({"connection":"retrying","interrupted":interrupted}),
                            );
                        }
                        for (id, _) in self.cloud.forwards.drain().collect::<Vec<_>>() {
                            self.cancel(id);
                        }
                    }
                    self.cloud.status = value.clone();
                    self.cloud.notice = if ready {
                        None
                    } else {
                        value["message"].as_str().map(str::to_owned)
                    };
                    if ready
                        && self.cloud.events.is_some()
                        && self.cloud.connected_epoch != Some(epoch)
                    {
                        self.cloud.connected_epoch = Some(epoch);
                        let _ = self
                            .cloud
                            .events
                            .as_ref()
                            .unwrap()
                            .unbounded_send(json!({"connection":"connected","epoch":epoch}));
                    }
                }
                5 => {
                    if value["accountId"].as_str() != self.cloud.account.as_deref()
                        || value["organizationId"].as_str() != self.cloud.organization.as_deref()
                        || value["apiOrigin"]
                            .as_str()
                            .is_some_and(|origin| Some(origin) != self.cloud.namespace.as_deref())
                    {
                        return true;
                    }
                    if value["session"].is_object() {
                        self.cloud.accepted = None;
                        self.cloud.queued_prompt = None;
                    }
                    if frame["cachedHistory"] != true && value["provisional"] != true {
                        if value["historyPage"] != true {
                            self.cloud.queue_paused = value["queuePaused"].as_bool();
                            self.cloud.command_admissions = value["commandAdmissions"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default();
                            self.cloud.registered_conversations = value["registeredConversations"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default();
                        }
                        self.cloud.command_api_ready = value["commandDeliveryMode"]
                            == "server-admitted-v2"
                            && value["commandAdmissionVersion"] == 2;
                        if let Some(target) = &mut self.cloud.target {
                            for key in [
                                "commandDeliveryMode",
                                "commandAdmissionVersion",
                                "queueRunSelectionVersion",
                                "taskCommandCanEdit",
                            ] {
                                target[key] = value[key].clone();
                            }
                        }
                    }
                    let mut incoming: Option<zuse_model::cloud::Head> =
                        serde_json::from_value(value["head"].clone()).ok();
                    if value["historyPage"] == true {
                        if let (Some(prior), Some(next)) = (&self.cloud.head, &mut incoming) {
                            if prior.workspace_id != next.workspace_id
                                || prior.session_id != next.session_id
                                || prior.runtime_generation != next.runtime_generation
                                || prior.cursor != next.cursor
                                || prior.older_message_sequence != value["requestedBefore"].as_i64()
                                || next.older_message_sequence.is_some_and(|n| {
                                    n <= 0 || Some(n) >= prior.older_message_sequence
                                })
                            {
                                incoming = None;
                            } else if let (
                                zuse_model::TranscriptFrame::Reset { reset: old },
                                zuse_model::TranscriptFrame::Reset { reset: new },
                            ) = (&prior.transcript.frame, &next.transcript.frame)
                            {
                                let mut entries = new.as_ref().clone();
                                entries.extend(old.iter().cloned());
                                next.transcript.replay_baseline =
                                    Some(zuse_model::TranscriptBaseline::capture(&entries));
                                next.transcript.frame = zuse_model::TranscriptFrame::Reset {
                                    reset: entries.into(),
                                };
                                self.cloud.prepended_history = true;
                            }
                        }
                        if incoming.is_none() {
                            self.cloud.notice = Some(
                                "Saved history changed. Reopen the conversation to refresh it."
                                    .into(),
                            );
                        }
                    }
                    self.cloud.retained_history_refresh = false;
                    if value["historyPage"] != true {
                        if let (Some(prior), Some(next)) = (&self.cloud.head, &mut incoming) {
                            self.cloud.retained_history_refresh =
                                retained_history::merge_loaded_prefix(prior, next);
                        }
                    }
                    let invalid_page = value["historyPage"] == true && incoming.is_none();
                    if incoming.is_some() || value["historyPage"] != true {
                        self.cloud.head = incoming;
                    }
                    if self.cloud.head.is_some() && frame["cachedHistory"] != true {
                        self.cloud.head_verified = Some(web_time::Instant::now());
                    }
                    self.remember_cloud_head();
                    if value["source"] == "admission" && value["admission"].is_object() {
                        self.cloud.accepted = Some(value["admission"].clone());
                        if self.cloud.queued_prompt.is_none() {
                            self.cloud.queued_prompt = Some(
                                crate::cloud_queue::Prompt::from_request(&value["admission"]),
                            );
                        }
                        if let Some(target) = &mut self.cloud.target {
                            if target["createdAt"].is_null() {
                                target["createdAt"] = value["acceptedAt"].clone();
                            }
                        }
                    }
                    if value["historyPage"] != true {
                        self.cloud.notice = None;
                    }
                    self.cloud.refresh_cached = !invalid_page;
                    self.attach_running_cloud_history(frame, value);
                    if invalid_page {
                        if let Some(child) = &self.cloud.child {
                            child.update(cx,|view,cx| {
                            view.cloud.older_loading=false;
                            view.error=Some("Saved history changed. Reopen this conversation to refresh it.".into());
                            cx.notify();
                        });
                        }
                    }
                }
                6 => {
                    self.cloud.pause_command = None;
                    let session = self.cloud.pause_session.take();
                    // Keep the child's request/event bridge alive across Sleep.
                    // close_cloud would drop its receiver and leave a cached
                    // composer whose later admitted Send cannot reattach.
                    if let Some(id) = self.cloud.status_request.take() {
                        self.cancel(id);
                    }
                    for (id, _) in std::mem::take(&mut self.cloud.forwards) {
                        self.cancel(id);
                    }
                    for (id, _) in std::mem::take(&mut self.cloud.read_forwards) {
                        self.cancel(id);
                    }
                    self.cloud.connected_epoch = None;
                    if let Some(events) = self.cloud.events.take() {
                        self.cloud.cached_events = Some(events);
                    }
                    if let Some(child) = &self.cloud.child {
                        child.update(cx, |view, _| {
                            view.cloud.cached = true;
                            view.connected = false;
                        });
                    }
                    self.cloud.target = Some(value["workspace"].clone());
                    self.cloud.status = Value::Null;
                    let body = json!({"workspaceId":value["workspace"]["workspaceId"],"sessionId":session.unwrap_or_else(||value["workspace"]["initialSessionId"].as_str().unwrap_or_default().to_owned()),"incremental":true,"expectedAccountId":self.cloud.account,"expectedOrganizationId":self.cloud.organization});
                    self.cloud.history_request = Some(self.call(methods::CLOUD_HISTORY, body));
                    self.cloud.notice = Some("Pausing cloud workspace…".into());
                    self.watch_cloud_pause(cx);
                }
                7 => {
                    self.cloud.metadata_failures = 0;
                    self.cloud.target = Some(value["workspace"].clone());
                    self.sync_cloud_repair_metadata();
                    self.sync_cloud_recreation_metadata();
                    self.cloud.notice = Some(
                        if value["workspace"]["state"] == "paused" {
                            "Cloud workspace paused. Connect to resume."
                        } else if value["workspace"]["statusCode"]
                            .as_str()
                            .is_some_and(|s| s.starts_with("cloud_"))
                        {
                            "Pause needs attention. Reopen this thread to check its state."
                        } else {
                            "Pausing cloud workspace…"
                        }
                        .into(),
                    );
                }
                _ => {}
            }
        }
        if kind == 4 && frame["done"] == true {
            self.cloud.status = json!({"state":"blocked"});
            self.cloud.connected_epoch = None;
            if let Some(events) = &self.cloud.events {
                let interrupted:Vec<_>=self.cloud.forwards.values().map(|(id,method)|json!({"id":id,"mutation":!methods::retryable_read(method,&Value::Null)})).collect();
                let _ = events
                    .unbounded_send(json!({"connection":"retrying","interrupted":interrupted}));
            }
            for (id, _) in self.cloud.forwards.drain().collect::<Vec<_>>() {
                self.cancel(id);
            }
        }
        if frame.get("ok").is_some() || frame.get("err").is_some() || frame["done"] == true {
            self.pending.remove(&id);
            match kind {
                1 => self.cloud.context = None,
                2 => self.cloud.list = None,
                3 => self.cloud.create = None,
                4 => self.cloud.status_request = None,
                5 => self.cloud.history_request = None,
                6 => self.cloud.pause = None,
                7 => self.cloud.metadata = None,
                _ => {}
            }
        }
        self.flush_cloud_account_events();
        cx.notify();
        true
    }
    pub(super) fn cloud_notices(&self, cx: &mut Context<Self>) -> AnyElement {
        self.cloud_creation_notice(cx)
    }
    pub(super) fn cloud_recent(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .gap_1()
            .px_2()
            .children(
                self.cloud
                    .workspaces
                    .iter()
                    .take(5)
                    .enumerate()
                    .map(|(i, w)| {
                        let w = w.clone();
                        control(
                            ("recent-cloud-thread", i),
                            SharedString::from(w["title"].as_str().unwrap_or("Cloud thread")),
                        )
                        .child(icon("cloud-dither").size(px(13.)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_cloud(w.clone());
                            cx.notify();
                        }))
                    }),
            )
            .into_any_element()
    }
    fn handle_parent_navigation(&mut self, request: &Value, cx: &mut Context<Self>) -> bool {
        if !matches!(
            request["ui"].as_str(),
            Some(
                "openCloudConversation" | "selectLocalConversation" | "newLocalProjectConversation"
            )
        ) {
            return false;
        }
        let this = self;
        match request["ui"].as_str() {
            Some("openCloudConversation") => {
                if request["accountId"].as_str() != this.cloud.account.as_deref()
                    || request["organizationId"].as_str() != this.cloud.organization.as_deref()
                {
                    return true;
                }
                if this
                    .cloud
                    .target
                    .as_ref()
                    .is_some_and(|row| row["workspaceId"] == request["workspaceId"])
                {
                    return true;
                }
                let Some(target) = this
                    .cloud
                    .workspaces
                    .iter()
                    .find(|row| {
                        row["workspaceId"] == request["workspaceId"]
                            && !matches!(
                                row["state"].as_str(),
                                Some("deleted" | "archived" | "archiving")
                            )
                    })
                    .cloned()
                else {
                    return true;
                };
                this.stash_cloud_navigation_draft(cx);
                this.open_cloud(target);
                cx.notify();
            }
            Some("selectLocalConversation" | "newLocalProjectConversation") => {
                let valid = this.connected
                    && !this.cloud_controller
                    && !this.cloud.guest
                    && !this.cached_only
                    && request["deviceId"].as_str() == this.device_id.as_deref()
                    && request["epoch"].as_u64() == Some(this.connection_epoch)
                    && request["accountId"].as_str() == this.cloud.account.as_deref()
                    && request["organizationId"].as_str() == this.cloud.organization.as_deref();
                if !valid {
                    if let Some(child) = &this.cloud.child {
                        child.update(cx,|view,cx|{view.error=Some("The local computer is unavailable. Cloud conversations remain available; no action was resent.".into());cx.notify();});
                    }
                    return true;
                }
                if request["ui"] == "selectLocalConversation" {
                    let Some(chat) = this
                        .chats
                        .iter()
                        .find(|chat| chat["id"] == request["chatId"])
                        .cloned()
                    else {
                        return true;
                    };
                    this.stash_cloud_navigation_draft(cx);
                    this.close_cloud();
                    this.cloud.mode = false;
                    this.settings = false;
                    this.select(chat, cx);
                } else {
                    let Some(path) = request["path"]
                        .as_str()
                        .filter(|path| this.spaces.iter().any(|space| space.path == *path))
                        .map(str::to_owned)
                    else {
                        return true;
                    };
                    this.stash_cloud_navigation_draft(cx);
                    this.close_cloud();
                    this.cloud.mode = false;
                    this.settings = false;
                    if this.new_chat(cx) {
                        this.project = Some(
                            json!({"path":path,"name":path.rsplit('/').next().unwrap_or(&path)}),
                        );
                    }
                }
                cx.notify();
            }
            _ => {}
        }
        true
    }
    fn stash_cloud_navigation_draft(&mut self, cx: &mut Context<Self>) {
        let Some(target) = &self.cloud.target else {
            return;
        };
        let Some(child) = self.cloud.child.clone() else {
            return;
        };
        let (session, draft) = child.update(cx, |view, cx| {
            view.stash_draft(cx);
            (
                view.selected
                    .as_ref()
                    .and_then(|row| row["id"].as_str())
                    .map(str::to_owned),
                view.drafts.get(&view.draft_key()).cloned(),
            )
        });
        let Some(session) = session else { return };
        let key = json!([
            self.cloud.namespace,
            self.cloud.account,
            self.cloud.organization,
            target["workspaceId"],
            session
        ])
        .to_string();
        if let Some(draft) = draft {
            self.cloud.cached_drafts.insert(key, draft);
        } else {
            self.cloud.cached_drafts.remove(&key);
        }
    }
    fn cached_cloud_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A live child's request channel remains authoritative while its
        // controller reconnects. A history refresh must not turn it into a
        // cached, resume-only view before or after environment readiness.
        if self.cloud.events.is_some() {
            return;
        }
        let head = self.cloud.head.clone();
        let pending = self
            .cloud
            .accepted
            .clone()
            .zip(self.cloud.queued_prompt.clone());
        if head.is_none() && pending.is_none() {
            return;
        }
        let new_child = self.cloud.child.is_none();
        if new_child {
            for (id, _) in std::mem::take(&mut self.cloud.forwards) {
                self.cancel(id);
            }
        }
        let (requests, mut receiver) = futures::channel::mpsc::unbounded::<Value>();
        let (cached_events, events) = futures::channel::mpsc::unbounded();
        if new_child {
            self.cloud.cached_events = Some(cached_events);
        }
        let child = self.cloud.child.clone().unwrap_or_else(|| {
            cx.new(|cx| {
                Self::new(
                    Transport {
                        requests: requests.clone(),
                        events,
                    },
                    window,
                    cx,
                )
            })
        });
        let navigation = if !self.cloud.guest && !self.cached_only && self.connected {
            self.device_id.clone().map(|device| LocalNavigation {
                chats: self.chats.clone(),
                spaces: self.spaces.clone(),
                device,
                epoch: self.connection_epoch,
            })
        } else {
            None
        };
        let restored_draft = head
            .as_ref()
            .and_then(|head| {
                self.cloud.cached_drafts.get(
                    &json!([
                        self.cloud.namespace,
                        self.cloud.account,
                        self.cloud.organization,
                        head.workspace_id,
                        head.session_id
                    ])
                    .to_string(),
                )
            })
            .cloned();
        let restore_draft = child
            .read(cx)
            .selected
            .as_ref()
            .map(|row| row["id"].clone())
            != head.as_ref().map(|head| json!(head.session_id));
        let mut status = self.cloud_guest_status();
        let prepended = std::mem::take(&mut self.cloud.prepended_history);
        let refreshed = std::mem::take(&mut self.cloud.retained_history_refresh);
        let anchor = {
            let view = child.read(cx);
            let scroll = &view.transcript_scroll;
            let reading_older = scroll.max_offset().y + scroll.offset().y > px(24.);
            (prepended || refreshed && reading_older).then(|| {
                (
                    scroll.clone(),
                    scroll.max_offset().y,
                    scroll.offset(),
                    prepended,
                )
            })
        };
        if status["state"] == "ready" {
            status["state"] = json!("blocked");
            status["message"] = json!("Reconnect to continue in this workspace.");
        }
        child.update(cx, |view, cx| {
            self.seed_cloud_child_authority(view);
            if new_child {
                view.requests = requests;
                view.pending.clear();
                view.queue_watch = None;
            }
            view.cloud.guest = true;
            view.cloud.cached = true;
            view.cloud.local_navigation = navigation;
            view.cloud.guest_status = status.clone();
            view.cloud.guest_workspace = self
                .cloud
                .target
                .as_ref()
                .and_then(|t| t["workspaceId"].as_str())
                .map(str::to_owned);
            view.cloud.older_before = head.as_ref().and_then(|head| head.older_message_sequence);
            view.cloud.older_loading = false;
            if let Some(head) = &head {
                view.install_cached_history(head);
                // The snapshot is read-only by transport; the original thread remains active.
                view.cached_only = false;
                if let Some(chat) = &mut view.selected {
                    chat["archived"] = json!(false);
                }
                view.chats = head
                    .conversations
                    .iter()
                    .filter_map(|chat| serde_json::to_value(chat).ok())
                    .collect();
                if !view
                    .chats
                    .iter()
                    .any(|chat| chat["id"].as_str() == Some(&head.chat.id))
                {
                    if let Ok(chat) = serde_json::to_value(&head.chat) {
                        view.chats.push(chat);
                    }
                }
                for chat in self.cloud.registered_conversations.iter().take(64) {
                    if chat["threadId"].as_str() == head.chat.thread_id.as_deref()
                        && !view
                            .chats
                            .iter()
                            .any(|canonical| canonical["id"] == chat["id"])
                    {
                        view.chats.push(chat.clone());
                    }
                }
                for chat in &mut view.chats {
                    chat["archived"] = json!(false);
                }
                // Normalize presentation only; the certified Head stays intact.
                view.chats = view.receive_cloud_conversations(&json!(view.chats));
                if let Some(selected) = view.selected.take() {
                    view.selected = view
                        .receive_cloud_conversations(&json!([selected]))
                        .into_iter()
                        .next();
                }
                view.device_id = Some(head.chat.device_id.clone());
                view.model = head
                    .chat
                    .config
                    .as_ref()
                    .and_then(|config| config.model.clone());
            } else if let (Some(target), Some((request, prompt))) = (&self.cloud.target, &pending) {
                view.install_pending_cloud_history(target, request, prompt);
            }
            if restore_draft {
                if let Some(draft) = restored_draft {
                    view.restore_failed_draft(draft, cx);
                }
            }
            if let Some(paused) = self.cloud.queue_paused {
                view.queue_paused = paused;
            }
            view.merge_cloud_admissions(&self.cloud.command_admissions);
            view.providers = self.providers.clone();
            view.preferences = self.preferences.clone();
            view.connected = false;
            if view.cloud_server_commands() && (new_child || view.queue_watch.is_none()) {
                if let Some(chat) = view.selected.clone() {
                    view.refresh_queue_snapshot(&chat);
                }
            }
            if !view.draft_journal_ready && view.device_id.is_some() {
                let _ = view
                    .requests
                    .unbounded_send(json!({"ui":"loadDrafts","deviceId":view.device_id}));
            }

            view.status = if pending.is_some() {
                "Preparing computer"
            } else if status["state"] == "paused" {
                "Paused"
            } else {
                "Saved history"
            }
            .into();
            cx.notify();
        });
        self.cloud.child = Some(child);
        self.restore_cloud_event_draft(cx);
        if let Some((scroll, old_height, offset, prepended)) = anchor {
            let child = self.cloud.child.clone().unwrap();
            window.on_next_frame(move |_, cx| {
                scroll.set_offset(point(
                    offset.x,
                    if prepended {
                        offset.y - (scroll.max_offset().y - old_height)
                    } else {
                        offset.y
                    },
                ));
                child.update(cx, |_, cx| cx.notify());
            });
        }
        // History revalidation updates the existing child. Its request channel
        // and in-flight account operations must survive the refreshed snapshot.
        if !new_child {
            return;
        }
        let navigation_window = window.window_handle();
        self.cloud.pump = Some(cx.spawn(async move |this, cx| {
            while let Some(request) = receiver.next().await {
                if request["ui"]=="newCloudProjectConversation" {
                    let _=navigation_window.update(cx,|_,window,cx|{let _=this.update(cx,|this,cx|{
                        if request["accountId"].as_str()!=this.cloud.account.as_deref()||request["organizationId"].as_str()!=this.cloud.organization.as_deref(){return}
                        let Some(project)=this.cloud.projects.iter().find(|row|row["projectId"]==request["projectId"]).cloned()else{return};
                        this.stash_cloud_navigation_draft(cx);this.start_cloud_project_session(project,window,cx);
                    });});continue;
                }
                if this.update(cx, |this, cx| {
                    if this.handle_parent_navigation(&request, cx) { return; }
                    match request["ui"].as_str() {
                        Some("resumeCloud") => {
                            this.cloud.resume_send = request["send"] == true;
                            this.connect_cloud(); cx.notify();
                        }
                        Some("leaveCloud") => { this.close_cloud(); this.refresh_cloud(); cx.notify(); }
                        Some("cloudAgentSetup") => this.open_selected_cloud_agent(cx),
                        Some("cloudSetup") => {
                            this.settings = true;
                            this.select_settings_section(crate::settings_view::Section::Cloud, cx);
                            cx.notify();
                        }
                        Some("cloudHistoryOlder") => {
                            if this.cloud.history_request.is_none() {
                                if let Some(head)=&this.cloud.head {
                                    if let Some(before)=head.older_message_sequence {
                                        let body=json!({"workspaceId":head.workspace_id,"sessionId":head.session_id,"beforeSequence":before,"cursor":head.cursor,"incremental":true,"expectedAccountId":this.cloud.account,"expectedOrganizationId":this.cloud.organization});
                                        this.cloud.history_request=Some(this.call(methods::CLOUD_HISTORY,body));
                                    }
                                }
                            }
                        }
                        Some("cloudPreparationLogs") => this.request_cloud_preparation_logs(cx),
                        Some("selectCachedCloudSession") => {
                            if let Some(target) = &this.cloud.target {
                                let body = json!({"workspaceId":target["workspaceId"],"sessionId":request["sessionId"],"incremental":true,"expectedAccountId":this.cloud.account,"expectedOrganizationId":this.cloud.organization});
                                this.cloud.history_request = Some(this.call(methods::CLOUD_HISTORY, body));
                            }
                        }
                        Some("pickAttachments"|"pasteAttachments")=>this.cloud_file_picker(request),
                        Some("saveDrafts" | "loadDrafts" | "savePreferences") => { let _ = this.requests.unbounded_send(request); }
                        _ => {
                            if request["cancel"]==true{if let Some(child)=request["id"].as_u64(){let forward=this.cloud.forwards.iter().find(|(_, (id,_))|*id==child).map(|(id,_)|*id);if let Some(id)=forward{this.cancel(id);this.cloud.forwards.remove(&id);}}return;}
                            if let (Some(target),Some(child_id),Some(method))=(&this.cloud.target,request["id"].as_u64(),request["method"].as_str()){
                                if target["commandDeliveryMode"]=="server-admitted-v2"&&method==methods::CLOUD_TASK_COMMAND&&matches!(request["params"]["request"]["action"]["type"].as_str(),Some("pause-queue"|"review-unknown")){
                                    let id=this.call(methods::CLOUD_TASK_COMMAND,json!({"workspaceId":target["workspaceId"],"expectedAccountId":this.cloud.account,"expectedOrganizationId":this.cloud.organization,"request":request["params"]["request"]}));
                                    this.cloud.forwards.insert(id,(child_id,method.to_owned()));return;
                                }
                                if target["commandDeliveryMode"]=="server-admitted-v2"&&matches!(method,methods::CREATE_THREAD_SESSION|methods::QUEUE_COMMAND|methods::QUEUE_MESSAGE|methods::UPDATE_QUEUED_MESSAGE|methods::BEGIN_QUEUED_MESSAGE_EDIT|methods::RENEW_QUEUED_MESSAGE_EDIT|methods::FINISH_QUEUED_MESSAGE_EDIT|methods::MOVE_QUEUED_MESSAGE|methods::REMOVE_QUEUED_MESSAGE|methods::SEND_QUEUED_MESSAGE_NOW|methods::STEER_QUEUED_MESSAGE_NOW|methods::WATCH_QUEUE|methods::READ_ATTACHMENT_CHUNK){
                                    let id=this.call(methods::CLOUD_RPC,json!({"workspaceId":target["workspaceId"],"expectedAccountId":this.cloud.account,"expectedOrganizationId":this.cloud.organization,"logicalCommandId":request["cloudCommandId"],"requestId":child_id,"method":method,"params":request["params"]}));
                                    this.cloud.forwards.insert(id,(child_id,method.to_owned()));return;
                                }
                            }
                            if this.forward_cloud_account_control(&request) { return; }
                            if request["method"]==methods::LIST_AGENT_ACCOUNTS {
                                if let (Some(events),Some(id))=(&this.cloud.cached_events,request["id"].as_u64()) {
                                    let _=events.unbounded_send(json!({"id":id,"err":"Open Cloud setup to inspect account usage."}));
                                }
                                return;
                            }
                            if request["method"]==methods::FETCH_TOOL_BLOB {
                                if let (Some(target),Some(child_id))=(&this.cloud.target,request["id"].as_u64()) {
                                    let body=json!({"workspaceId":target["workspaceId"],"toolBlobRef":request["params"]["blobRef"],"offset":request["params"]["offset"],"expectedAccountId":this.cloud.account,"expectedOrganizationId":this.cloud.organization});
                                    let id=this.call(methods::CLOUD_HISTORY,body);
                                    this.cloud.read_forwards.insert(id,child_id);
                                }
                                return;
                            }
                            // The cached child's same pump becomes the live
                            // bridge after a passive attachment reaches Ready.
                            if this.cloud.events.is_some() && this.cloud.status["state"]=="ready" {
                                if let (Some(target),Some(child_id),Some(method))=(&this.cloud.target,request["id"].as_u64(),request["method"].as_str()) {
                                    let local_chat=request["params"]["targetDeviceId"].as_str().and_then(|s|s.strip_prefix(crate::terminals::LOCAL_MIRROR_PREFIX));
                                    let route=if local_chat.is_some(){methods::CLOUD_LOCAL_TERMINAL}else{methods::CLOUD_RPC};
                                    let id=this.call(route,json!({"workspaceId":target["workspaceId"],"expectedAccountId":this.cloud.account,"expectedOrganizationId":this.cloud.organization,"chatId":local_chat,"requestId":child_id,"logicalCommandId":request["cloudCommandId"],"method":method,"params":request["params"]}));
                                    this.cloud.forwards.insert(id,(child_id,method.to_owned()));
                                }
                            }
                        }
                    }
                }).is_err() { break; }
            }
        }));
    }
    fn cloud_reconnect_tray(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.cloud.status["state"]
            .as_str()
            .filter(|state| !state.is_empty())
            .or_else(|| self.cloud.target.as_ref().and_then(|t| t["state"].as_str()))
            .unwrap_or("paused");
        let busy = self.cloud.status_request.is_some()
            && !matches!(state, "paused" | "blocked" | "failed");
        let paused = state == "paused"
            || self
                .cloud
                .target
                .as_ref()
                .is_some_and(|t| t["state"] == "paused");
        let title = if busy {
            "Resuming cloud workspace…"
        } else if paused {
            "Cloud workspace paused"
        } else {
            "Reconnect to cloud workspace"
        };
        column()
            .w_full()
            .max_w(px(920.))
            .self_center()
            .px_3()
            .pb_3()
            .pt_2()
            .child(
                row()
                    .id("cloud-connection-notice")
                    .role(Role::Status)
                    .min_h(px(36.))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .rounded(px(10.))
                    .bg(crate::palette::composer_toolbar())
                    .child(icon("cloud-dither").size(px(14.)))
                    .child(
                        column()
                            .min_w_0()
                            .flex_1()
                            .child(label(title).text_color(rgb(crate::shell::TEXT)))
                            .when_some(
                                self.cloud
                                    .notice
                                    .clone()
                                    .filter(|text| !text.contains("Connect to resume")),
                                |view, text| view.child(label(text).text_size(px(11.))),
                            ),
                    )
                    .child(
                        control(
                            "connect-cloud",
                            if busy {
                                "Resuming…"
                            } else if paused {
                                "Resume"
                            } else {
                                "Reconnect"
                            },
                        )
                        .opacity(if busy { 0.5 } else { 1. })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !busy {
                                this.connect_cloud();
                                cx.notify();
                            }
                        })),
                    ),
            )
            .into_any_element()
    }
    fn prepare_cloud_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ready = self.cloud.status["state"] == "ready";
        if ready
            && self.cloud.events.is_none()
            && self.cloud.cached_events.is_some()
            && self
                .cloud
                .child
                .as_ref()
                .is_some_and(|child| child.read(cx).cloud.cached)
        {
            // Promote the existing cached bridge. Retain transcript, scroll,
            // draft, editor and in-flight HTTP admission identities.
            self.cloud.events = self.cloud.cached_events.clone();
            if let Some(child) = &self.cloud.child {
                child.update(cx, |view, _| {
                    self.initialize_live_cloud_child(view);
                    view.cloud.cached = false;
                    view.cached_only = false;
                });
            }
            let epoch = self.cloud.status["epoch"].as_u64().unwrap_or(0);
            self.cloud.connected_epoch = Some(epoch);
            let _ = self
                .cloud
                .events
                .as_ref()
                .unwrap()
                .unbounded_send(json!({"connection":"connected","epoch":epoch}));
        }
        if ready && self.cloud.events.is_none() {
            // A view without a cached child establishes its first live bridge.
            let (requests, mut receiver) = futures::channel::mpsc::unbounded::<Value>();
            let (events, event_receiver) = futures::channel::mpsc::unbounded();
            let resumed = self
                .cloud
                .child
                .as_ref()
                .filter(|child| child.read(cx).cloud.cached)
                .map(|child| {
                    let view = child.read(cx);
                    (
                        view.selected
                            .as_ref()
                            .and_then(|chat| chat["id"].as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        view.input.read(cx).value(cx),
                        view.attachments.clone(),
                        self.cloud.resume_send,
                    )
                });
            let panel = self.cloud.child.as_ref().map(|child| {
                let view = child.read(cx);
                (view.right_open, view.right_tab.clone())
            });
            let child = cx.new(|cx| {
                Self::new(
                    Transport {
                        requests,
                        events: event_receiver,
                    },
                    window,
                    cx,
                )
            });
            child.update(cx, |view, _| {
                self.initialize_live_cloud_child(view);
                view.cloud.local_navigation =
                    if !self.cloud.guest && !self.cached_only && self.connected {
                        self.device_id.clone().map(|device| LocalNavigation {
                            chats: self.chats.clone(),
                            spaces: self.spaces.clone(),
                            device,
                            epoch: self.connection_epoch,
                        })
                    } else {
                        None
                    };
                view.cloud.resume_draft = resumed;
                if let Some((open, tab)) = panel {
                    view.right_open = open;
                    view.right_tab = tab;
                }
            });
            self.cloud.child = Some(child);
            self.restore_cloud_event_draft(cx);
            self.cloud.events = Some(events);
            self.cloud.initial_session = None;
            let navigation_window = window.window_handle();
            self.cloud.pump = Some(cx.spawn(async move |this,cx|{
                while let Some(request) = receiver.next().await {
                    if request["ui"]=="newCloudProjectConversation" {
                        let _=navigation_window.update(cx,|_,window,cx|{let _=this.update(cx,|this,cx|{
                            if request["accountId"].as_str()!=this.cloud.account.as_deref()||request["organizationId"].as_str()!=this.cloud.organization.as_deref(){return}
                            let Some(project)=this.cloud.projects.iter().find(|row|row["projectId"]==request["projectId"]).cloned()else{return};
                            this.stash_cloud_navigation_draft(cx);this.start_cloud_project_session(project,window,cx);
                        });});continue;
                    }
                    if this.update(cx,|this,cx|{
                        if this.handle_parent_navigation(&request, cx) { return; }
                        if request["ui"].is_string() {
                            if request["ui"]=="leaveCloud" {
                                this.close_cloud(); this.refresh_cloud();
                            } else if request["ui"]=="resumeCloud" {
                                this.connect_cloud();
                            } else if request["ui"]=="pauseCloud" {
                                this.pause_cloud(request["sessionId"].as_str().map(str::to_owned));
                            } else if request["ui"]=="cloudAgentSetup" {
                                this.open_selected_cloud_agent(cx);
                            } else if request["ui"]=="cloudSetup" {
                                this.settings = true;
                                this.select_settings_section(crate::settings_view::Section::Cloud, cx);
                                cx.notify();
                            } else if request["ui"]=="cloudPreparationLogs" {
                                this.request_cloud_preparation_logs(cx);
                            } else if matches!(request["ui"].as_str(),Some("loadDrafts"|"saveDrafts"|"savePreferences")) {
                                let _ = this.requests.unbounded_send(request);
                            } else if matches!(request["ui"].as_str(),Some("pickAttachments"|"pasteAttachments")) {
                                this.cloud_file_picker(request);
                            }
                            return;
                        }
                        let Some(child_id)=request["id"].as_u64() else{return;};
                        if request["cancel"] == true {
                            let forward=this.cloud.forwards.iter().find(|(_, (id,_))|*id==child_id).map(|(id,_)|*id);
                            if let Some(id)=forward {this.cancel(id);this.cloud.forwards.remove(&id);} return;
                        }
                        if this.forward_cloud_account_control(&request) {return;}
                        let Some(target)=&this.cloud.target else{return;};
                        let method=request["method"].as_str().unwrap_or_default().to_owned();
                        let local_chat=request["params"]["targetDeviceId"].as_str().and_then(|s|s.strip_prefix(crate::terminals::LOCAL_MIRROR_PREFIX));
                        let route=if local_chat.is_some(){methods::CLOUD_LOCAL_TERMINAL}else{methods::CLOUD_RPC};
                        let id=this.call(route,json!({"workspaceId":target["workspaceId"],"expectedAccountId":this.cloud.account,"expectedOrganizationId":this.cloud.organization,"chatId":local_chat,"requestId":child_id,"logicalCommandId":request["cloudCommandId"],"method":method,"params":request["params"]}));
                        this.cloud.forwards.insert(id,(child_id,method));
                    }).is_err(){break;}
                }
            }));
            let epoch = self.cloud.status["epoch"].as_u64().unwrap_or(0);
            self.cloud.connected_epoch = Some(epoch);
            let _ = self
                .cloud
                .events
                .as_ref()
                .unwrap()
                .unbounded_send(json!({"connection":"connected","epoch":epoch}));
        } else if !ready
            && self.cloud.events.is_none()
            && (self.cloud.child.is_none() || self.cloud.refresh_cached)
        {
            self.cloud.refresh_cached = false;
            self.cached_cloud_view(window, cx);
        } else if !ready
            && self
                .cloud
                .child
                .as_ref()
                .is_some_and(|child| child.read(cx).cloud.cached)
        {
            let mut state = self.cloud_guest_status();
            if state["state"] == "ready" || state["state"].is_null() {
                state["state"] = json!("blocked");
            }
            self.cloud
                .child
                .as_ref()
                .unwrap()
                .update(cx, |view, _| view.cloud.guest_status = state);
        }
    }
    pub(super) fn cloud_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.prepare_cloud_workspace(window, cx);
        let live = self
            .cloud
            .child
            .as_ref()
            .is_some_and(|child| !child.read(cx).cached_only);
        let branch = self
            .cloud
            .target
            .as_ref()
            .and_then(|t| t["branch"].as_str())
            .unwrap_or("Cloud workspace")
            .to_owned();
        column()
            .relative()
            .size_full()
            .min_h_0()
            .bg(rgb(BG))
            .text_color(rgb(MUTED))
            .font_family("Geist")
            .when(!live, |view| {
                view.child(
                    row()
                        .h(px(36.))
                        .px_3()
                        .gap_2()
                        .child(
                            icon_button("leave-cloud", "alt-arrow-left", "Back to threads")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_cloud();
                                    this.refresh_cloud();
                                    cx.notify();
                                })),
                        )
                        .child(icon("git-branch").size(px(14.)))
                        .child(label(SharedString::from(branch)))
                        .child(div().flex_1())
                        .child(self.cloud_details_button(window, cx)),
                )
            })
            .when_some(self.cloud.child.clone(), |view, child| {
                view.child(div().flex_1().min_h_0().child(child))
            })
            .when(live, |view| {
                view.child(
                    div()
                        .absolute()
                        .top(px(4.))
                        .right(px(44.))
                        .child(self.cloud_details_button(window, cx)),
                )
            })
            .when(!live, |view| view.child(self.cloud_reconnect_tray(cx)))
            .into_any_element()
    }
}
