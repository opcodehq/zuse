//! Cloud launch and environment selection. Only interactive connection acquires compute.
mod uploads;
mod sync;
use super::*;
use crate::assets::icon;
use crate::shell::{BG, BORDER, MUTED, column, control, label, row};

#[derive(Default)]
pub(super) struct Cloud {
    pub mode: bool,
    sync: sync::SyncUi,
    pub guest: bool,
    pub picker: Option<&'static str>,
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
    fn refresh_cloud(&mut self) {
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
        if text.trim().is_empty() && self.attachments.is_empty() {
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
        if self.cloud.launch.is_none() {
            let branch = self.cloud_branch.read(cx).value(cx);
            let model = self.cloud_model.read(cx).value(cx);
            self.cloud.launch = Some(json!({
                "projectId":project["projectId"],"providerId":project["providerId"],
                "baseRef":if branch.trim().is_empty(){project["defaultBranch"].as_str().unwrap_or("main")}else{branch.trim()},
                "agent":"claude-code", "model":if model.trim().is_empty(){"sonnet"}else{model.trim()},
                "idempotencyKey":uuid::Uuid::new_v4().to_string(),"firstMessage":text,
                "expectedAccountId":self.cloud.account,
                "attachmentTransfers":self.attachments.iter().map(|a| json!({"uploadId":a["uploadId"],"fileName":a["fileName"]})).collect::<Vec<_>>()
            }));
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
            .is_some_and(|m| m == methods::AUTH_STATUS)
        {
            if let Some(value) = frame.get("ok").or_else(|| frame.get("item")) {
                let owner = match serde_json::from_value::<zuse_protocol::AuthState>(value.clone())
                {
                    Ok(zuse_protocol::AuthState::SignedIn { user, .. }) => Some(user.id),
                    _ => None,
                };
                if self.cloud.account.is_some() && owner != self.cloud.account {
                    self.close_cloud();
                    self.cloud = Cloud::default();
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
                    self.cloud.projects = value["projects"].as_array().cloned().unwrap_or_default();
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
                    let first = self
                        .cloud
                        .launch
                        .as_ref()
                        .and_then(|v| v["firstMessage"].as_str());
                    if first == Some(self.input.read(cx).value(cx).as_str()) {
                        self.input
                            .update(cx, |input, cx| input.set_value(String::new(), cx));
                    }
                    if let Some(sent) = self
                        .cloud
                        .launch
                        .as_ref()
                        .and_then(|v| v["attachmentTransfers"].as_array())
                    {
                        self.attachments.retain(|attachment| {
                            !sent
                                .iter()
                                .any(|file| file["uploadId"] == attachment["uploadId"])
                        });
                        self.sync_previews(true, cx);
                    }
                    self.cloud.launch = None;
                    self.close_cloud();
                    let launch = &value["workspace"];
                    self.cloud.target = Some(launch.get("workspace").unwrap_or(launch).clone());
                    self.connect_cloud();
                }
                4 => {
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
                    self.cloud.notice =
                        Some("Saved history · viewing does not start compute.".into());
                }
                6 => {
                    self.cloud.pause_command = None;
                    let session = self.cloud.pause_session.take();
                    self.close_cloud();
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
    pub(super) fn cloud_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let project = self.cloud.projects.get(self.cloud.project);
        let name = if self.cloud.mode {
            project
                .and_then(|p| p["displayName"].as_str())
                .unwrap_or("Select repository")
                .to_owned()
        } else if self.project.is_some() {
            self.project_name()
        } else {
            "Select project".into()
        };
        let destination = if self.cloud.mode {
            format!(
                "Cloud · {}",
                project
                    .and_then(|p| p["displayName"].as_str())
                    .unwrap_or("Select")
            )
        } else {
            "This computer".into()
        };
        row()
            .relative()
            .w(relative(14. / 15.))
            .self_center()
            .min_h(px(38.))
            .px_2()
            .gap_1()
            .bg(rgb(0x191a17))
            .border_1()
            .border_b_0()
            .border_color(rgb(0x2b2d26))
            .when(!self.has_composer_notices(), |view| view.rounded_t(px(19.)))
            .child(
                control("composer-project", "")
                    .child(icon("folder").size(px(14.)))
                    .child(SharedString::from(name))
                    .child(icon("alt-arrow-down").size(px(11.)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.cloud.mode {
                            this.cloud.picker = Some("repository");
                            this.refresh_cloud();
                        } else {
                            this.project_picker = true;
                        }
                        cx.notify();
                    })),
            )
            .child(
                control("composer-environment", "")
                    .rounded_full()
                    .bg(rgb(0x202020))
                    .child(icon(if self.cloud.mode { "cloud" } else { "monitor" }).size(px(15.)))
                    .child(SharedString::from(destination))
                    .child(icon("alt-arrow-down").size(px(11.)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.cloud.picker = if this.cloud.picker == Some("environment") {
                            None
                        } else {
                            Some("environment")
                        };
                        this.refresh_cloud();
                        cx.notify();
                    })),
            )
            .when(self.cloud.mode, |view| {
                view.child(
                    control("composer-cloud-branch", "")
                        .child(icon("git-branch").size(px(14.)))
                        .child(SharedString::from({
                            let branch = self.cloud_branch.read(cx).value(cx);
                            if branch.is_empty() {
                                project
                                    .and_then(|p| p["defaultBranch"].as_str())
                                    .unwrap_or("main")
                                    .to_owned()
                            } else {
                                branch
                            }
                        }))
                        .child(icon("alt-arrow-down").size(px(11.)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.cloud.picker = Some("branch");
                            cx.notify();
                        })),
                )
            })
            .when_some(self.cloud.picker, |view, kind| {
                view.child(self.cloud_destination_menu(kind, cx))
            })
            .into_any_element()
    }
    fn cloud_destination_menu(&self, kind: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let menu = column()
            .id("composer-destination-menu")
            .absolute()
            .bottom_full()
            .mb_2()
            .left(px(120.))
            .w(px(320.))
            .p_2()
            .gap_1()
            .rounded(px(14.))
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(0x191a17))
            .shadow_lg()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.cloud.picker = None;
                cx.notify();
            }));
        if kind == "branch" || kind == "model" {
            return menu
                .child(
                    label(if kind == "branch" {
                        "Base branch"
                    } else {
                        "Claude model"
                    })
                    .px_2()
                    .py_1(),
                )
                .child(if kind == "branch" {
                    self.cloud_branch.clone()
                } else {
                    self.cloud_model.clone()
                })
                .child(control("cloud-config-done", "Done").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.cloud.picker = None;
                        cx.notify();
                    },
                )))
                .into_any_element();
        }
        menu.when(kind=="environment",|view|view.child(
            control("destination-this-computer", "").w_full().h(px(36.))
                .when(!self.cloud.mode,|view|view.bg(rgb(0x202020)))
                .child(icon("monitor").size(px(16.))).child("This computer").child(div().flex_1())
                .child(destination_radio(!self.cloud.mode))
                .on_click(cx.listener(|this,_,_,cx|{this.cloud.mode=false;this.cloud.picker=None;cx.notify();}))))
            .children(self.cloud.projects.iter().enumerate().map(|(index,project)|{
                let ready=project["prepared"]==true && project["state"]=="ready";
                let name=project["displayName"].as_str().unwrap_or("Repository");
                let title=if kind=="environment"{format!("Cloud · {name}")}else{name.to_owned()};
                control(("cloud-destination",index),"").w_full().h(px(36.))
                    .child(icon(if kind=="environment"{"cloud"}else{"folder"}).size(px(16.)))
                    .child(div().flex_1().min_w_0().truncate().child(SharedString::from(title)))
                    .when(kind=="environment",|view|view.child(label("Beta").text_size(px(11.))))
                    .when(!ready,|view|view.child(label("Update cloud image").text_size(px(10.))))
                    .child(destination_radio(self.cloud.mode && self.cloud.project==index))
                    .on_click(cx.listener(move|this,_,_,cx|{
                        if ready {this.cloud.mode=true;this.cloud.project=index;this.cloud.picker=None;}else{this.cloud.notice=Some("This repository needs a prepared Rust cloud image before it can run.".into());}
                        cx.notify();
                    }))
            }))
            .when(self.cloud.projects.is_empty(),|view|view.child(label(if self.cloud.context.is_some(){"Loading cloud destinations…"}else{"No cloud repositories available."}).px_2().py_2()))
            .when_some(self.cloud.notice.clone(),|view,n|view.child(label(n).px_2().py_1()))
            .when(kind=="environment",|view|view.child(div().h(px(1.)).my_1().bg(rgb(BORDER))).child(
                control("destination-add-computer","").w_full().h(px(36.)).child(icon("monitor").size(px(16.))).child("Add computer…")
                    .on_click(cx.listener(|this,_,_,cx|{this.cloud.picker=None;this.settings=true;this.select_settings_section(crate::settings_view::Section::Devices,cx);}))))
            .into_any_element()
    }
    pub(super) fn cloud_notices(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .when_some(self.cloud.notice.clone(), |view, notice| {
                view.child(label(notice).px_3().py_2())
            })
            .when(
                self.cloud.launch.is_some() && self.cloud.create.is_none(),
                |view| {
                    view.child(
                        control("retry-cloud-launch", "Retry launch")
                            .on_click(cx.listener(|this, _, _, cx| this.submit_cloud(cx))),
                    )
                },
            )
            .into_any_element()
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
                        .child(icon("cloud").size(px(13.)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_cloud(w.clone());
                            cx.notify();
                        }))
                    }),
            )
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
                            if request["ui"]=="pauseCloud" {
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
        } else if !ready && self.cloud.child.is_none() {
            if let Some(head) = self.cloud.head.clone() {
                let (requests, _) = futures::channel::mpsc::unbounded();
                let (_, events) = futures::channel::mpsc::unbounded();
                let preferences = self.preferences.clone();
                self.cloud.child = Some(cx.new(|cx| {
                    let mut view = Self::new(Transport { requests, events }, window, cx);
                    view.preferences = preferences;
                    view.install_cached_history(&head);
                    view
                }));
            }
        }
        column()
            .size_full()
            .min_h_0()
            .bg(rgb(BG))
            .text_color(rgb(MUTED))
            .child(
                row()
                    .h(px(44.))
                    .px_4()
                    .gap_3()
                    .child(control("leave-cloud", "Back").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.close_cloud();
                            this.refresh_cloud();
                            cx.notify();
                        },
                    )))
                    .child(label("Cloud"))
                    .child(label(SharedString::from(
                        self.cloud
                            .target
                            .as_ref()
                            .and_then(|t| t["branch"].as_str())
                            .unwrap_or_default(),
                    )))
                    .when_some(self.cloud.notice.clone(), |v, n| v.child(label(n)))
                    .child(div().flex_1())
                    .child(self.cloud_details_button(cx))
                    .when(
                        self.cloud.status_request.is_none()
                            || matches!(
                                self.cloud.status["state"].as_str(),
                                Some("paused" | "blocked")
                            ),
                        |v| {
                            v.child(control("connect-cloud", "Connect").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.connect_cloud();
                                    cx.notify();
                                },
                            )))
                        },
                    ),
            )
            .when_some(self.cloud.child.clone(), |v, child| {
                v.child(div().flex_1().min_h_0().child(child))
            })
            .into_any_element()
    }
}

fn destination_radio(selected: bool) -> Div {
    div()
        .size(px(14.))
        .rounded_full()
        .border_1()
        .border_color(rgb(if selected { 0x89ca21 } else { BORDER }))
        .flex()
        .items_center()
        .justify_center()
        .when(selected, |view| {
            view.child(div().size(px(7.)).rounded_full().bg(rgb(0x89ca21)))
        })
}
