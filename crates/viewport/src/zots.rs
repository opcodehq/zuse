//! Browser coordinator views use the same commands as the native application.
use super::*;
use crate::assets::icon;
use crate::shell::{BORDER, FAINT, MUTED, RAISED, TEXT, column, icon_button, row};
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
    /// Thread whose actions menu is open, and where it was opened.
    pub menu: Option<(String, Point<Pixels>)>,
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
            ],
            FormKind::File(file) if file.version == 0 => {
                vec![("Name", String::new()), ("Content", String::new())]
            }
            FormKind::File(file) => vec![("Content", file.content.clone())],
            FormKind::Decision(_) => vec![("Your answer", String::new())],
            FormKind::Worker(Some(_)) => vec![("Message", String::new())],
            FormKind::Worker(None) => {
                vec![("Objective", String::new()), ("Base branch", "HEAD".into())]
            }
            FormKind::Delete => vec![],
        };
        let inputs = values
            .into_iter()
            .map(|(name, value)| {
                // Free text gets room to wrap; names and numbers stay on one line.
                let long = matches!(
                    name,
                    "Purpose" | "Content" | "Message" | "Objective" | "Your answer"
                );
                let input = cx.new(|cx| {
                    if long {
                        input::TextInput::multiline(name, window, cx)
                    } else {
                        input::TextInput::new(name, window, cx)
                    }
                });
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
        let get = |key: &str| {
            form.inputs
                .iter()
                .find(|(n, _)| *n == key)
                .map(|(_, i)| i.read(cx).value(cx))
                .unwrap_or_default()
        };
        let action = match &form.kind {
            FormKind::Config => {
                let name = get("Name").trim().to_owned();
                if name.is_empty() {
                    self.error = Some("Name the Zot".into());
                    cx.notify();
                    return;
                }
                // Model and limits aren't edited here: the coordinator's own
                // conversation picks its model, and limits keep their values.
                let composer = self.current_model_config();
                let config = match &form.zot {
                    Some(zot) => ZotConfig {
                        name,
                        purpose: get("Purpose"),
                        home_device_id: form.home.clone(),
                        repositories: form.repositories.clone(),
                        worker_device_id: form.worker_device.clone(),
                        ..zot.config.clone()
                    },
                    None => ZotConfig {
                        name,
                        purpose: get("Purpose"),
                        home_device_id: form.home.clone(),
                        repositories: form.repositories.clone(),
                        coordinator_model: ZotModel {
                            harness: composer
                                .as_ref()
                                .map(|c| c.provider.clone())
                                .unwrap_or_else(|| "claude-code".into()),
                            model: composer.and_then(|c| c.model),
                        },
                        worker_device_id: form.worker_device.clone(),
                        max_concurrent_workers: 4,
                        max_followups: 3,
                        avatar: None,
                    },
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
                        objective: get("Objective"),
                        followups: 0,
                    },
                    // New threads start on the coordinator conversation's current model.
                    model: self
                        .chats
                        .iter()
                        .find(|c| {
                            form.zot
                                .as_ref()
                                .is_some_and(|z| c["id"] == z.coordinator_chat_id)
                        })
                        .and_then(|c| {
                            Some(ZotModel {
                                harness: c["config"]["harness"].as_str()?.to_owned(),
                                model: c["config"]["model"].as_str().map(str::to_owned),
                            })
                        })
                        .or_else(|| {
                            form.zot
                                .as_ref()
                                .map(|z| z.config.coordinator_model.clone())
                        })
                        .unwrap_or(ZotModel {
                            harness: "claude-code".into(),
                            model: None,
                        }),
                    base: {
                        let base = get("Base branch");
                        if base.trim().is_empty() {
                            "HEAD".into()
                        } else {
                            base
                        }
                    },
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
    /// Zot forms open as a dialog over the workspace instead of replacing it.
    pub(super) fn zot_form(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::popover::{self, Kind};
        let form = self.zots.form.as_ref().unwrap();
        let saving = self.zots.pending.is_some();
        let name = |key: &str| {
            form.inputs
                .iter()
                .find(|(n, _)| *n == key)
                .map(|(_, i)| i.clone())
        };
        let title = match &form.kind {
            FormKind::File(file) if file.version == 0 => "New document".to_owned(),
            FormKind::File(file) => file.path.clone(),
            FormKind::Config if form.zot.is_none() => "New Zot".to_owned(),
            FormKind::Config => "Zot settings".to_owned(),
            FormKind::Worker(Some(_)) => "Message thread".to_owned(),
            FormKind::Worker(None) => "New thread".to_owned(),
            FormKind::Decision(_) => "Answer".to_owned(),
            FormKind::Delete => "Delete Zot?".to_owned(),
        };
        let close = cx.listener(|this, _, _, cx| {
            this.zots.form = None;
            cx.notify();
        });
        let mut body = column().px(px(20.)).pb(px(18.)).gap(px(14.));
        match &form.kind {
            FormKind::Config => {
                let zot_name = form
                    .zot
                    .as_ref()
                    .map(|z| z.config.name.clone())
                    .unwrap_or_default();
                body = body.child(
                    column()
                        .items_center()
                        .gap(px(10.))
                        .pt(px(4.))
                        .pb(px(6.))
                        .child(avatar(&zot_name, 56.))
                        .when_some(name("Name"), |d, input| {
                            d.child(
                                div()
                                    .w(px(260.))
                                    .h(px(34.))
                                    .px(px(10.))
                                    .flex()
                                    .items_center()
                                    .rounded(px(8.))
                                    .border_1()
                                    .border_color(rgb(BORDER))
                                    .child(input),
                            )
                        }),
                );
                if let Some(input) = name("Purpose") {
                    body = body.child(field("Purpose", "What this Zot is responsible for", input));
                }
                body = body.child(
                    column()
                        .gap(px(6.))
                        .child(caption("Repositories"))
                        .child(self.zot_repository_list(cx)),
                );
                // Device choice only matters once there is more than one.
                if self.zots.devices.len() > 1 {
                    body = body.child(
                        column()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .child(setting_row(
                                "Runs on",
                                "Where the coordinator lives",
                                self.zot_device_chips(
                                    form.home.clone(),
                                    true,
                                    form.zot.is_some(),
                                    cx,
                                ),
                                true,
                            ))
                            .child(setting_row(
                                "Threads run on",
                                "Default for new threads",
                                self.zot_device_chips(form.worker_device.clone(), false, false, cx),
                                false,
                            )),
                    );
                }
            }
            FormKind::Delete => {
                body = body.child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(19.))
                        .text_color(rgb(MUTED))
                        .child("The coordinator stops and the Zot is removed. Its threads, documents and repository files are kept."),
                );
            }
            _ => {
                if let FormKind::Decision(decision) = &form.kind {
                    body = body.child(
                        div()
                            .p(px(12.))
                            .rounded(px(10.))
                            .bg(rgb(RAISED))
                            .text_size(px(13.))
                            .line_height(px(19.))
                            .text_color(rgb(TEXT))
                            .child(decision.question.clone()),
                    );
                }
                for (key, input) in &form.inputs {
                    let label = match *key {
                        "Base branch" => "Start from",
                        other => other,
                    };
                    body = body.child(field(label, "", input.clone()));
                }
                if matches!(form.kind, FormKind::Worker(None)) && self.zots.devices.len() > 1 {
                    body = body.child(column().gap(px(6.)).child(caption("Runs on")).child(
                        self.zot_device_chips(form.worker_device.clone(), false, false, cx),
                    ));
                }
            }
        }
        let delete = matches!(form.kind, FormKind::Delete);
        let editing = matches!(form.kind, FormKind::Config) && form.zot.is_some();
        let zot = form.zot.clone();
        let mut footer = row()
            .px(px(20.))
            .py(px(14.))
            .gap_2()
            .border_t_1()
            .border_color(rgb(BORDER));
        if editing {
            let z = zot.clone().unwrap();
            let archive = z.clone();
            footer = footer
                .child(
                    popover::button("zot-delete", "Delete", Kind::Destructive).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.open_zot_form(Some(z.clone()), FormKind::Delete, window, cx)
                        }),
                    ),
                )
                .child(
                    popover::button("zot-archive", "Archive", Kind::Secondary).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.zot_action(
                                &archive,
                                ZotAction::SetStatus {
                                    status: ZotStatus::Archived,
                                },
                                cx,
                            );
                            this.zots.form = None;
                        }),
                    ),
                );
        }
        footer = footer
            .child(div().flex_1())
            .child(popover::button("cancel-zot", "Cancel", Kind::Secondary).on_click(close))
            .child(
                popover::button(
                    "save-zot",
                    if saving {
                        "Saving…"
                    } else if delete {
                        "Delete Zot"
                    } else if matches!(form.kind, FormKind::Worker(_)) {
                        "Send"
                    } else if matches!(form.kind, FormKind::Config) && form.zot.is_none() {
                        "Create"
                    } else {
                        "Save"
                    },
                    if delete {
                        Kind::Destructive
                    } else {
                        Kind::Primary
                    },
                )
                .when(saving, |d| d.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| this.save_zot(cx))),
            );
        let (backdrop, card) = popover::dialog(
            "zot-form",
            title.clone(),
            if matches!(form.kind, FormKind::Config) {
                520.
            } else {
                440.
            },
        );
        backdrop
            .child(
                card.child(
                    row()
                        .px(px(20.))
                        .pt(px(16.))
                        .pb(px(10.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(15.))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(TEXT))
                                .child(title),
                        )
                        .child(icon_button("close-zot-form", "close", "Close").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.zots.form = None;
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .id("zot-form-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(body),
                )
                .child(footer),
            )
            .into_any_element()
    }
    fn zot_device_chips(
        &self,
        selected: String,
        home: bool,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut chips = row().flex_wrap().justify_end().gap_1();
        for device in &self.zots.devices {
            let id = device["id"].as_str().unwrap_or_default().to_owned();
            let name = device["name"]
                .as_str()
                .filter(|n| !n.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    if self.device_id.as_deref() == Some(id.as_str()) {
                        "This device".to_owned()
                    } else {
                        "Unnamed device".to_owned()
                    }
                });
            let active = selected == id;
            chips = chips.child(
                chip(
                    SharedString::from(format!("zot-device-{home}-{id}")),
                    name,
                    active,
                )
                .when(locked && !active, |d| d.opacity(0.4))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = this.zots.form.as_mut() {
                        if home {
                            if form.zot.is_none() {
                                form.home = id.clone();
                            }
                        } else {
                            form.worker_device = id.clone();
                        }
                    }
                    cx.notify();
                })),
            );
        }
        chips.into_any_element()
    }
    /// Projects as a checklist: name, location and a tick on the right.
    fn zot_repository_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.zots.form else {
            return Empty.into_any_element();
        };
        if self.zots.spaces.is_empty() {
            return muted("Open a project first to give this Zot a repository.").into_any_element();
        }
        let mut list = column()
            .p(px(4.))
            .gap(px(2.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(BORDER));
        for space in &self.zots.spaces {
            let id = space["id"].as_str().unwrap_or_default().to_owned();
            let device = space["deviceId"].as_str().unwrap_or_default().to_owned();
            let path = space["path"].as_str().unwrap_or_default().to_owned();
            let name = space["name"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| path.rsplit('/').next().unwrap_or(&path).to_owned());
            let checked = form.repositories.iter().any(|r| r.space_id == id);
            list = list.child(
                row()
                    .id(SharedString::from(format!("zot-repo-{id}")))
                    .role(Role::CheckBox)
                    .aria_label(SharedString::from(name.clone()))
                    .h(px(40.))
                    .px(px(8.))
                    .gap(px(10.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(RAISED)))
                    .child(
                        row()
                            .size(px(26.))
                            .flex_none()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(rgb(RAISED))
                            .child(icon("folder").size(px(13.)).text_color(rgb(MUTED))),
                    )
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(rgb(TEXT))
                                    .child(name),
                            )
                            .when(!path.is_empty(), |d| {
                                d.child(
                                    div()
                                        .truncate()
                                        .text_size(px(11.))
                                        .text_color(rgb(FAINT))
                                        .child(path.clone()),
                                )
                            }),
                    )
                    .child(
                        row()
                            .size(px(18.))
                            .flex_none()
                            .justify_center()
                            .rounded(px(5.))
                            .when(checked, |d| {
                                d.bg(rgb(TEXT)).child(
                                    icon("check")
                                        .size(px(12.))
                                        .text_color(crate::settings_style::card_bg()),
                                )
                            })
                            .when(!checked, |d| d.border_1().border_color(rgb(MUTED))),
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
        list.into_any_element()
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
            self.zots.menu = None;
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
            let color = rgb(if active { TEXT } else { FAINT });
            let base = row()
                .id(SharedString::from(format!("zot-tab-{name}")))
                .role(Role::Tab)
                .aria_label(name)
                .h(px(28.))
                .px(px(7.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .when(!active, |d| d.hover(|d| d.text_color(rgb(MUTED))))
                .child(icon(tab.icon()).size(px(15.)).text_color(color))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.zots.tabs.select(tab) {
                        let history = tab == ZotTab::History;
                        if this.zots.history != history {
                            this.zots.history = history;
                            this.refresh_zot();
                        }
                        this.zots.menu = None;
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
                    Animation::new(std::time::Duration::from_millis(220)).with_easing(ease_in_out),
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
            .px(px(8.))
            .child(strip)
            .child(div().flex_1())
            .when(tabs.active == ZotTab::Drive, |d| {
                d.child(
                    quiet_icon("zot-new-document", "document-add", "New document").on_click(
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
            .when(tabs.active == ZotTab::Threads, |d| {
                let z = zot.clone();
                d.child(
                    quiet_icon("new-zot-worker", "plus", "New thread").on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.open_zot_form(Some(z.clone()), FormKind::Worker(None), window, cx)
                        },
                    )),
                )
            })
            .child(
                quiet_icon("zot-settings", "settings", "Zot settings").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.open_zot_form(Some(settings.clone()), FormKind::Config, window, cx)
                    },
                )),
            )
            .into_any_element()
    }
    /// The panel floats as its own rounded surface beside the conversation.
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
        crate::workspace_style::column()
            .w(px(336.))
            .flex_none()
            .pr(px(crate::workspace_style::GUTTER))
            .child(
                crate::workspace_style::surface()
                    .child(self.zot_tab_bar(&zot, cx))
                    .child(
                        column()
                            .id("zot-panel-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px(px(12.))
                            .pt(px(4.))
                            .pb(px(16.))
                            .gap(px(18.))
                            .child(body),
                    ),
            )
            .children(self.zot_thread_menu(&zot, cx))
            .into_any_element()
    }
    fn zot_overview(&self, zot: &Zot, cx: &mut Context<Self>) -> AnyElement {
        let working = zot
            .workers
            .iter()
            .filter(|w| self.zot_thread_status(&w.chat_id) == "working")
            .count();
        let decisions: Vec<_> = zot
            .decisions
            .iter()
            .filter(|d| d.answer.is_none())
            .cloned()
            .collect();
        let events: Vec<_> = self
            .zots
            .events
            .iter()
            .filter(|e| e.zot_id == zot.id && !e.acknowledged && e.kind != "settled")
            .cloned()
            .collect();
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
        let hero = row()
            .items_start()
            .gap(px(12.))
            .px(px(4.))
            .pt(px(6.))
            .child(avatar(&zot.config.name, 40.))
            .child(
                column()
                    .flex_1()
                    .min_w_0()
                    .gap(px(3.))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(15.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT))
                            .child(zot.config.name.clone()),
                    )
                    .child(
                        row()
                            .gap(px(6.))
                            .text_size(px(11.5))
                            .text_color(rgb(MUTED))
                            .child(div().size(px(6.)).rounded_full().bg(rgb(color)))
                            .child(status),
                    ),
            )
            .when(
                matches!(zot.status, ZotStatus::Active | ZotStatus::Paused),
                |d| {
                    d.child(
                        crate::popover::button(
                            "zot-pause",
                            if toggle == ZotStatus::Paused {
                                "Pause"
                            } else {
                                "Resume"
                            },
                            crate::popover::Kind::Secondary,
                        )
                        .h(px(26.))
                        .px(px(10.))
                        .text_size(px(12.))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.zot_action(&z, ZotAction::SetStatus { status: toggle }, cx)
                        })),
                    )
                },
            );
        let stats = row()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(BORDER))
            .child(stat("Threads", zot.workers.len(), false))
            .child(div().w(px(1.)).h(px(34.)).bg(rgb(BORDER)))
            .child(stat("Working", working, false))
            .child(div().w(px(1.)).h(px(34.)).bg(rgb(BORDER)))
            .child(stat(
                "Needs you",
                decisions.len() + events.len(),
                decisions.len() + events.len() > 0,
            ));
        let mut view = column()
            .gap(px(16.))
            .child(hero)
            .when(!zot.config.purpose.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(4.))
                        .text_size(px(12.5))
                        .line_height(px(19.))
                        .text_color(rgb(MUTED))
                        .child(zot.config.purpose.clone()),
                )
            })
            .child(stats);
        let mut needs = group();
        for decision in decisions.iter().cloned() {
            let z = zot.clone();
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
        for event in events.iter() {
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
        view = view.child(column().gap(px(6.)).child(section("Needs you")).child(
            if decisions.is_empty() && events.is_empty() {
                empty_state(
                    "check-circle",
                    "All clear",
                    "Decisions and blocked threads show up here.",
                )
            } else {
                needs.into_any_element()
            },
        ));
        let current = zot.coordinator_chat_id.clone();
        view.child(
            column().gap(px(6.)).child(section("Coordinator")).child(
                group()
                    .child(
                        list_row(
                            "zot-current-conversation",
                            "chat-round-line",
                            MUTED,
                            "Current conversation".into(),
                            None,
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_zot_chat(&current, cx)),
                        ),
                    )
                    .children(zot.previous_coordinator_chats.iter().enumerate().rev().map(
                        |(index, id)| {
                            let id = id.clone();
                            list_row(
                                SharedString::from(format!("zot-old-{id}")),
                                "history",
                                FAINT,
                                format!("Earlier conversation {}", index + 1),
                                None,
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_zot_chat(&id, cx)),
                            )
                        },
                    ))
                    .child({
                        let z = zot.clone();
                        list_row(
                            "fresh-zot",
                            "restart",
                            FAINT,
                            "Start a fresh conversation".into(),
                            None,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.zot_action(&z, ZotAction::FreshConversation, cx)
                        }))
                    }),
            ),
        )
        .into_any_element()
    }
    fn zot_threads(&self, zot: &Zot, cx: &mut Context<Self>) -> AnyElement {
        let mut threads = group();
        for worker in &zot.workers {
            let id = worker.chat_id.clone();
            let (dot, status) = thread_state(self.zot_thread_status(&id));
            let title = self
                .chats
                .iter()
                .find(|c| c["id"] == id)
                .and_then(|c| c["title"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| worker.objective.clone());
            let open = id.clone();
            let menu = id.clone();
            let open_menu = self.zots.menu.as_ref().is_some_and(|(m, _)| m == &id);
            threads = threads.child(
                row()
                    .id(SharedString::from(format!("worker-{id}")))
                    .group(SharedString::from(format!("worker-row-{id}")))
                    .role(Role::Button)
                    .aria_label(SharedString::from(title.clone()))
                    .h(px(40.))
                    .px(px(10.))
                    .gap(px(10.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(RAISED)))
                    .when(open_menu, |d| d.bg(rgb(RAISED)))
                    .child(div().size(px(7.)).flex_none().rounded_full().bg(dot))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .text_color(rgb(TEXT))
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(FAINT))
                            .when(!open_menu, |d| {
                                d.group_hover(SharedString::from(format!("worker-row-{id}")), |d| {
                                    d.invisible()
                                })
                            })
                            .when(open_menu, |d| d.invisible())
                            .child(status),
                    )
                    .child(
                        quiet_icon(
                            SharedString::from(format!("worker-menu-{id}")),
                            "more-horizontal",
                            "Thread actions",
                        )
                        .absolute()
                        .right(px(6.))
                        .when(!open_menu, |d| {
                            d.invisible()
                                .group_hover(SharedString::from(format!("worker-row-{id}")), |d| {
                                    d.visible()
                                })
                        })
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.zots.menu =
                                    if this.zots.menu.as_ref().is_some_and(|(m, _)| m == &menu) {
                                        None
                                    } else {
                                        Some((menu.clone(), window.mouse_position()))
                                    };
                                cx.notify();
                            },
                        )),
                    )
                    .relative()
                    .on_click(cx.listener(move |this, _, _, cx| this.open_zot_chat(&open, cx))),
            );
        }
        let mut view = column().gap(px(16.)).child(
            column()
                .gap(px(6.))
                .child(section(format!("Threads · {}", zot.workers.len())))
                .child(if zot.workers.is_empty() {
                    empty_state(
                        "chat-round-line",
                        "No threads yet",
                        "Threads this Zot starts or adopts appear here.",
                    )
                } else {
                    threads.into_any_element()
                }),
        );
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
            let mut adopt = group();
            for chat in candidates {
                let z = zot.clone();
                let title = chat["title"].as_str().unwrap_or("Thread").to_owned();
                adopt = adopt.child(
                    list_row(
                        SharedString::from(format!("adopt-{}", chat["id"])),
                        "plus",
                        FAINT,
                        title.clone(),
                        Some("Add"),
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
            view = view.child(
                column()
                    .gap(px(6.))
                    .child(section("Add from this project"))
                    .child(adopt),
            );
        }
        view.into_any_element()
    }
    /// Per-thread actions float beside the row instead of crowding it.
    fn zot_thread_menu(&self, zot: &Zot, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::popover;
        let (id, at) = self.zots.menu.clone()?;
        zot.workers.iter().any(|w| w.chat_id == id).then_some(())?;
        let item = |key: &'static str,
                    glyph,
                    label,
                    destructive,
                    action: Option<fn(String) -> ZotAction>| {
            let id = id.clone();
            let z = zot.clone();
            popover::item(
                SharedString::from(format!("zot-thread-{key}")),
                glyph,
                label,
                destructive,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.zots.menu = None;
                match action {
                    Some(action) => this.zot_action(&z, action(id.clone()), cx),
                    None => this.open_zot_form(
                        Some(z.clone()),
                        FormKind::Worker(Some(id.clone())),
                        window,
                        cx,
                    ),
                }
                cx.notify();
            }))
        };
        let open = id.clone();
        let menu = popover::menu("zot-thread-menu", 200.)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.zots.menu = None;
                cx.notify();
            }))
            .child(
                popover::item("zot-thread-open", "arrow-up-right", "Open thread", false)
                    .on_click(cx.listener(move |this, _, _, cx| this.open_zot_chat(&open, cx))),
            )
            .child(item(
                "message",
                "chat-round-line",
                "Send a message",
                false,
                None,
            ))
            .child(popover::separator())
            .child(item(
                "stop",
                "stop",
                "Stop",
                false,
                Some(|chat_id| ZotAction::StopWorker { chat_id }),
            ))
            .child(item(
                "release",
                "arrow-up-right",
                "Release from Zot",
                false,
                Some(|chat_id| ZotAction::ReleaseWorker { chat_id }),
            ))
            .child(item(
                "archive",
                "archive-minimalistic",
                "Archive",
                true,
                Some(|chat_id| ZotAction::ArchiveWorker { chat_id }),
            ));
        Some(popover::anchored_at(at - point(px(180.), px(-10.)), menu))
    }
    fn zot_drive(&self, zot: &Zot, cx: &mut Context<Self>) -> AnyElement {
        let files = if self.zots.history {
            vec![]
        } else {
            self.zots.files.clone()
        };
        let mut list = group();
        for file in files.iter().cloned() {
            let z = zot.clone();
            let preview = file
                .content
                .lines()
                .map(|l| l.trim().trim_start_matches(['#', '-', '*', '>']).trim())
                .find(|l| !l.is_empty())
                .unwrap_or("Empty")
                .to_owned();
            list = list.child(
                row()
                    .id(SharedString::from(format!("edit-{}", file.path)))
                    .role(Role::Button)
                    .aria_label(SharedString::from(file.path.clone()))
                    .px(px(10.))
                    .py(px(8.))
                    .gap(px(10.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|d| d.bg(rgb(RAISED)))
                    .child(
                        row()
                            .size(px(28.))
                            .flex_none()
                            .justify_center()
                            .rounded(px(7.))
                            .bg(rgb(RAISED))
                            .child(icon("document").size(px(14.)).text_color(rgb(MUTED))),
                    )
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .gap(px(1.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .text_color(rgb(TEXT))
                                    .child(file.path.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.5))
                                    .text_color(rgb(FAINT))
                                    .child(preview),
                            ),
                    )
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
        column()
            .gap(px(6.))
            .child(section(format!("Documents · {}", files.len())))
            .child(if files.is_empty() {
                empty_state(
                    "folder-open",
                    "Nothing here yet",
                    "Rules, notes and the digest this Zot keeps.",
                )
            } else {
                list.into_any_element()
            })
            .into_any_element()
    }
    fn zot_history(&self, zot: &Zot, cx: &mut Context<Self>) -> AnyElement {
        let mut files = if self.zots.history {
            self.zots.files.clone()
        } else {
            vec![]
        };
        files.sort_by(|a, b| a.path.cmp(&b.path).then(b.version.cmp(&a.version)));
        if files.is_empty() {
            return empty_state(
                "history",
                "No history yet",
                "Every edit to a document is kept here.",
            );
        }
        let mut view = column().gap(px(16.));
        let mut paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        paths.dedup();
        for path in paths {
            let versions: Vec<_> = files.iter().filter(|f| f.path == path).cloned().collect();
            let current = versions.iter().map(|f| f.version).max().unwrap_or(0);
            let mut list = group();
            for file in versions {
                let z = zot.clone();
                let latest = file.version == current;
                let group_id =
                    SharedString::from(format!("version-{}-{}", file.path, file.version));
                list = list.child(
                    row()
                        .id(group_id.clone())
                        .group(group_id.clone())
                        .h(px(34.))
                        .px(px(10.))
                        .gap(px(10.))
                        .rounded(px(8.))
                        .hover(|d| d.bg(rgb(RAISED)))
                        .child(
                            div()
                                .size(px(7.))
                                .flex_none()
                                .rounded_full()
                                .when(latest, |d| d.bg(rgb(GREEN)))
                                .when(!latest, |d| d.border_1().border_color(rgb(FAINT))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.5))
                                .text_color(rgb(if latest { TEXT } else { MUTED }))
                                .child(format!("Version {}", file.version)),
                        )
                        .when(latest, |d| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(FAINT))
                                    .child("Current"),
                            )
                        })
                        .when(!latest, |d| {
                            d.child(
                                crate::popover::button(
                                    SharedString::from(format!(
                                        "restore-{}-{}",
                                        file.path, file.version
                                    )),
                                    "Restore",
                                    crate::popover::Kind::Secondary,
                                )
                                .h(px(24.))
                                .px(px(8.))
                                .text_size(px(11.5))
                                .invisible()
                                .group_hover(group_id.clone(), |d| d.visible())
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
            view = view.child(
                column()
                    .gap(px(6.))
                    .child(
                        row()
                            .gap(px(6.))
                            .child(icon("document").size(px(12.)).text_color(rgb(FAINT)))
                            .child(section(path)),
                    )
                    .child(list),
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
        "awaitingInput" => (rgb(AMBER).into(), "Needs input"),
        "errored" => (rgb(RED).into(), "Failed"),
        "idle" => (rgb(FAINT).into(), "Idle"),
        _ => (Hsla::from(rgb(FAINT)).opacity(0.5), "Not running"),
    }
}
/// The Zot's mark: its initial on a soft tile.
fn avatar(name: &str, size: f32) -> Div {
    row()
        .size(px(size))
        .flex_none()
        .justify_center()
        .rounded(px(size * 0.28))
        .bg(Hsla::from(rgb(AMBER)).opacity(0.16))
        .text_size(px(size * 0.42))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(AMBER))
        .child(
            name.chars()
                .next()
                .unwrap_or('Z')
                .to_uppercase()
                .to_string(),
        )
}
fn section(text: impl Into<SharedString>) -> Div {
    div()
        .px(px(4.))
        .text_size(px(11.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(FAINT))
        .child(text.into())
}
fn caption(text: &'static str) -> Div {
    div().text_size(px(12.)).text_color(rgb(MUTED)).child(text)
}
fn muted(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(12.))
        .text_color(rgb(FAINT))
        .child(text.into())
}
/// Rows sharing one quiet container.
fn group() -> Div {
    column().gap(px(1.))
}
fn empty_state(glyph: &'static str, title: &'static str, body: &'static str) -> AnyElement {
    column()
        .items_center()
        .gap(px(4.))
        .py(px(22.))
        .px(px(16.))
        .rounded(px(10.))
        .border_1()
        .border_dashed()
        .border_color(rgb(BORDER))
        .child(icon(glyph).size(px(18.)).text_color(rgb(FAINT)))
        .child(
            div()
                .pt(px(4.))
                .text_size(px(12.5))
                .text_color(rgb(MUTED))
                .child(title),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(rgb(FAINT))
                .text_center()
                .child(body),
        )
        .into_any_element()
}
fn stat(name: &'static str, value: usize, alert: bool) -> Div {
    column()
        .flex_1()
        .py(px(8.))
        .items_center()
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(if alert { AMBER } else { TEXT }))
                .child(value.to_string()),
        )
        .child(div().text_size(px(11.)).text_color(rgb(FAINT)).child(name))
}
fn quiet_icon(id: impl Into<ElementId>, glyph: &'static str, name: &'static str) -> Stateful<Div> {
    row()
        .id(id)
        .role(Role::Button)
        .aria_label(name)
        .cursor_pointer()
        .justify_center()
        .size(px(26.))
        .rounded(px(7.))
        .text_color(rgb(FAINT))
        .hover(|d| d.bg(rgb(RAISED)).text_color(rgb(TEXT)))
        .child(icon(glyph).size(px(15.)))
}
fn chip(id: impl Into<ElementId>, name: String, active: bool) -> Stateful<Div> {
    row()
        .id(id)
        .role(Role::CheckBox)
        .aria_label(SharedString::from(name.clone()))
        .cursor_pointer()
        .h(px(26.))
        .px(px(10.))
        .gap(px(6.))
        .rounded_full()
        .border_1()
        .text_size(px(12.))
        .when(active, |d| {
            d.border_color(rgb(MUTED))
                .bg(rgb(RAISED))
                .text_color(rgb(TEXT))
        })
        .when(!active, |d| {
            d.border_color(rgb(BORDER))
                .text_color(rgb(MUTED))
                .hover(|d| d.text_color(rgb(TEXT)))
        })
        .when(active, |d| d.child(icon("check").size(px(11.))))
        .child(name)
}
/// Label and hint on the left, control on the right, like a settings row.
fn setting_row(name: &'static str, hint: &'static str, control: AnyElement, first: bool) -> Div {
    row()
        .px(px(12.))
        .py(px(10.))
        .gap(px(12.))
        .when(!first, |d| d.border_t_1().border_color(rgb(BORDER)))
        .child(
            column()
                .flex_none()
                .w(px(170.))
                .gap(px(1.))
                .child(div().text_size(px(12.5)).text_color(rgb(TEXT)).child(name))
                .child(div().text_size(px(11.)).text_color(rgb(FAINT)).child(hint)),
        )
        .child(row().flex_1().min_w_0().justify_end().child(control))
}
/// A labelled input with an outlined box.
fn field(name: &'static str, hint: &'static str, input: Entity<input::TextInput>) -> Div {
    column()
        .gap(px(6.))
        .child(
            row()
                .gap_2()
                .child(caption(name))
                .when(!hint.is_empty(), |d| {
                    d.child(div().text_size(px(11.)).text_color(rgb(FAINT)).child(hint))
                }),
        )
        .child(
            div()
                .min_h(px(34.))
                .px(px(10.))
                .py(px(7.))
                .rounded(px(8.))
                .border_1()
                .border_color(rgb(BORDER))
                .child(input),
        )
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
        .h(px(34.))
        .px(px(10.))
        .gap(px(10.))
        .rounded(px(8.))
        .cursor_pointer()
        .text_size(px(13.))
        .text_color(rgb(TEXT))
        .hover(|d| d.bg(rgb(RAISED)))
        .child(icon(glyph).size(px(14.)).flex_none().text_color(rgb(tint)))
        .child(div().flex_1().min_w_0().truncate().child(title))
        .when_some(trailing, |d, t| {
            d.child(div().text_size(px(11.5)).text_color(rgb(FAINT)).child(t))
        })
}
