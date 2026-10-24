//! Cloud launch and environment selection. Only interactive connection acquires compute.
mod uploads;
mod creation;
mod sync;
pub(super) mod models;
use super::*;
use crate::assets::icon;
use crate::shell::{BG, BORDER, MUTED, column, control, icon_button, label, row};

#[derive(Default)]
pub(super) struct Cloud {
    pub mode: bool,
    sync: sync::SyncUi,
    pub guest: bool,
    pub guest_status: Value,
    pub cached: bool,
    refresh_cached: bool,
    pub resume_draft: Option<(String, String, Vec<Value>, bool)>,
    resume_send: bool,
    pub picker: Option<&'static str>,
    creation: creation::State,
    pub target: Option<Value>,
    account: Option<String>,
    projects: Vec<Value>,
    pub workspaces: Vec<Value>,
    project: usize,
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
    queued_prompt: Option<crate::cloud_queue::Prompt>,
    status_request: Option<u64>,
    status: Value,
    history_request: Option<u64>,
    head: Option<zuse_model::cloud::Head>,
    child: Option<Entity<WorkspaceView>>,
    events: Option<UnboundedSender<Value>>,
    pump: Option<Task<()>>,
    forwards: HashMap<u64, (u64, String)>,
    uploads: HashMap<u64, Value>,
    viewport: String,
    connected_epoch: Option<u64>,
    pub initial_session: Option<String>,
    notice: Option<String>,
}
impl WorkspaceView {
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
        let body = json!({"workspaceId":target["workspaceId"],"expectedAccountId":self.cloud.account,"commandId":command});
        self.cloud.pause_session = session;
        self.cloud.pause = Some(self.call(methods::CLOUD_PAUSE, body));
        self.cloud.notice = Some("Pausing cloud workspace…".into());
    }
    fn watch_cloud_pause(&mut self, cx: &mut Context<Self>) {
        self.cloud.poll=Some(cx.spawn(async move|this,cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
                if !this.update(cx,|this,_| {
                    if this.cloud.target.as_ref().is_none_or(|t|t["state"]!="pausing") {return false;}
                    if this.cloud.metadata.is_none() {
                        let body=json!({"workspaceId":this.cloud.target.as_ref().unwrap()["workspaceId"],"expectedAccountId":this.cloud.account});
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
                    Some(self.call(methods::CLOUD_ENVIRONMENT, json!({"workspaceId":id})));
            }
        }
    }
    fn close_cloud(&mut self) {
        self.cloud.queued_prompt = None;
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
        if let Some(id) = self.cloud.history_request.take() {
            self.cancel(id);
        }
        for (id, _) in self.cloud.forwards.drain().collect::<Vec<_>>() {
            self.cancel(id);
        }
        for id in [self.cloud.sync.request, self.cloud.sync.metadata].into_iter().flatten() {
            self.cancel(id); self.pending.remove(&id);
        }
        self.cloud.sync = sync::SyncUi::default();
        self.cloud.child = None;
        self.cloud.events = None;
        self.cloud.pump = None;
        self.cloud.head = None;
        self.cloud.target = None;
        self.cloud.connected_epoch = None;
        self.cloud.initial_session = None;
        self.cloud.status = Value::Null;
    }
    fn open_cloud(&mut self, target: Value) {
        self.close_cloud();
        let id = target["workspaceId"].clone();
        self.cloud.target = Some(target);
        self.cloud.notice = Some("Opening saved history…".into());
        self.cloud.history_request =
            Some(self.call(methods::CLOUD_HISTORY, json!({"workspaceId":id})));
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
            self.error = Some("Select a prepared cloud repository before starting.".into());
            cx.notify();
            return;
        };
        if self.cloud.launch.is_none() && !self.cloud_model_ready() {
            self.error = Some("Choose an available Cloud model before starting. Your draft is preserved.".into());
            cx.notify();
            return;
        }
        if self.cloud.launch.is_none() {
            let (base_ref, branch) = self.creation_source_picker.read(cx).cloud_ref(project["defaultBranch"].as_str().unwrap_or("main"));
            let model = self.selected_model().unwrap().id.clone();
            let configured = self.cloud_model_configuration_available();
            let mode = match self.sandbox {
                zuse_protocol::SandboxLevel::WorkspaceWrite => "approval-required",
                zuse_protocol::SandboxLevel::Auto => "auto",
                zuse_protocol::SandboxLevel::DangerFullAccess => "full-access",
                zuse_protocol::SandboxLevel::ReadOnly => {
                    self.error = Some("Choose a supported permission mode before starting Cloud.".into());
                    cx.notify();
                    return;
                }
            };
            self.cloud.launch = Some(json!({
                "projectId":project["projectId"],"providerId":project["providerId"],
                "baseRef":base_ref,"branch":branch,
                "agent":self.composer_provider().map(|provider| provider["id"].clone()), "model":model,
                "reasoning":if configured { self.reasoning } else { None },
                "modelOptions":if configured { self.model_options.clone() } else { Default::default() },
                "runtimeMode":mode,
                "idempotencyKey":uuid::Uuid::new_v4().to_string(),"firstMessage":text,
                "expectedAccountId":self.cloud.account,
                "attachmentTransfers":self.attachments.iter().map(|a| json!({"uploadId":a["uploadId"],"fileName":a["fileName"]})).collect::<Vec<_>>()
            }));
            self.cloud.queued_prompt = self.cloud.launch.as_ref().map(crate::cloud_queue::Prompt::from_request);
            self.input.update(cx, |input, cx| input.set_value(String::new(), cx));
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
                let owner = value["user"]["id"].as_str().filter(|_| value["orgId"].is_null()).map(str::to_owned);
                if self.cloud.account.is_some() && owner != self.cloud.account {
                    self.close_cloud();
                    self.cloud = Cloud::default();
                    self.cloud_models = Default::default();
                }
            }
        }
        if self.receive_cloud_sync(frame) { return true; }
        let Some(id) = frame["id"].as_u64() else {
            return false;
        };
        if let Some((child_id, _)) = self.cloud.forwards.get(&id) {
            if let Some(events) = &self.cloud.events {
                if let Some(inner) = frame.get("item") {
                    let _ = events.unbounded_send(inner.clone());
                }
                if let Some(error) = frame.get("err") {
                    let _ = events.unbounded_send(json!({"id":child_id,"err":error}));
                }
            }
            if frame["done"] == true || frame.get("err").is_some() {
                self.cloud.forwards.remove(&id);
                self.pending.remove(&id);
            }
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
            if kind == 1 {
                self.cloud_models.error = Some(error.as_str().or_else(|| error["message"].as_str())
                    .unwrap_or("Couldn't load Cloud models. Retry when connected.").to_owned());
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
                    if self.cloud.account.is_some() && self.cloud.account != account {
                        self.close_cloud();
                        self.cloud.workspaces.clear();
                    }
                    self.cloud.account = account;
                    if self.cloud.launch.is_none() && self.cloud.target.is_none() { self.cloud.notice = None; }
                    self.cloud.projects = value["projects"].as_array().cloned().unwrap_or_default();
                    self.receive_cloud_models(value["agentCatalog"].clone(), cx);
                    self.cloud.project = self
                        .cloud
                        .project
                        .min(self.cloud.projects.len().saturating_sub(1));
                }
                2 => {
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
                        .collect()
                }
                3 => {
                    self.creation_source_picker.update(cx, |picker, cx| picker.clear_selection(cx));
                    self.cloud.launch = None;
                    let queued = self.cloud.queued_prompt.take();
                    self.close_cloud();
                    self.cloud.queued_prompt = queued;
                    let launch = &value["workspace"];
                    self.cloud.target = Some(launch.get("workspace").unwrap_or(launch).clone());
                    self.cloud.notice = Some("Connecting to cloud workspace…".into());
                    self.connect_cloud();
                }
                4 => {
                    if let Some(child) = &self.cloud.child {
                        child.update(cx, |view, cx| { view.cloud.guest_status = value.clone(); cx.notify(); });
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
                    self.cloud.head = serde_json::from_value(value["head"].clone()).ok();
                    self.cloud.notice = None;
                    self.cloud.refresh_cached = true;
                }
                6 => {
                    self.cloud.pause_command = None;
                    let session = self.cloud.pause_session.take();
                    let child = self.cloud.child.take();
                    self.close_cloud();
                    self.cloud.child = child;
                    if let Some(child) = &self.cloud.child { child.update(cx, |view, _| view.cloud.cached = true); }
                    self.cloud.target = Some(value["workspace"].clone());
                    self.cloud.status = Value::Null;
                    let body = json!({"workspaceId":value["workspace"]["workspaceId"],"sessionId":session.unwrap_or_else(||value["workspace"]["initialSessionId"].as_str().unwrap_or_default().to_owned())});
                    self.cloud.history_request = Some(self.call(methods::CLOUD_HISTORY, body));
                    self.cloud.notice = Some("Pausing cloud workspace…".into());
                    self.watch_cloud_pause(cx);
                }
                7 => {
                    self.cloud.metadata_failures = 0;
                    self.cloud.target = Some(value["workspace"].clone());
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
    fn cached_cloud_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(head) = self.cloud.head.clone() else { return; };
        let (requests, mut receiver) = futures::channel::mpsc::unbounded::<Value>();
        let (_, events) = futures::channel::mpsc::unbounded();
        let child = self.cloud.child.clone().unwrap_or_else(|| cx.new(|cx| Self::new(Transport { requests: requests.clone(), events }, window, cx)));
        let mut status = self.cloud.target.as_ref().cloned().unwrap_or_default();
        if status["state"] == "ready" {
            status["state"] = json!("blocked");
            status["message"] = json!("Reconnect to continue in this workspace.");
        }
        child.update(cx, |view, cx| {
            view.requests = requests;
            view.pending.clear();
            view.cloud.guest = true;
            view.cloud.cached = true;
            view.cloud.guest_status = status.clone();
            view.cloud.guest_workspace = Some(head.workspace_id.clone());
            view.install_cached_history(&head);
            // The snapshot is read-only by transport; the original thread remains active.
            view.cached_only = false;
            if let Some(chat) = &mut view.selected { chat["archived"] = json!(false); }
            view.chats = head.conversations.iter().filter_map(|chat| serde_json::to_value(chat).ok()).collect();
            if !view.chats.iter().any(|chat|chat["id"].as_str() == Some(&head.chat.id)) {
                if let Ok(chat) = serde_json::to_value(&head.chat) { view.chats.push(chat); }
            }
            for chat in &mut view.chats { chat["archived"] = json!(false); }
            view.device_id = Some(head.chat.device_id.clone());
            view.providers = self.providers.clone();
            view.preferences = self.preferences.clone();
            view.connected = false;
            view.status = if status["state"] == "paused" { "Paused" } else { "Saved history" }.into();
            view.model = head.chat.config.as_ref().and_then(|config| config.model.clone());
            cx.notify();
        });
        self.cloud.child = Some(child);
        self.cloud.pump = Some(cx.spawn(async move |this, cx| {
            while let Some(request) = receiver.next().await {
                if this.update(cx, |this, cx| {
                    match request["ui"].as_str() {
                        Some("resumeCloud") => {
                            this.cloud.resume_send = request["send"] == true;
                            this.connect_cloud(); cx.notify();
                        }
                        Some("leaveCloud") => { this.close_cloud(); this.refresh_cloud(); cx.notify(); }
                        Some("selectCachedCloudSession") => {
                            if let Some(target) = &this.cloud.target {
                                let body = json!({"workspaceId":target["workspaceId"],"sessionId":request["sessionId"]});
                                this.cloud.history_request = Some(this.call(methods::CLOUD_HISTORY, body));
                            }
                        }
                        Some("saveDrafts" | "loadDrafts" | "savePreferences") => { let _ = this.requests.unbounded_send(request); }
                        _ => {}
                    }
                }).is_err() { break; }
            }
        }));
    }
    fn cloud_reconnect_tray(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = self.cloud.status["state"].as_str()
            .filter(|state| !state.is_empty())
            .or_else(|| self.cloud.target.as_ref().and_then(|t| t["state"].as_str()))
            .unwrap_or("paused");
        let busy = self.cloud.status_request.is_some() && !matches!(state, "paused" | "blocked" | "failed");
        let paused = state == "paused" || self.cloud.target.as_ref().is_some_and(|t| t["state"] == "paused");
        let title = if busy { "Resuming cloud workspace…" } else if paused { "Cloud workspace paused" } else { "Reconnect to cloud workspace" };
        column().w_full().max_w(px(920.)).self_center().px_3().pb_3().pt_2()
            .child(row().id("cloud-connection-notice").role(Role::Status)
                .min_h(px(36.)).px_3().py_1().gap_2().rounded(px(10.))
                .bg(crate::palette::composer_toolbar())
                .child(icon("cloud-dither").size(px(14.)))
                .child(column().min_w_0().flex_1()
                    .child(label(title).text_color(rgb(crate::shell::TEXT)))
                    .when_some(self.cloud.notice.clone().filter(|text| !text.contains("Connect to resume")), |view, text| view.child(label(text).text_size(px(11.)))))
                .child(control("connect-cloud", if busy { "Resuming…" } else if paused { "Resume" } else { "Reconnect" })
                    .opacity(if busy {0.5} else {1.})
                    .on_click(cx.listener(move |this, _, _, cx| { if !busy { this.connect_cloud(); cx.notify(); } }))))
            .into_any_element()
    }
    pub(super) fn cloud_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ready = self.cloud.status["state"] == "ready";
        if ready && self.cloud.events.is_none() {
            // Replace the cached transcript only once the live transport is available.
            let (requests, mut receiver) = futures::channel::mpsc::unbounded::<Value>();
            let (events, event_receiver) = futures::channel::mpsc::unbounded();
            let resumed = self.cloud.child.as_ref().filter(|child| child.read(cx).cloud.cached).map(|child| {
                let view = child.read(cx);
                (view.selected.as_ref().and_then(|chat|chat["id"].as_str()).unwrap_or_default().to_owned(), view.input.read(cx).value(cx), view.attachments.clone(), self.cloud.resume_send)
            });
            let panel = self.cloud.child.as_ref().map(|child| { let view = child.read(cx); (view.right_open, view.right_tab.clone()) });
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
                view.cloud.guest = true;
                view.cloud.guest_status = self.cloud.status.clone();
                view.cloud.resume_draft = resumed;
                if let Some((open, tab)) = panel { view.right_open = open; view.right_tab = tab; }

                view.cloud.guest_workspace = self
                    .cloud
                    .target
                    .as_ref()
                    .and_then(|t| t["workspaceId"].as_str())
                    .map(str::to_owned);
                view.preferences = self.preferences.clone();
                view.cloud.initial_session = self
                    .cloud
                    .head
                    .as_ref()
                    .map(|h| h.session_id.clone())
                    .or_else(|| {
                        self.cloud
                            .target
                            .as_ref()
                            .and_then(|t| t["initialSessionId"].as_str())
                            .map(str::to_owned)
                    });
            });
            self.cloud.child = Some(child);
            self.cloud.events = Some(events);
            self.cloud.initial_session = None;
            self.cloud.pump = Some(cx.spawn(async move |this,cx|{
                while let Some(request) = receiver.next().await {
                    if this.update(cx,|this,_|{
                        if request["ui"].is_string() {
                            if request["ui"]=="leaveCloud" {
                                this.close_cloud(); this.refresh_cloud();
                            } else if request["ui"]=="resumeCloud" {
                                this.connect_cloud();
                            } else if request["ui"]=="pauseCloud" {
                                this.pause_cloud(request["sessionId"].as_str().map(str::to_owned));
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
                        let Some(target)=&this.cloud.target else{return;};
                        let method=request["method"].as_str().unwrap_or_default().to_owned();
                        let id=this.call(methods::CLOUD_RPC,json!({"workspaceId":target["workspaceId"],"requestId":child_id,"method":method,"params":request["params"]}));
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
        } else if !ready && (self.cloud.child.is_none() || self.cloud.refresh_cached) {
            self.cloud.refresh_cached = false;
            self.cached_cloud_view(window, cx);
        } else if !ready && self.cloud.child.as_ref().is_some_and(|child|child.read(cx).cloud.cached) {
            let mut state = if self.cloud.status_request.is_some() { self.cloud.status.clone() } else { self.cloud.target.as_ref().cloned().unwrap_or_default() };
            if state["state"] == "ready" || state["state"].is_null() { state["state"] = json!("blocked"); }
            self.cloud.child.as_ref().unwrap().update(cx, |view, _|view.cloud.guest_status = state);
        }
        let live = self.cloud.child.as_ref().is_some_and(|child| !child.read(cx).cached_only);
        let branch = self.cloud.target.as_ref()
            .and_then(|t| t["branch"].as_str()).unwrap_or("Cloud workspace").to_owned();
        column()
            .relative()
            .size_full()
            .min_h_0()
            .bg(rgb(BG))
            .text_color(rgb(MUTED))
            .font_family("Geist")
            .when(!live, |view| view.child(
                row().h(px(36.)).px_3().gap_2()
                    .child(icon_button("leave-cloud", "alt-arrow-left", "Back to threads")
                        .on_click(cx.listener(|this, _, _, cx| { this.close_cloud(); this.refresh_cloud(); cx.notify(); })))
                    .child(icon("git-branch").size(px(14.)))
                    .child(label(SharedString::from(branch)))
                    .child(div().flex_1())
                    .child(self.cloud_details_button(cx))
            ))
            .when_some(self.cloud.child.clone(), |view, child| {
                view.child(div().flex_1().min_h_0().child(child))
            })
            .when(live, |view| view.child(
                div().absolute().top(px(4.)).right(px(44.))
                    .child(self.cloud_details_button(cx))
            ))
            .when(!live, |view| view.child(self.cloud_reconnect_tray(cx)))
            .into_any_element()
    }
}
