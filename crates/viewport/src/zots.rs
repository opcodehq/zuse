//! Browser coordinator views use the same commands as the native application.
use super::*;
use crate::assets::icon;
use crate::shell::{
    BORDER, FAINT, MUTED, RAISED, SURFACE, TEXT, column, control, icon_button, label, row,
};
use zuse_protocol::presentation::zot_panel::{ZotTab, ZotTabs, zot_owns_chat};
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
    tabs: ZotTabs,
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
        let selected = self
            .selected
            .as_ref()
            .and_then(|c| c["id"].as_str())
            .map(str::to_owned);
        let mut list = column().px_2().pt_2().pb_2().gap(px(1.)).child(
            row()
                .h(px(26.))
                .px_2()
                .justify_between()
                .text_size(px(11.))
                .text_color(rgb(FAINT))
                .child("Zots")
                .child(
                    icon_button("new-zot", "plus", "New Zot")
                        .size(px(20.))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_zot_form(None, FormKind::Config, window, cx)
                        })),
                ),
        );
        for z in &self.zots.items {
            let id = z.coordinator_chat_id.clone();
            let active = selected.as_deref() == Some(id.as_str())
                || z.previous_coordinator_chats
                    .iter()
                    .any(|c| Some(c) == selected.as_ref());
            let count = z.decisions.iter().filter(|d| d.answer.is_none()).count();
            let initial = z
                .config
                .name
                .chars()
                .next()
                .unwrap_or('Z')
                .to_uppercase()
                .to_string();
            list = list.child(
                row()
                    .id(SharedString::from(format!("zot-{}", z.id)))
                    .role(Role::Button)
                    .aria_label(SharedString::from(z.config.name.clone()))
                    .h(px(28.))
                    .px(px(6.))
                    .gap_2()
                    .rounded(px(7.))
                    .cursor_pointer()
                    .text_size(px(13.))
                    .text_color(rgb(TEXT))
                    .when(active, |d| d.bg(rgb(RAISED)))
                    .when(!active, |d| d.hover(|d| d.bg(rgb(RAISED))))
                    .child(
                        row()
                            .size(px(18.))
                            .flex_none()
                            .justify_center()
                            .rounded(px(5.))
                            .bg(rgb(AMBER).opacity(0.16))
                            .text_size(px(10.))
                            .text_color(rgb(AMBER))
                            .child(initial),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(z.config.name.clone()),
                    )
                    .when(z.status != ZotStatus::Active, |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(FAINT))
                                .child(format!("{:?}", z.status)),
                        )
                    })
                    .when(count > 0, |d| {
                        d.child(
                            div()
                                .px(px(5.))
                                .rounded_full()
                                .bg(rgb(AMBER).opacity(0.18))
                                .text_size(px(10.))
                                .text_color(rgb(AMBER))
                                .child(count.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.open_zot_chat(&id, cx))),
            );
            // A Zot's threads live under it instead of under their projects.
            let total = z.workers.len();
            for (index, worker) in z.workers.iter().enumerate() {
                let chat = worker.chat_id.clone();
                let current = selected.as_deref() == Some(chat.as_str());
                let (dot, _) = thread_state(self.zot_thread_status(&chat));
                let title = self
                    .chats
                    .iter()
                    .find(|c| c["id"] == chat)
                    .and_then(|c| c["title"].as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| worker.objective.clone());
                list = list.child(
                    row()
                        .id(SharedString::from(format!("zot-thread-{chat}")))
                        .role(Role::Button)
                        .aria_label(SharedString::from(title.clone()))
                        .relative()
                        .h(px(26.))
                        .ml(px(14.))
                        .pl(px(10.))
                        .pr(px(6.))
                        .gap_2()
                        .rounded(px(7.))
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .text_color(rgb(if current { TEXT } else { MUTED }))
                        .when(current, |d| d.bg(rgb(RAISED)))
                        .when(!current, |d| {
                            d.hover(|d| d.bg(rgb(RAISED)).text_color(rgb(TEXT)))
                        })
                        // Tree guide joining the threads to their Zot.
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .w(px(1.))
                                .h(px(if index + 1 == total { 13. } else { 26. }))
                                .bg(rgb(BORDER)),
                        )
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top(px(13.))
                                .w(px(6.))
                                .h(px(1.))
                                .bg(rgb(BORDER)),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .child(div().size(px(6.)).flex_none().rounded_full().bg(dot))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_zot_chat(&chat, cx))),
                );
            }
        }
        list.into_any_element()
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
            FormKind::File(file) if file.version == 0 => {
                vec![("Name", String::new()), ("Content", String::new())]
            }
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
            FormKind::File(file) => {
                let (path, content) = if file.version == 0 {
                    (values[0].trim().to_owned(), values[1].clone())
                } else {
                    (file.path.clone(), values[0].clone())
                };
                if path.is_empty() {
                    self.error = Some("Name the document".into());
                    cx.notify();
                    return;
                }
                ZotAction::WriteFile {
                    path,
                    content,
                    expected_version: file.version,
                }
            }
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
            .child(label(match &form.kind {
                FormKind::File(file) if file.version == 0 => "New document".to_owned(),
                FormKind::File(file) => file.path.clone(),
                FormKind::Config if form.zot.is_none() => "Create a Zot".to_owned(),
                FormKind::Config => "Zot settings".to_owned(),
                FormKind::Worker(Some(_)) => "Message thread".to_owned(),
                FormKind::Worker(None) => "New thread".to_owned(),
                FormKind::Decision(_) => "Answer decision".to_owned(),
                FormKind::Delete => "Delete Zot".to_owned(),
            }));
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
    /// Live run state of one thread, ignoring rows a crashed host left behind.
    fn zot_thread_status(&self, chat: &str) -> &str {
        self.sessions
            .iter()
            .find(|s| s["chatId"] == chat)
            .filter(|s| {
                s["updatedAt"]
                    .as_str()
                    .and_then(|v| chrono::DateTime::parse_from_rfc3339(v).ok())
                    .is_some_and(|t| {
                        (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_seconds() < 45
                    })
            })
            .and_then(|s| s["status"].as_str())
            .unwrap_or("unknown")
    }
    fn open_zot_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.iter().find(|c| c["id"] == id).cloned() {
            self.zots.form = None;
            self.select(chat, cx);
        }
    }
    fn zot_tab_bar(&self, zot: &Zot, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.zots.tabs;
        let mut strip = row().gap(px(2.));
        for tab in ZotTab::ALL {
            let active = tab == tabs.active;
            let name = tab.label();
            let width = tab.label_width();
            let color = rgb(if active { TEXT } else { MUTED });
            let base = row()
                .id(SharedString::from(format!("zot-tab-{name}")))
                .role(Role::Tab)
                .aria_label(name)
                .h(px(28.))
                .px(px(7.))
                .rounded(px(7.))
                .cursor_pointer()
                .text_size(px(12.5))
                .text_color(color)
                .when(!active, |d| {
                    d.hover(|d| d.bg(rgb(RAISED)).text_color(rgb(TEXT)))
                })
                .child(icon(tab.icon()).size(px(15.)).text_color(color))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.zots.tabs.select(tab) {
                        let history = tab == ZotTab::History;
                        if this.zots.history != history {
                            this.zots.history = history;
                            this.refresh_zot();
                        }
                        cx.notify();
                    }
                }));
            // The active tab grows its label; the one it replaced folds back.
            let reveal = move |el: Stateful<Div>, r: f32| {
                el.bg(rgb(RAISED).opacity(r)).when(r > 0., |el| {
                    el.child(
                        div()
                            .ml(px(6. * r))
                            .max_w(px(width * r))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .opacity(r)
                            .child(name),
                    )
                })
            };
            let animating = tabs.epoch > 0 && (active || tabs.previous == Some(tab));
            strip = strip.child(if animating {
                base.with_animation(
                    SharedString::from(format!("zot-tab-{name}-{}", tabs.epoch)),
                    Animation::new(std::time::Duration::from_millis(200)).with_easing(ease_in_out),
                    move |el, t| reveal(el, tabs.reveal(tab, t)),
                )
                .into_any_element()
            } else {
                reveal(base, tabs.reveal(tab, 1.)).into_any_element()
            });
        }
        let doc = zot.clone();
        let settings = zot.clone();
        row()
            .h(px(44.))
            .flex_shrink_0()
            .px_2()
            .border_b_1()
            .border_color(rgb(BORDER))
            .child(strip)
            .child(div().flex_1())
            .when(tabs.active == ZotTab::Drive, |d| {
                d.child(
                    icon_button("zot-new-document", "document-add", "New document").on_click(
                        cx.listener(move |this, _, window, cx| {
                            let file = ZotFile {
                                path: String::new(),
                                version: 0,
                                content: String::new(),
                            };
                            this.open_zot_form(Some(doc.clone()), FormKind::File(file), window, cx)
                        }),
                    ),
                )
            })
            .child(
                icon_button("zot-settings", "settings", "Zot settings").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.open_zot_form(Some(settings.clone()), FormKind::Config, window, cx)
                    },
                )),
            )
            .into_any_element()
    }
    pub(super) fn zot_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(zot) = self.selected_zot() else {
            return Empty.into_any_element();
        };
        let body = match self.zots.tabs.active {
            ZotTab::Overview => self.zot_overview(&zot, cx),
            ZotTab::Threads => self.zot_threads(&zot, cx),
            ZotTab::Drive => self.zot_drive(&zot, cx),
            ZotTab::History => self.zot_history(&zot, cx),
        };
        column()
            .id("zot-panel")
            .w(px(340.))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(rgb(BORDER))
            .child(self.zot_tab_bar(&zot, cx))
            .child(
                column()
                    .id("zot-panel-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .gap(px(14.))
                    .child(body),
            )
            .into_any_element()
    }
    fn zot_overview(&self, zot: &Zot, cx: &mut Context<Self>) -> AnyElement {
        let working = zot
            .workers
            .iter()
            .filter(|w| self.zot_thread_status(&w.chat_id) == "working")
            .count();
        let (color, status) = match zot.status {
            ZotStatus::Active => (GREEN, "Active"),
            ZotStatus::Paused => (AMBER, "Paused"),
            ZotStatus::Stopped => (FAINT, "Stopped"),
            ZotStatus::Archived => (FAINT, "Archived"),
        };
        let toggle = if zot.status == ZotStatus::Active {
            ZotStatus::Paused
        } else {
            ZotStatus::Active
        };
        let z = zot.clone();
        let mut view = column().gap(px(14.)).child(
            column()
                .gap(px(6.))
                .child(
                    div()
                        .text_size(px(15.))
                        .text_color(rgb(TEXT))
                        .child(zot.config.name.clone()),
                )
                .when(!zot.config.purpose.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(px(12.5))
                            .text_color(rgb(MUTED))
                            .child(zot.config.purpose.clone()),
                    )
                })
                .child(
                    row()
                        .gap_2()
                        .text_size(px(11.5))
                        .text_color(rgb(FAINT))
                        .child(div().size(px(6.)).rounded_full().bg(rgb(color)))
                        .child(status)
                        .child(format!(
                            "· {} thread{} · {working} working",
                            zot.workers.len(),
                            if zot.workers.len() == 1 { "" } else { "s" }
                        ))
                        .child(div().flex_1())
                        .when(
                            matches!(zot.status, ZotStatus::Active | ZotStatus::Paused),
                            |d| {
                                d.child(
                                    control(
                                        "zot-pause",
                                        if toggle == ZotStatus::Paused {
                                            "Pause"
                                        } else {
                                            "Resume"
                                        },
                                    )
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            this.zot_action(
                                                &z,
                                                ZotAction::SetStatus { status: toggle },
                                                cx,
                                            )
                                        },
                                    )),
                                )
                            },
                        ),
                ),
        );
        let mut needs = column().gap(px(2.)).child(section("Needs you"));
        let mut empty = true;
        for decision in zot.decisions.iter().filter(|d| d.answer.is_none()) {
            empty = false;
            let z = zot.clone();
            let decision = decision.clone();
            needs = needs.child(
                list_row(
                    SharedString::from(format!("decision-{}", decision.id)),
                    "info-circle",
                    AMBER,
                    decision.question.clone(),
                    Some("Answer"),
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
            empty = false;
            let id = event.worker_chat_id.clone();
            needs = needs.child(
                list_row(
                    SharedString::from(format!("zot-event-{}", event.id)),
                    "chat-round-line",
                    MUTED,
                    event.summary.clone(),
                    None,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.open_zot_chat(&id, cx))),
            );
        }
        if empty {
            needs = needs.child(muted("Nothing needs you right now."));
        }
        view = view.child(needs);
        let mut manage = column().gap(px(2.)).child(section("Manage"));
        for (key, name, kind) in [
            ("fresh-zot", "Start a fresh conversation", 0),
            ("zot-Stop Zot", "Stop Zot", 1),
            ("zot-Archive Zot", "Archive Zot and threads", 2),
        ] {
            let z = zot.clone();
            manage = manage.child(control(key, name).on_click(cx.listener(
                move |this, _, _, cx| {
                    let action = match kind {
                        0 => ZotAction::FreshConversation,
                        1 => ZotAction::SetStatus {
                            status: ZotStatus::Stopped,
                        },
                        _ => ZotAction::SetStatus {
                            status: ZotStatus::Archived,
                        },
                    };
                    this.zot_action(&z, action, cx)
                },
            )));
        }
        let z = zot.clone();
        manage = manage.child(control("delete-zot", "Delete…").on_click(cx.listener(
            move |this, _, window, cx| {
                this.open_zot_form(Some(z.clone()), FormKind::Delete, window, cx)
            },
        )));
        view.child(manage).into_any_element()
    }
    fn zot_threads(&self, zot: &Zot, cx: &mut Context<Self>) -> AnyElement {
        let z = zot.clone();
        let mut threads = column().gap(px(2.)).child(
            row()
                .justify_between()
                .child(section(format!("Threads · {}", zot.workers.len())))
                .child(
                    control("new-zot-worker", "New thread").on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.open_zot_form(Some(z.clone()), FormKind::Worker(None), window, cx)
                        },
                    )),
                ),
        );
        if zot.workers.is_empty() {
            threads = threads.child(muted("Threads this Zot starts or adopts appear here."));
        }
        for worker in &zot.workers {
            let id = worker.chat_id.clone();
            let state = self.zot_thread_status(&id);
            let (dot, status) = thread_state(state);
            let open = id.clone();
            let z = zot.clone();
            let message = id.clone();
            let mut actions = row().gap(px(2.)).pl(px(18.)).child(
                control(SharedString::from(format!("message-{id}")), "Message").on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.open_zot_form(
                            Some(z.clone()),
                            FormKind::Worker(Some(message.clone())),
                            window,
                            cx,
                        )
                    }),
                ),
            );
            for (name, kind) in [("Stop", 0), ("Archive", 1), ("Release", 2)] {
                let chat_id = id.clone();
                let z = zot.clone();
                actions = actions.child(
                    control(SharedString::from(format!("worker-{kind}-{id}")), name).on_click(
                        cx.listener(move |this, _, _, cx| {
                            let chat_id = chat_id.clone();
                            let action = match kind {
                                0 => ZotAction::StopWorker { chat_id },
                                1 => ZotAction::ArchiveWorker { chat_id },
                                _ => ZotAction::ReleaseWorker { chat_id },
                            };
                            this.zot_action(&z, action, cx);
                        }),
                    ),
                );
            }
            threads = threads.child(
                column()
                    .py_1()
                    .child(
                        row()
                            .id(SharedString::from(format!("worker-{id}")))
                            .role(Role::Button)
                            .aria_label(SharedString::from(worker.objective.clone()))
                            .gap(px(10.))
                            .px(px(6.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .cursor_pointer()
                            .hover(|d| d.bg(rgb(RAISED)))
                            .child(div().size(px(7.)).flex_none().rounded_full().bg(dot))
                            .child(
                                column()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(px(13.))
                                            .text_color(rgb(TEXT))
                                            .child(worker.objective.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(rgb(FAINT))
                                            .child(status),
                                    ),
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_zot_chat(&open, cx)),
                            ),
                    )
                    .child(actions),
            );
        }
        let mut view = column().gap(px(14.)).child(threads);
        let current = zot.coordinator_chat_id.clone();
        let mut coordinator = column()
            .gap(px(2.))
            .child(section("Coordinator conversations"))
            .child(
                list_row(
                    "zot-current-conversation",
                    "chat-round-line",
                    MUTED,
                    "Current conversation".into(),
                    None,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.open_zot_chat(&current, cx))),
            );
        for (index, id) in zot.previous_coordinator_chats.iter().enumerate().rev() {
            let id = id.clone();
            coordinator = coordinator.child(
                list_row(
                    SharedString::from(format!("zot-old-{id}")),
                    "history",
                    FAINT,
                    format!("Conversation {}", index + 1),
                    None,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.open_zot_chat(&id, cx))),
            );
        }
        view = view.child(coordinator);
        let candidates = self
            .chats
            .iter()
            .filter(|c| {
                c["parentChatId"].is_null()
                    && c["archived"] != true
                    && !zot_owns_chat(&self.zots.items, c["id"].as_str().unwrap_or_default())
                    && zot
                        .config
                        .repositories
                        .iter()
                        .any(|r| Some(r.space_id.as_str()) == c["spaceId"].as_str())
            })
            .take(16)
            .cloned()
            .collect::<Vec<_>>();
        if !candidates.is_empty() {
            let mut adopt = column()
                .gap(px(2.))
                .child(section("Add an existing thread"));
            for chat in candidates {
                let z = zot.clone();
                let title = chat["title"].as_str().unwrap_or("Thread").to_owned();
                adopt = adopt.child(
                    list_row(
                        SharedString::from(format!("adopt-{}", chat["id"])),
                        "plus",
                        FAINT,
                        title.clone(),
                        None,
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
            view = view.child(adopt);
        }
        view.into_any_element()
    }
    fn zot_drive(&self, zot: &Zot, cx: &mut Context<Self>) -> AnyElement {
        let files = if self.zots.history {
            vec![]
        } else {
            self.zots.files.clone()
        };
        let mut view = column()
            .gap(px(8.))
            .child(section(format!("Drive · {}", files.len())));
        if files.is_empty() {
            view = view.child(muted(
                "Rules, notes and the digest this Zot keeps live here.",
            ));
        }
        for file in files {
            let z = zot.clone();
            let preview = file.content.lines().take(6).collect::<Vec<_>>().join("\n");
            view = view.child(
                column()
                    .id(SharedString::from(format!("edit-{}", file.path)))
                    .role(Role::Button)
                    .aria_label(SharedString::from(file.path.clone()))
                    .p(px(10.))
                    .gap(px(6.))
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(SURFACE))
                    .cursor_pointer()
                    .hover(|d| d.border_color(rgb(MUTED)))
                    .child(
                        row()
                            .gap_2()
                            .child(icon("document").size(px(14.)).text_color(rgb(MUTED)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12.5))
                                    .text_color(rgb(TEXT))
                                    .child(file.path.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(FAINT))
                                    .child(format!("v{}", file.version)),
                            ),
                    )
                    .when(!preview.trim().is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(px(11.5))
                                .text_color(rgb(MUTED))
                                .child(preview),
                        )
                    })
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
        view.into_any_element()
    }
    fn zot_history(&self, zot: &Zot, cx: &mut Context<Self>) -> AnyElement {
        let mut files = if self.zots.history {
            self.zots.files.clone()
        } else {
            vec![]
        };
        files.sort_by(|a, b| a.path.cmp(&b.path).then(b.version.cmp(&a.version)));
        let mut view = column().gap(px(2.)).child(section("Document versions"));
        if files.is_empty() {
            view = view.child(muted("No versions yet."));
        }
        for file in files.clone() {
            let current = files
                .iter()
                .filter(|f| f.path == file.path)
                .map(|f| f.version)
                .max()
                .unwrap_or(file.version);
            let z = zot.clone();
            view = view.child(
                row()
                    .gap_2()
                    .px(px(6.))
                    .py(px(5.))
                    .child(icon("document").size(px(13.)).text_color(rgb(FAINT)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.5))
                            .text_color(rgb(TEXT))
                            .child(file.path.clone()),
                    )
                    .child(div().text_size(px(11.)).text_color(rgb(FAINT)).child(
                        if current == file.version {
                            format!("v{} · current", file.version)
                        } else {
                            format!("v{}", file.version)
                        },
                    ))
                    .when(current > file.version, |d| {
                        d.child(
                            control(
                                SharedString::from(format!(
                                    "restore-{}-{}",
                                    file.path, file.version
                                )),
                                "Restore",
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.zot_action(
                                        &z,
                                        ZotAction::RestoreFile {
                                            path: file.path.clone(),
                                            version: file.version,
                                            expected_version: current,
                                        },
                                        cx,
                                    )
                                },
                            )),
                        )
                    }),
            );
        }
        view.into_any_element()
    }
}
const AMBER: u32 = 0xd09a48;
const GREEN: u32 = 0x8ccca4;
const BLUE: u32 = 0x86a9dc;
const RED: u32 = 0xe5a0a0;
fn thread_state(state: &str) -> (Hsla, &'static str) {
    match state {
        "working" => (rgb(BLUE).into(), "Working"),
        "awaitingInput" => (rgb(AMBER).into(), "Waiting for input"),
        "errored" => (rgb(RED).into(), "Failed"),
        "idle" => (rgb(FAINT).into(), "Idle"),
        _ => (Hsla::from(rgb(FAINT)).opacity(0.5), "Not running"),
    }
}
fn section(text: impl Into<SharedString>) -> Div {
    div()
        .pb_1()
        .text_size(px(11.))
        .text_color(rgb(FAINT))
        .child(text.into())
}
fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(6.))
        .py_1()
        .text_size(px(12.))
        .text_color(rgb(FAINT))
        .child(text.into())
}
fn list_row(
    id: impl Into<ElementId>,
    glyph: &'static str,
    tint: u32,
    title: String,
    trailing: Option<&'static str>,
) -> Stateful<Div> {
    row()
        .id(id)
        .role(Role::Button)
        .aria_label(SharedString::from(title.clone()))
        .gap_2()
        .px(px(6.))
        .py(px(5.))
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(12.5))
        .text_color(rgb(TEXT))
        .hover(|d| d.bg(rgb(RAISED)))
        .child(icon(glyph).size(px(13.)).flex_none().text_color(rgb(tint)))
        .child(div().flex_1().min_w_0().truncate().child(title))
        .when_some(trailing, |d, t| {
            d.child(div().text_size(px(11.)).text_color(rgb(FAINT)).child(t))
        })
}
