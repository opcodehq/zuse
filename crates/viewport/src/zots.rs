//! Browser coordinator views use the same commands as the native application.
use super::*;
use crate::shell::{BORDER, column, control, label, row};
use zuse_protocol::{control as api, zots::*};

#[derive(Default)]
pub(super) struct ZotView {
    pub items: Vec<Zot>,
    pub watches: HashMap<u64, String>,
    pub form: Option<Form>,
    spaces: Vec<Value>,
    devices: Vec<Value>,
    files: Vec<ZotFile>,
    events: Vec<ZotEvent>,
    tab: usize,
    history: bool,
    pending: Option<u64>,
    selection: Option<String>,
    retry: Option<ZotCommand>,
    file_request: Option<u64>,
    event_request: Option<u64>,
}
pub(super) struct Form {
    id: String,
    zot: Option<Zot>,
    kind: FormKind,
    inputs: Vec<(&'static str, Entity<input::TextInput>)>,
    repositories: Vec<ZotRepository>,
    home: String,
    worker_device: String,
}
enum FormKind {
    Config,
    File(ZotFile),
    Worker(Option<String>),
    Decision(ZotDecision),
    Delete,
}

impl WorkspaceView {
    pub(super) fn watch_zot_home(&mut self, home: String) {
        if self.zots.watches.values().any(|id| id == &home) {
            return;
        }
        let id = self.call(api::WATCH_ZOTS, json!({"targetDeviceId":home}));
        self.zots.watches.insert(id, home);
    }
    pub(super) fn selected_zot(&self) -> Option<Zot> {
        let id = self.selected.as_ref()?.get("id")?.as_str()?;
        self.zots
            .items
            .iter()
            .find(|z| z.coordinator_chat_id == id)
            .cloned()
    }
    pub(super) fn refresh_zot(&mut self) {
        if let Some(zot) = self.selected_zot() {
            let params = json!({"zotId":zot.id,"targetDeviceId":zot.config.home_device_id,"history":self.zots.history});
            self.zots.file_request = Some(self.call(api::ZOT_FILES, params.clone()));
            self.zots.event_request = Some(self.call(api::ZOT_EVENTS, params));
        }
    }
    pub(super) fn receive_zot(
        &mut self,
        id: u64,
        method: &str,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let value = frame.get("item").or_else(|| frame.get("ok"));
        if method == methods::WATCH_CHATS {
            if let (Some(value), Some(selected)) = (value, self.zots.selection.clone()) {
                if let Some(chat) = value
                    .as_array()
                    .and_then(|chats| chats.iter().find(|c| c["id"] == selected))
                    .cloned()
                {
                    self.zots.selection = None;
                    self.select(chat, cx);
                }
            }
            return false;
        }
        if !matches!(
            method,
            api::WATCH_ZOTS
                | api::ZOT_COMMAND
                | api::ZOT_FILES
                | api::ZOT_EVENTS
                | methods::WATCH_SPACES
                | methods::WATCH_DEVICES
        ) {
            return false;
        }
        if let Some(error) = frame.get("err").and_then(Value::as_str) {
            if method != api::WATCH_ZOTS {
                self.error = Some(error.into());
            }
            if self.zots.pending == Some(id) {
                self.zots.pending = None;
            }
            self.pending.remove(&id);
            cx.notify();
            return true;
        }
        if let Some(value) = value {
            match method {
                api::WATCH_ZOTS => {
                    if let Ok(items) = serde_json::from_value::<Vec<Zot>>(value.clone()) {
                        let home = self.zots.watches.get(&id).cloned().unwrap_or_default();
                        let old = self.selected_zot();
                        self.zots.items.retain(|z| z.config.home_device_id != home);
                        self.zots.items.extend(items);
                        if old != self.selected_zot() {
                            self.refresh_zot();
                        }
                    }
                }
                methods::WATCH_SPACES => {
                    self.zots.spaces = value.as_array().cloned().unwrap_or_default()
                }
                methods::WATCH_DEVICES => {
                    self.zots.devices = value.as_array().cloned().unwrap_or_default();
                    let devices = self
                        .zots
                        .devices
                        .iter()
                        .filter_map(|d| d["id"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>();
                    for device in devices {
                        self.watch_zot_home(device);
                    }
                }
                api::ZOT_COMMAND if self.zots.pending == Some(id) => {
                    self.zots.pending = None;
                    self.zots.retry = None;
                    self.zots.form = None;
                    if let Ok(Some(zot)) = serde_json::from_value::<Option<Zot>>(value.clone()) {
                        if let Some(chat) = self
                            .chats
                            .iter()
                            .find(|c| c["id"] == zot.coordinator_chat_id)
                            .cloned()
                        {
                            self.select(chat, cx);
                        } else {
                            self.zots.selection = Some(zot.coordinator_chat_id);
                        }
                    }
                    self.refresh_zot();
                }
                api::ZOT_FILES if self.zots.file_request == Some(id) => {
                    self.zots.files = serde_json::from_value(value.clone()).unwrap_or_default()
                }
                api::ZOT_EVENTS if self.zots.event_request == Some(id) => {
                    self.zots.events = serde_json::from_value(value.clone()).unwrap_or_default()
                }
                _ => {}
            }
        }
        // Shared catalogs also feed the workspace's project and device controls.
        if matches!(method, methods::WATCH_SPACES | methods::WATCH_DEVICES) {
            cx.notify();
            return false;
        }
        if frame.get("ok").is_some() || frame.get("done").is_some() {
            self.pending.remove(&id);
        }
        cx.notify();
        true
    }
    pub(super) fn zot_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        column()
            .px_2()
            .py_2()
            .gap_1()
            .child(row().justify_between().child(label("Zots")).child(
                control("new-zot", "+").on_click(cx.listener(|this, _, window, cx| {
                    this.open_zot_form(None, FormKind::Config, window, cx)
                })),
            ))
            .children(self.zots.items.iter().map(|z| {
                let id = z.coordinator_chat_id.clone();
                let count = z.decisions.iter().filter(|d| d.answer.is_none()).count();
                control(
                    SharedString::from(format!("zot-{}", z.id)),
                    format!(
                        "{}{}",
                        z.config.name,
                        if count > 0 {
                            format!(" · {count} needs you")
                        } else {
                            String::new()
                        }
                    ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(chat) = this.chats.iter().find(|c| c["id"] == id).cloned() {
                        this.zots.form = None;
                        this.select(chat, cx);
                    }
                }))
            }))
            .into_any_element()
    }
    fn open_zot_form(
        &mut self,
        zot: Option<Zot>,
        kind: FormKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let home = zot
            .as_ref()
            .map(|z| z.config.home_device_id.clone())
            .or_else(|| self.device_id.clone())
            .unwrap_or_default();
        let repositories = zot
            .as_ref()
            .map(|z| z.config.repositories.clone())
            .unwrap_or_default();
        let worker_device = zot
            .as_ref()
            .map(|z| z.config.worker_device_id.clone())
            .unwrap_or_else(|| home.clone());
        let values = match &kind {
            FormKind::Config => vec![
                (
                    "Name",
                    zot.as_ref()
                        .map(|z| z.config.name.clone())
                        .unwrap_or_default(),
                ),
                (
                    "Purpose",
                    zot.as_ref()
                        .map(|z| z.config.purpose.clone())
                        .unwrap_or_default(),
                ),
                (
                    "Agent",
                    zot.as_ref()
                        .map(|z| z.config.coordinator_model.harness.clone())
                        .unwrap_or_else(|| "claude-code".into()),
                ),
                (
                    "Model",
                    zot.as_ref()
                        .and_then(|z| z.config.coordinator_model.model.clone())
                        .unwrap_or_default(),
                ),
                (
                    "Concurrent workers",
                    zot.as_ref()
                        .map(|z| z.config.max_concurrent_workers.to_string())
                        .unwrap_or_else(|| "4".into()),
                ),
                (
                    "Automatic follow-ups",
                    zot.as_ref()
                        .map(|z| z.config.max_followups.to_string())
                        .unwrap_or_else(|| "3".into()),
                ),
            ],
            FormKind::File(file) => vec![("Content", file.content.clone())],
            FormKind::Decision(_) => vec![("Your answer", String::new())],
            FormKind::Worker(Some(_)) => vec![("Message", String::new())],
            FormKind::Worker(None) => vec![
                ("Objective", String::new()),
                (
                    "Agent",
                    zot.as_ref()
                        .unwrap()
                        .config
                        .coordinator_model
                        .harness
                        .clone(),
                ),
                (
                    "Model",
                    zot.as_ref()
                        .unwrap()
                        .config
                        .coordinator_model
                        .model
                        .clone()
                        .unwrap_or_default(),
                ),
                ("Base branch", "HEAD".into()),
            ],
            FormKind::Delete => vec![],
        };
        let inputs = values
            .into_iter()
            .map(|(name, value)| {
                let input = cx.new(|cx| input::TextInput::multiline(name, window, cx));
                input.update(cx, |i, cx| i.set_value(value, cx));
                cx.observe(&input, |_, _, cx| cx.notify()).detach();
                (name, input)
            })
            .collect();
        self.zots.form = Some(Form {
            id: zot
                .as_ref()
                .map(|z| z.id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            zot,
            kind,
            inputs,
            repositories,
            home,
            worker_device,
        });
        self.settings = false;
        self.project_picker = false;
        cx.notify();
    }
    fn zot_action(&mut self, zot: &Zot, action: ZotAction, cx: &mut Context<Self>) {
        self.send_zot(
            zot.id.clone(),
            Some(zot.revision),
            zot.config.home_device_id.clone(),
            action,
            cx,
        );
    }
    fn send_zot(
        &mut self,
        id: String,
        revision: Option<u32>,
        home: String,
        action: ZotAction,
        cx: &mut Context<Self>,
    ) {
        if self.zots.pending.is_some() || !self.require_connection(cx) {
            return;
        }
        let mut command = ZotCommand {
            command_id: uuid::Uuid::new_v4().to_string(),
            zot_id: id,
            expected_revision: revision,
            action,
        };
        if let ZotAction::Delegate { worker, .. } = &mut command.action {
            worker.chat_id = format!("zot-worker-{}", command.command_id);
        }
        if let Some(previous) = &self.zots.retry {
            let mut comparison = command.clone();
            comparison.command_id = previous.command_id.clone();
            if let ZotAction::Delegate { worker, .. } = &mut comparison.action {
                worker.chat_id = format!("zot-worker-{}", previous.command_id);
            }
            if serde_json::to_value(comparison).ok() == serde_json::to_value(previous).ok() {
                command = previous.clone();
            }
        }
        self.zots.retry = Some(command.clone());
        let mut params = serde_json::to_value(command).unwrap();
        params["targetDeviceId"] = json!(home);
        self.zots.pending = Some(self.call(api::ZOT_COMMAND, params));
        cx.notify();
    }
    fn save_zot(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.zots.form else {
            return;
        };
        let values = form
            .inputs
            .iter()
            .map(|(_, i)| i.read(cx).value(cx))
            .collect::<Vec<_>>();
        let action = match &form.kind {
            FormKind::Config => {
                let (Ok(max_concurrent_workers), Ok(max_followups)) =
                    (values[4].parse(), values[5].parse())
                else {
                    self.error = Some("Enter valid worker limits".into());
                    cx.notify();
                    return;
                };
                let config = ZotConfig {
                    name: values[0].clone(),
                    purpose: values[1].clone(),
                    home_device_id: form.home.clone(),
                    repositories: form.repositories.clone(),
                    coordinator_model: ZotModel {
                        harness: values[2].clone(),
                        model: (!values[3].is_empty()).then(|| values[3].clone()),
                    },
                    worker_device_id: form.worker_device.clone(),
                    max_concurrent_workers,
                    max_followups,
                };
                if form.zot.is_some() {
                    ZotAction::Configure { config }
                } else {
                    ZotAction::Create { config }
                }
            }
            FormKind::File(file) => ZotAction::WriteFile {
                path: file.path.clone(),
                content: values[0].clone(),
                expected_version: file.version,
            },
            FormKind::Decision(decision) => {
                let mut decision = decision.clone();
                decision.answer = Some(values[0].clone());
                ZotAction::Decide { decision }
            }
            FormKind::Worker(Some(id)) => ZotAction::MessageWorker {
                chat_id: id.clone(),
                text: values[0].clone(),
                automatic: false,
            },
            FormKind::Worker(None) => {
                let Some(repository) = form
                    .repositories
                    .iter()
                    .find(|r| r.device_id == form.worker_device)
                else {
                    self.error = Some("Select a repository on the worker environment".into());
                    cx.notify();
                    return;
                };
                ZotAction::Delegate {
                    worker: ZotWorker {
                        chat_id: String::new(),
                        device_id: repository.device_id.clone(),
                        space_id: repository.space_id.clone(),
                        objective: values[0].clone(),
                        followups: 0,
                    },
                    model: ZotModel {
                        harness: values[1].clone(),
                        model: (!values[2].is_empty()).then(|| values[2].clone()),
                    },
                    base: values[3].clone(),
                }
            }
            FormKind::Delete => ZotAction::Delete,
        };
        self.send_zot(
            form.id.clone(),
            form.zot.as_ref().map(|z| z.revision),
            form.home.clone(),
            action,
            cx,
        );
    }
    pub(super) fn zot_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let form = self.zots.form.as_ref().unwrap();
        let mut view = column()
            .id("zot-form")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .child(label("Zot workspace"));
        for (name, input) in &form.inputs {
            view = view.child(label(*name)).child(input.clone());
        }
        if matches!(form.kind, FormKind::Config | FormKind::Worker(None)) {
            view = view.child(label("Home environment"));
            for device in &self.zots.devices {
                let id = device["id"].as_str().unwrap_or_default().to_owned();
                let name = device["name"].as_str().unwrap_or(&id);
                view = view.child(
                    control(
                        SharedString::from(format!("zot-home-{id}")),
                        format!("{}{}", if form.home == id { "✓ " } else { "" }, name),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = this.zots.form.as_mut() {
                            if form.zot.is_none() {
                                form.home = id.clone();
                            }
                        }
                        cx.notify();
                    })),
                );
            }
            view = view.child(label("Worker environment"));
            for device in &self.zots.devices {
                let id = device["id"].as_str().unwrap_or_default().to_owned();
                let name = device["name"].as_str().unwrap_or(&id);
                view = view.child(
                    control(
                        SharedString::from(format!("zot-device-{id}")),
                        format!(
                            "{}{}",
                            if form.worker_device == id { "✓ " } else { "" },
                            name
                        ),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = this.zots.form.as_mut() {
                            form.worker_device = id.clone();
                        }
                        cx.notify();
                    })),
                );
            }
            view = view.child(label("Repositories"));
            for space in &self.zots.spaces {
                let id = space["id"].as_str().unwrap_or_default().to_owned();
                let device = space["deviceId"].as_str().unwrap_or_default().to_owned();
                let name = space["name"]
                    .as_str()
                    .or_else(|| space["path"].as_str())
                    .unwrap_or(&id);
                let checked = form.repositories.iter().any(|r| r.space_id == id);
                view = view.child(
                    control(
                        SharedString::from(format!("zot-repo-{id}")),
                        format!("{}{}", if checked { "✓ " } else { "" }, name),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = this.zots.form.as_mut() {
                            if form.repositories.iter().any(|r| r.space_id == id) {
                                form.repositories.retain(|r| r.space_id != id);
                            } else {
                                form.repositories.push(ZotRepository {
                                    space_id: id.clone(),
                                    device_id: device.clone(),
                                });
                            }
                        }
                        cx.notify();
                    })),
                );
            }
        }
        if matches!(form.kind, FormKind::Delete) {
            view = view.child(label(
                "Delete this Zot? Worker conversations and files will be kept.",
            ));
        }
        view.child(
            row()
                .gap_2()
                .child(
                    control(
                        "save-zot",
                        if self.zots.pending.is_some() {
                            "Saving…"
                        } else {
                            "Save"
                        },
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.save_zot(cx))),
                )
                .child(
                    control("cancel-zot", "Cancel").on_click(cx.listener(|this, _, _, cx| {
                        this.zots.form = None;
                        cx.notify();
                    })),
                ),
        )
        .into_any_element()
    }
    pub(super) fn zot_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(zot) = self.selected_zot() else {
            return Empty.into_any_element();
        };
        let mut view = column()
            .id("zot-panel")
            .w(px(320.))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .p_3()
            .gap_2()
            .border_l_1()
            .border_color(rgb(BORDER));
        view = view.child(
            row().children(["Overview", "Digest", "Files"].into_iter().enumerate().map(
                |(tab, name)| {
                    control(SharedString::from(format!("zot-tab-{tab}")), name).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.zots.tab = tab;
                            cx.notify();
                        }),
                    )
                },
            )),
        );
        if self.zots.tab == 0 {
            view = view
                .child(label(format!("{:?}", zot.status)))
                .child(label("Needs you"));
            for decision in zot.decisions.iter().filter(|d| d.answer.is_none()) {
                let z = zot.clone();
                let decision = decision.clone();
                view = view.child(
                    control(
                        SharedString::from(format!("decision-{}", decision.id)),
                        decision.question.clone(),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_zot_form(
                            Some(z.clone()),
                            FormKind::Decision(decision.clone()),
                            window,
                            cx,
                        )
                    })),
                );
            }
            for event in self
                .zots
                .events
                .iter()
                .filter(|e| e.zot_id == zot.id && !e.acknowledged && e.kind != "settled")
            {
                view = view.child(label(event.summary.clone()));
            }
            let z = zot.clone();
            view = view.child(
                control("new-zot-worker", "New worker").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.open_zot_form(Some(z.clone()), FormKind::Worker(None), window, cx)
                    },
                )),
            );
            for worker in &zot.workers {
                let id = worker.chat_id.clone();
                let status = self
                    .sessions
                    .iter()
                    .find(|s| s["chatId"] == id)
                    .filter(|s| {
                        s["updatedAt"]
                            .as_str()
                            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                            .is_some_and(|t| {
                                (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds()
                                    < 45
                            })
                    })
                    .and_then(|s| s["status"].as_str())
                    .unwrap_or("unknown");
                let z = zot.clone();
                view = view.child(
                    control(
                        SharedString::from(format!("worker-{id}")),
                        format!("{} · {status}", worker.objective),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(chat) = this.chats.iter().find(|c| c["id"] == id).cloned() {
                            this.select(chat, cx);
                        }
                    })),
                );
                let id = worker.chat_id.clone();
                view = view.child(
                    control(SharedString::from(format!("message-{id}")), "Message").on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.open_zot_form(
                                Some(z.clone()),
                                FormKind::Worker(Some(id.clone())),
                                window,
                                cx,
                            )
                        }),
                    ),
                );
                for (name, kind) in [("Stop", 0), ("Archive", 1), ("Release", 2)] {
                    let id = worker.chat_id.clone();
                    let z = zot.clone();
                    view = view.child(
                        control(SharedString::from(format!("worker-{kind}-{id}")), name).on_click(
                            cx.listener(move |this, _, _, cx| {
                                let action = match kind {
                                    0 => ZotAction::StopWorker {
                                        chat_id: id.clone(),
                                    },
                                    1 => ZotAction::ArchiveWorker {
                                        chat_id: id.clone(),
                                    },
                                    _ => ZotAction::ReleaseWorker {
                                        chat_id: id.clone(),
                                    },
                                };
                                this.zot_action(&z, action, cx);
                            }),
                        ),
                    );
                }
            }
            for chat in self
                .chats
                .iter()
                .filter(|c| {
                    c["parentChatId"].is_null()
                        && zot
                            .config
                            .repositories
                            .iter()
                            .any(|r| Some(r.space_id.as_str()) == c["spaceId"].as_str())
                        && !self.zots.items.iter().any(|z| {
                            z.workers
                                .iter()
                                .any(|w| Some(w.chat_id.as_str()) == c["id"].as_str())
                        })
                })
                .take(16)
            {
                let chat = chat.clone();
                let z = zot.clone();
                let title = chat["title"].as_str().unwrap_or("Conversation").to_owned();
                view = view.child(
                    control(
                        SharedString::from(format!("adopt-{}", chat["id"])),
                        format!("Adopt {title}"),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.zot_action(
                            &z,
                            ZotAction::AdoptWorker {
                                worker: ZotWorker {
                                    chat_id: chat["id"].as_str().unwrap_or_default().into(),
                                    device_id: chat["deviceId"].as_str().unwrap_or_default().into(),
                                    space_id: chat["spaceId"].as_str().unwrap_or_default().into(),
                                    objective: title.clone(),
                                    followups: 0,
                                },
                            },
                            cx,
                        )
                    })),
                );
            }
            for (name, status) in [
                ("Pause", ZotStatus::Paused),
                ("Resume", ZotStatus::Active),
                ("Stop Zot", ZotStatus::Stopped),
                ("Archive Zot", ZotStatus::Archived),
            ] {
                let z = zot.clone();
                view = view.child(
                    control(SharedString::from(format!("zot-{name}")), name).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.zot_action(&z, ZotAction::SetStatus { status }, cx)
                        },
                    )),
                );
            }
            let z = zot.clone();
            view = view.child(
                control("fresh-zot", "Fresh conversation").on_click(cx.listener(
                    move |this, _, _, cx| this.zot_action(&z, ZotAction::FreshConversation, cx),
                )),
            );
            for (index, id) in zot.previous_coordinator_chats.iter().enumerate() {
                let id = id.clone();
                view = view.child(
                    control(
                        SharedString::from(format!("zot-old-{id}")),
                        format!("Previous conversation {}", index + 1),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(chat) = this.chats.iter().find(|c| c["id"] == id).cloned() {
                            this.select(chat, cx);
                        }
                    })),
                );
            }
            let z = zot.clone();
            view = view.child(control("delete-zot", "Delete…").on_click(cx.listener(
                move |this, _, window, cx| {
                    this.open_zot_form(Some(z.clone()), FormKind::Delete, window, cx)
                },
            )));
        } else {
            view = view.child(
                control(
                    "zot-file-history",
                    if self.zots.history {
                        "Current versions"
                    } else {
                        "Version history"
                    },
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.zots.history = !this.zots.history;
                    this.refresh_zot();
                    cx.notify();
                })),
            );
            for file in self
                .zots
                .files
                .iter()
                .filter(|f| self.zots.tab == 2 || f.path == "DIGEST.md")
            {
                view = view
                    .child(label(format!("{} · version {}", file.path, file.version)))
                    .child(label(file.content.clone()));
                let latest = self
                    .zots
                    .files
                    .iter()
                    .filter(|f| f.path == file.path)
                    .map(|f| f.version)
                    .max()
                    .unwrap_or(file.version);
                let z = zot.clone();
                let file = file.clone();
                if latest > file.version {
                    view = view.child(
                        control(
                            SharedString::from(format!("restore-{}-{}", file.path, file.version)),
                            "Restore as new version",
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.zot_action(
                                &z,
                                ZotAction::RestoreFile {
                                    path: file.path.clone(),
                                    version: file.version,
                                    expected_version: latest,
                                },
                                cx,
                            )
                        })),
                    );
                } else {
                    view = view.child(
                        control(SharedString::from(format!("edit-{}", file.path)), "Edit")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_zot_form(
                                    Some(z.clone()),
                                    FormKind::File(file.clone()),
                                    window,
                                    cx,
                                )
                            })),
                    );
                }
            }
        }
        view.child(control("zot-settings", "Settings").on_click(cx.listener(
            move |this, _, window, cx| {
                this.open_zot_form(Some(zot.clone()), FormKind::Config, window, cx)
            },
        )))
        .into_any_element()
    }
}
