//! Browser coordinator views use the same commands as the native application.
use super::*;
use crate::assets::icon;
use crate::shell::{BORDER, FAINT, MUTED, RAISED, TEXT, column, icon_button, row};
use zuse_protocol::presentation::zapper_panel::{ZapperTab, ZapperTabs, zapper_owns_chat};
use zuse_protocol::{control as api, zappers::*};

/// The selected Zapper's inspector width; the shell reserves the same space.
pub(crate) const ZAPPER_PANEL_WIDTH: f32 = 520.;

#[derive(Default)]
pub(super) struct ZapperView {
    pub items: Vec<Zapper>,
    pub watches: HashMap<u64, String>,
    pub form: Option<Form>,
    spaces: Vec<Value>,
    devices: Vec<Value>,
    files: Vec<ZapperFile>,
    events: Vec<ZapperEvent>,
    tabs: ZapperTabs,
    issues: Option<Entity<crate::zapper_issues::IssuePanel>>,
    pull_request: Option<String>,
    document: Option<ZapperFile>,
    history: bool,
    pending: Option<u64>,
    /// Whether the pending command came from the issue panel, so only its own
    /// results close or fail the panel's editor.
    pending_issue: bool,
    issue_command: bool,
    selection: Option<String>,
    retry: Option<ZapperCommand>,
    file_request: Option<u64>,
    event_request: Option<u64>,
    /// Thread whose actions menu is open, and where it was opened.
    pub menu: Option<(String, Point<Pixels>)>,
}
pub(super) struct Form {
    id: String,
    zapper: Option<Zapper>,
    kind: FormKind,
    inputs: Vec<(&'static str, Entity<input::TextInput>)>,
    repositories: Vec<ZapperRepository>,
    home: String,
    worker_device: String,
}
enum FormKind {
    Config,
    File(ZapperFile),
    Worker(Option<String>),
    Decision(ZapperDecision),
    Delete,
}

impl WorkspaceView {
    /// Send a Zapper call under its pre-rename name and fields: this hosted
    /// client can reach an engine that predates the rename, and every engine
    /// accepts the old names. Replies are still matched under the current
    /// name. Legacy, remove after one release.
    fn zapper_call(&mut self, method: &'static str, params: Value) -> u64 {
        let wire = zuse_protocol::zappers::legacy::legacy_method(method).unwrap_or(method);
        let id = self.call(
            wire,
            zuse_protocol::zappers::legacy::to_legacy_params(params),
        );
        if let Some(pending) = self.pending.get_mut(&id) {
            *pending = method.into();
        }
        id
    }
    pub(super) fn watch_zapper_home(&mut self, home: String) {
        if self.zappers.watches.values().any(|id| id == &home) {
            return;
        }
        let id = self.zapper_call(api::WATCH_ZAPPERS, json!({"targetDeviceId":home}));
        self.zappers.watches.insert(id, home);
    }
    pub(super) fn selected_zapper(&self) -> Option<Zapper> {
        let id = self.selected.as_ref()?.get("id")?.as_str()?;
        self.zappers
            .items
            .iter()
            .find(|z| z.coordinator_chat_id == id)
            .cloned()
    }
    pub(super) fn refresh_zapper(&mut self) {
        if let Some(zapper) = self.selected_zapper() {
            let params = json!({"zapperId":zapper.id,"targetDeviceId":zapper.config.home_device_id,"history":self.zappers.history});
            self.zappers.file_request = Some(self.zapper_call(api::ZAPPER_FILES, params.clone()));
            self.zappers.event_request = Some(self.zapper_call(api::ZAPPER_EVENTS, params));
        }
    }
    pub(super) fn receive_zapper(
        &mut self,
        id: u64,
        method: &str,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let value = frame.get("item").or_else(|| frame.get("ok"));
        if method == methods::WATCH_CHATS {
            if let (Some(value), Some(selected)) = (value, self.zappers.selection.clone()) {
                if let Some(chat) = value
                    .as_array()
                    .and_then(|chats| chats.iter().find(|c| c["id"] == selected))
                    .cloned()
                {
                    self.zappers.selection = None;
                    self.select(chat, cx);
                }
            }
            return false;
        }
        if !matches!(
            method,
            api::WATCH_ZAPPERS
                | api::ZAPPER_COMMAND
                | api::ZAPPER_FILES
                | api::ZAPPER_EVENTS
                | methods::WATCH_SPACES
                | methods::WATCH_DEVICES
        ) {
            return false;
        }
        if let Some(error) = frame.get("err").and_then(Value::as_str) {
            if method != api::WATCH_ZAPPERS {
                self.error = Some(error.into());
            }
            if self.zappers.pending == Some(id) {
                // The engine answered: nothing was written, so the next
                // attempt is a new command at the current revision.
                if !command_outcome_unknown(error) {
                    self.zappers.retry = None;
                }
                if let Some(panel) = self
                    .zappers
                    .issues
                    .as_ref()
                    .filter(|_| self.zappers.pending_issue)
                {
                    panel.update(cx, |p, cx| p.failed(error.to_owned(), cx));
                }
                self.zappers.pending = None;
            }
            self.pending.remove(&id);
            cx.notify();
            return true;
        }
        if let Some(value) = value {
            match method {
                api::WATCH_ZAPPERS => {
                    if let Ok(items) = serde_json::from_value::<Vec<Zapper>>(value.clone()) {
                        let home = self.zappers.watches.get(&id).cloned().unwrap_or_default();
                        let old = self.selected_zapper();
                        self.zappers
                            .items
                            .retain(|z| z.config.home_device_id != home);
                        crate::zapper_avatar::remember_zappers(&items);
                        self.zappers.items.extend(items);
                        if old != self.selected_zapper() {
                            self.refresh_zapper();
                        }
                    }
                }
                methods::WATCH_SPACES => {
                    self.zappers.spaces = value.as_array().cloned().unwrap_or_default()
                }
                methods::WATCH_DEVICES => {
                    self.zappers.devices = value.as_array().cloned().unwrap_or_default();
                    let devices = self
                        .zappers
                        .devices
                        .iter()
                        .filter_map(|d| d["id"].as_str().map(str::to_owned))
                        .collect::<Vec<_>>();
                    for device in devices {
                        self.watch_zapper_home(device);
                    }
                }
                api::ZAPPER_COMMAND if self.zappers.pending == Some(id) => {
                    self.zappers.pending = None;
                    self.zappers.retry = None;
                    if let Some(panel) = self
                        .zappers
                        .issues
                        .as_ref()
                        .filter(|_| self.zappers.pending_issue)
                    {
                        panel.update(cx, |p, cx| p.committed(cx));
                    }
                    self.zappers.form = None;
                    if let Ok(Some(zapper)) =
                        serde_json::from_value::<Option<Zapper>>(value.clone())
                    {
                        if let Some(chat) = self
                            .chats
                            .iter()
                            .find(|c| c["id"] == zapper.coordinator_chat_id)
                            .cloned()
                        {
                            self.select(chat, cx);
                        } else {
                            self.zappers.selection = Some(zapper.coordinator_chat_id);
                        }
                    }
                    self.refresh_zapper();
                }
                api::ZAPPER_FILES if self.zappers.file_request == Some(id) => {
                    self.zappers.files = serde_json::from_value(value.clone()).unwrap_or_default()
                }
                api::ZAPPER_EVENTS if self.zappers.event_request == Some(id) => {
                    self.zappers.events = serde_json::from_value(
                        zuse_protocol::zappers::legacy::events_from_legacy(value.clone()),
                    )
                    .unwrap_or_default()
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
    pub(super) fn zapper_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
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
                .child("Zappers")
                .child(
                    icon_button("new-zapper", "plus", "New Zapper")
                        .size(px(20.))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_zapper_form(None, FormKind::Config, window, cx)
                        })),
                ),
        );
        for z in &self.zappers.items {
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
                    .id(SharedString::from(format!("zapper-{}", z.id)))
                    .role(Role::Button)
                    .aria_label(SharedString::from(z.config.name.clone()))
                    .h(px(26.))
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
                    .when(z.status != ZapperStatus::Active, |d| {
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
                    .on_click(cx.listener(move |this, _, _, cx| this.open_zapper_chat(&id, cx))),
            );
            // A Zapper's threads live under it instead of under their projects.
            let threads: Vec<_> = z
                .workers
                .iter()
                .filter(|w| !self.zapper_thread_archived(&w.chat_id))
                .collect();
            let total = threads.len();
            for (index, worker) in threads.into_iter().enumerate() {
                let chat = worker.chat_id.clone();
                let current = selected.as_deref() == Some(chat.as_str());
                let (dot, _) = thread_state(self.zapper_thread_status(&chat));
                let title = self
                    .chats
                    .iter()
                    .find(|c| c["id"] == chat)
                    .and_then(|c| c["title"].as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| worker.objective.clone());
                list = list.child(
                    row()
                        .id(SharedString::from(format!("zapper-thread-{chat}")))
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
                        // Tree guide joining the threads to their Zapper.
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
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_zapper_chat(&chat, cx)),
                        ),
                );
            }
        }
        list.into_any_element()
    }
    fn open_zapper_form(
        &mut self,
        zapper: Option<Zapper>,
        kind: FormKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let home = zapper
            .as_ref()
            .map(|z| z.config.home_device_id.clone())
            .or_else(|| self.device_id.clone())
            .unwrap_or_default();
        let repositories = zapper
            .as_ref()
            .map(|z| z.config.repositories.clone())
            .unwrap_or_default();
        let worker_device = zapper
            .as_ref()
            .map(|z| z.config.worker_device_id.clone())
            .unwrap_or_else(|| home.clone());
        let values = match &kind {
            FormKind::Config => vec![
                (
                    "Name",
                    zapper
                        .as_ref()
                        .map(|z| z.config.name.clone())
                        .unwrap_or_default(),
                ),
                (
                    "Purpose",
                    zapper
                        .as_ref()
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
        self.zappers.form = Some(Form {
            id: zapper
                .as_ref()
                .map(|z| z.id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            zapper,
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
    fn zapper_action(&mut self, zapper: &Zapper, action: ZapperAction, cx: &mut Context<Self>) {
        self.send_zapper(
            zapper.id.clone(),
            Some(zapper.revision),
            zapper.config.home_device_id.clone(),
            action,
            cx,
        );
    }
    fn send_zapper(
        &mut self,
        id: String,
        revision: Option<u32>,
        home: String,
        action: ZapperAction,
        cx: &mut Context<Self>,
    ) {
        let from_issue_panel = std::mem::take(&mut self.zappers.issue_command);
        if self.zappers.pending.is_some() || !self.require_connection(cx) {
            return;
        }
        let mut command = ZapperCommand {
            command_id: uuid::Uuid::new_v4().to_string(),
            zapper_id: id,
            expected_revision: revision,
            action,
        };
        if let ZapperAction::Delegate { worker, .. } = &mut command.action {
            worker.chat_id = format!("zapper-worker-{}", command.command_id);
        }
        if let Some(previous) = &self.zappers.retry {
            let mut comparison = command.clone();
            comparison.command_id = previous.command_id.clone();
            comparison.expected_revision = previous.expected_revision;
            if let ZapperAction::Delegate { worker, .. } = &mut comparison.action {
                worker.chat_id = format!("zapper-worker-{}", previous.command_id);
            }
            if serde_json::to_value(comparison).ok() == serde_json::to_value(previous).ok() {
                command = previous.clone();
            }
        }
        self.zappers.retry = Some(command.clone());
        let mut params = serde_json::to_value(command).unwrap();
        params["targetDeviceId"] = json!(home);
        self.zappers.pending = Some(self.zapper_call(api::ZAPPER_COMMAND, params));
        self.zappers.pending_issue = from_issue_panel;
        cx.notify();
    }
    fn save_zapper(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &self.zappers.form else {
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
                    self.error = Some("Name the Zapper".into());
                    cx.notify();
                    return;
                }
                // Model and limits aren't edited here: the coordinator's own
                // conversation picks its model, and limits keep their values.
                let composer = self.current_model_config();
                let config = match &form.zapper {
                    Some(zapper) => ZapperConfig {
                        name,
                        purpose: get("Purpose"),
                        home_device_id: form.home.clone(),
                        repositories: form.repositories.clone(),
                        worker_device_id: form.worker_device.clone(),
                        ..zapper.config.clone()
                    },
                    None => ZapperConfig {
                        name,
                        purpose: get("Purpose"),
                        home_device_id: form.home.clone(),
                        repositories: form.repositories.clone(),
                        coordinator_model: ZapperModel {
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
                        avatar_image: None,
                    },
                };
                if form.zapper.is_some() {
                    ZapperAction::Configure { config }
                } else {
                    ZapperAction::Create { config }
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
                ZapperAction::WriteFile {
                    path,
                    content,
                    expected_version: file.version,
                }
            }
            FormKind::Decision(decision) => {
                let mut decision = decision.clone();
                decision.answer = Some(values[0].clone());
                ZapperAction::Decide { decision }
            }
            FormKind::Worker(Some(id)) => ZapperAction::MessageWorker {
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
                ZapperAction::Delegate {
                    issue_id: None,
                    worker: ZapperWorker {
                        chat_id: String::new(),
                        device_id: repository.device_id.clone(),
                        space_id: repository.space_id.clone(),
                        objective: get("Objective"),
                        followups: 0,
                        queue_position: None,
                    },
                    // New threads start on the coordinator conversation's current model.
                    model: self
                        .chats
                        .iter()
                        .find(|c| {
                            form.zapper
                                .as_ref()
                                .is_some_and(|z| c["id"] == z.coordinator_chat_id)
                        })
                        .and_then(|c| {
                            Some(ZapperModel {
                                harness: c["config"]["harness"].as_str()?.to_owned(),
                                model: c["config"]["model"].as_str().map(str::to_owned),
                            })
                        })
                        .or_else(|| {
                            form.zapper
                                .as_ref()
                                .map(|z| z.config.coordinator_model.clone())
                        })
                        .unwrap_or(ZapperModel {
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
            FormKind::Delete => ZapperAction::Delete,
        };
        self.send_zapper(
            form.id.clone(),
            form.zapper.as_ref().map(|z| z.revision),
            form.home.clone(),
            action,
            cx,
        );
    }
    /// Zapper forms open as a dialog over the workspace instead of replacing it.
    pub(super) fn zapper_form(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::popover::{self, Kind};
        let form = self.zappers.form.as_ref().unwrap();
        let saving = self.zappers.pending.is_some();
        let name = |key: &str| {
            form.inputs
                .iter()
                .find(|(n, _)| *n == key)
                .map(|(_, i)| i.clone())
        };
        let title = match &form.kind {
            FormKind::File(file) if file.version == 0 => "New document".to_owned(),
            FormKind::File(file) => file.path.clone(),
            FormKind::Config if form.zapper.is_none() => "New Zapper".to_owned(),
            FormKind::Config => "Zapper settings".to_owned(),
            FormKind::Worker(Some(_)) => "Message thread".to_owned(),
            FormKind::Worker(None) => "New thread".to_owned(),
            FormKind::Decision(_) => "Answer".to_owned(),
            FormKind::Delete => "Delete Zapper?".to_owned(),
        };
        let close = cx.listener(|this, _, _, cx| {
            this.zappers.form = None;
            cx.notify();
        });
        let mut body = column().px(px(20.)).pb(px(18.)).gap(px(14.));
        match &form.kind {
            FormKind::Config => {
                let zapper_name = form
                    .zapper
                    .as_ref()
                    .map(|z| z.config.name.clone())
                    .unwrap_or_default();
                body = body.child(
                    column()
                        .items_center()
                        .gap(px(10.))
                        .pt(px(4.))
                        .pb(px(6.))
                        .child(match form.zapper.as_ref() {
                            Some(zapper) => crate::zapper_avatar::zapper_face(
                                &crate::zapper_avatar::ZapperFace::of(zapper),
                                56.,
                            ),
                            None => avatar(&zapper_name, 56.),
                        })
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
                    body = body.child(field(
                        "Purpose",
                        "What this Zapper is responsible for",
                        input,
                    ));
                }
                body = body.child(
                    column()
                        .gap(px(6.))
                        .child(caption("Repositories"))
                        .child(self.zapper_repository_list(cx)),
                );
                // Device choice only matters once there is more than one.
                if self.zappers.devices.len() > 1 {
                    body = body.child(
                        column()
                            .rounded(px(10.))
                            .border_1()
                            .border_color(rgb(BORDER))
                            .child(setting_row(
                                "Runs on",
                                "Where the coordinator lives",
                                self.zapper_device_chips(
                                    form.home.clone(),
                                    true,
                                    form.zapper.is_some(),
                                    cx,
                                ),
                                true,
                            ))
                            .child(setting_row(
                                "Threads run on",
                                "Default for new threads",
                                self.zapper_device_chips(
                                    form.worker_device.clone(),
                                    false,
                                    false,
                                    cx,
                                ),
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
                        .child("The coordinator stops and the Zapper is removed. Its threads, documents and repository files are kept."),
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
                if matches!(form.kind, FormKind::Worker(None)) && self.zappers.devices.len() > 1 {
                    body = body.child(column().gap(px(6.)).child(caption("Runs on")).child(
                        self.zapper_device_chips(form.worker_device.clone(), false, false, cx),
                    ));
                }
            }
        }
        let delete = matches!(form.kind, FormKind::Delete);
        let editing = matches!(form.kind, FormKind::Config) && form.zapper.is_some();
        let zapper = form.zapper.clone();
        let mut footer = row()
            .px(px(20.))
            .py(px(14.))
            .gap_2()
            .border_t_1()
            .border_color(rgb(BORDER));
        if editing {
            let z = zapper.clone().unwrap();
            let archive = z.clone();
            footer = footer
                .child(
                    popover::button("zapper-delete", "Delete", Kind::Destructive).on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.open_zapper_form(Some(z.clone()), FormKind::Delete, window, cx)
                        }),
                    ),
                )
                .child(
                    popover::button("zapper-archive", "Archive", Kind::Secondary).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.zapper_action(
                                &archive,
                                ZapperAction::SetStatus {
                                    status: ZapperStatus::Archived,
                                },
                                cx,
                            );
                            this.zappers.form = None;
                        }),
                    ),
                );
        }
        footer = footer
            .child(div().flex_1())
            .child(popover::button("cancel-zapper", "Cancel", Kind::Secondary).on_click(close))
            .child(
                popover::button(
                    "save-zapper",
                    if saving {
                        "Saving…"
                    } else if delete {
                        "Delete Zapper"
                    } else if matches!(form.kind, FormKind::Worker(_)) {
                        "Send"
                    } else if matches!(form.kind, FormKind::Config) && form.zapper.is_none() {
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
                .on_click(cx.listener(|this, _, _, cx| this.save_zapper(cx))),
            );
        let (backdrop, card) = popover::dialog(
            "zapper-form",
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
                        .child(icon_button("close-zapper-form", "close", "Close").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.zappers.form = None;
                                cx.notify();
                            }),
                        )),
                )
                .child(
                    div()
                        .id("zapper-form-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(body),
                )
                .child(footer),
            )
            .into_any_element()
    }
    fn zapper_device_chips(
        &self,
        selected: String,
        home: bool,
        locked: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut chips = row().flex_wrap().justify_end().gap_1();
        for device in &self.zappers.devices {
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
                    SharedString::from(format!("zapper-device-{home}-{id}")),
                    name,
                    active,
                )
                .when(locked && !active, |d| d.opacity(0.4))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = this.zappers.form.as_mut() {
                        if home {
                            if form.zapper.is_none() {
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
    fn zapper_repository_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = &self.zappers.form else {
            return Empty.into_any_element();
        };
        if self.zappers.spaces.is_empty() {
            return muted("Open a project first to give this Zapper a repository.")
                .into_any_element();
        }
        let mut list = column()
            .p(px(4.))
            .gap(px(2.))
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(BORDER));
        for space in &self.zappers.spaces {
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
                    .id(SharedString::from(format!("zapper-repo-{id}")))
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
                        if let Some(form) = this.zappers.form.as_mut() {
                            if form.repositories.iter().any(|r| r.space_id == id) {
                                form.repositories.retain(|r| r.space_id != id);
                            } else {
                                form.repositories.push(ZapperRepository {
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
    fn zapper_thread_archived(&self, chat: &str) -> bool {
        self.chats
            .iter()
            .any(|c| c["id"] == chat && c["archived"] == true)
    }
    fn zapper_thread_status(&self, chat: &str) -> &str {
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
    fn open_zapper_chat(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.iter().find(|c| c["id"] == id).cloned() {
            self.zappers.form = None;
            self.zappers.menu = None;
            self.select(chat, cx);
        }
    }
    fn zapper_tab_bar(&self, zapper: &Zapper, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.zappers.tabs;
        let detail_open = self.zappers.pull_request.is_some()
            || self
                .zappers
                .issues
                .as_ref()
                .is_some_and(|p| p.read(cx).selected_code().is_some());
        let mut strip = row().gap(px(2.));
        for tab in ZapperTab::ALL {
            let active = tab == tabs.active && !detail_open;
            let name = tab.label();
            let color = rgb(if active { TEXT } else { FAINT });
            let base = row()
                .id(SharedString::from(format!("zapper-tab-{name}")))
                .role(Role::Tab)
                .aria_label(name)
                .h(px(26.))
                .px(px(7.))
                .rounded(px(8.))
                .cursor_pointer()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .when(!active, |d| d.hover(|d| d.text_color(rgb(MUTED))))
                .child(if tab == ZapperTab::Issues {
                    crate::zapper_issues::status_icon(ZapperIssueStatus::Todo).into_any_element()
                } else {
                    icon(tab.icon())
                        .size(px(12.))
                        .text_color(color)
                        .into_any_element()
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.zappers.pull_request = None;
                    if let Some(panel) = &this.zappers.issues {
                        panel.update(cx, |p, cx| p.show_list(cx));
                    }
                    if this.zappers.tabs.select(tab) {
                        let history = tab == ZapperTab::History;
                        if this.zappers.history != history {
                            this.zappers.history = history;
                            this.refresh_zapper();
                        }
                        this.zappers.menu = None;
                        cx.notify();
                    }
                }));
            strip = strip.child(
                base.when(
                    matches!(
                        tab,
                        ZapperTab::Overview | ZapperTab::Issues | ZapperTab::Drive
                    ) || active,
                    |d| d.child(div().ml(px(6.)).child(name)),
                )
                .when(active, |d| d.bg(rgb(RAISED))),
            );
        }
        let extra = self
            .zappers
            .pull_request
            .as_ref()
            .map(|u| format!("#{}", u.rsplit('/').next().unwrap_or("")))
            .or_else(|| {
                self.zappers
                    .issues
                    .as_ref()
                    .and_then(|p| p.read(cx).selected_code())
            });
        if let Some(code) =
            extra.filter(|_| matches!(tabs.active, ZapperTab::Issues | ZapperTab::Overview))
        {
            strip = strip.child(
                row()
                    .px(px(8.))
                    .h(px(26.))
                    .gap(px(6.))
                    .rounded(px(7.))
                    .bg(rgb(RAISED))
                    .text_color(rgb(TEXT))
                    .child(if self.zappers.pull_request.is_some() {
                        icon("git-branch")
                            .size(px(12.))
                            .text_color(rgb(0xd9bf60))
                            .into_any_element()
                    } else {
                        crate::zapper_issues::status_icon(
                            self.zappers
                                .issues
                                .as_ref()
                                .and_then(|p| p.read(cx).selected_status())
                                .unwrap_or(ZapperIssueStatus::Todo),
                        )
                        .into_any_element()
                    })
                    .child(code),
            );
        }
        let doc = zapper.clone();
        let settings = zapper.clone();
        row()
            .h(px(44.))
            .flex_shrink_0()
            .px(px(8.))
            .child(strip)
            .child(div().flex_1())
            .when(tabs.active == ZapperTab::Drive, |d| {
                d.child(
                    quiet_icon("zapper-new-document", "document-add", "New document").on_click(
                        cx.listener(move |this, _, window, cx| {
                            let file = ZapperFile {
                                path: String::new(),
                                version: 0,
                                content: String::new(),
                            };
                            this.open_zapper_form(
                                Some(doc.clone()),
                                FormKind::File(file),
                                window,
                                cx,
                            )
                        }),
                    ),
                )
            })
            .when(tabs.active == ZapperTab::Threads, |d| {
                let z = zapper.clone();
                d.child(
                    quiet_icon("new-zapper-worker", "plus", "New thread").on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.open_zapper_form(
                                Some(z.clone()),
                                FormKind::Worker(None),
                                window,
                                cx,
                            )
                        },
                    )),
                )
            })
            .child(
                quiet_icon("zapper-settings", "settings", "Zapper settings").on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.open_zapper_form(Some(settings.clone()), FormKind::Config, window, cx)
                    },
                )),
            )
            .into_any_element()
    }
    /// The panel floats as its own rounded surface beside the conversation.
    pub(super) fn zapper_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(zapper) = self.selected_zapper() else {
            return Empty.into_any_element();
        };
        if self
            .zappers
            .issues
            .as_ref()
            .is_some_and(|p| p.read(cx).zapper_id() != zapper.id)
        {
            self.zappers.pull_request = None;
            self.zappers.document = None;
        }
        if let Some(file) = self.zappers.document.take() {
            self.open_zapper_form(Some(zapper.clone()), FormKind::File(file), window, cx);
        }
        let body = match self.zappers.tabs.active {
            ZapperTab::Issues => self.zapper_issue_surface(&zapper, false, cx),
            ZapperTab::Overview => self.zapper_issue_surface(&zapper, true, cx),
            ZapperTab::Threads => column()
                .gap(px(18.))
                .child(self.zapper_threads(&zapper, cx))
                .child(self.zapper_overview(&zapper, cx))
                .into_any_element(),
            ZapperTab::Drive => self.zapper_drive(&zapper, cx),
            ZapperTab::History => self.zapper_history(&zapper, cx),
        };
        let body = if let Some(url) = self.zappers.pull_request.clone() {
            let mut pr = column().gap(px(12.));
            if let Some(issue) = zapper
                .issues
                .iter()
                .find(|i| i.pull_requests.iter().any(|p| p.url == url))
            {
                let id = issue.id.clone();
                let mut links = row().gap(px(12.)).child(
                    div()
                        .id("pr-linked-issue")
                        .cursor_pointer()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(format!("‹ {} · {}", issue.code(), issue.details.title))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.zappers.pull_request = None;
                            if let Some(panel) = &this.zappers.issues {
                                panel.update(cx, |p, cx| p.show_issue(id.clone(), cx));
                            }
                            cx.notify();
                        })),
                );
                if let Some(chat) = issue.threads.first().cloned() {
                    links = links.child(
                        div()
                            .id("pr-linked-thread")
                            .cursor_pointer()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child("Thread ↗")
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_zapper_chat(&chat, cx)),
                            ),
                    );
                }
                pr = pr.child(links);
            }
            pr.child(self.pull_requests_view(cx)).into_any_element()
        } else {
            body
        };
        crate::workspace_style::column()
            .w(px(ZAPPER_PANEL_WIDTH))
            .flex_none()
            .pr(px(crate::workspace_style::GUTTER))
            .child(
                crate::workspace_style::surface()
                    .bg(rgb(0x101010))
                    .child(self.zapper_tab_bar(&zapper, cx))
                    .child(
                        column()
                            .id("zapper-panel-body")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .px(px(16.))
                            .pt(px(16.))
                            .pb(px(16.))
                            .gap(px(18.))
                            .child(body),
                    ),
            )
            .children(self.zapper_thread_menu(&zapper, cx))
            .into_any_element()
    }
    fn zapper_issue_surface(
        &mut self,
        zapper: &Zapper,
        overview: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::zapper_issues::{IssueEvent, IssuePanel};
        if self.zappers.issues.is_none() {
            let panel = cx.new(|_| IssuePanel::new(zapper.clone(), overview));
            cx.subscribe(&panel, |this, _, event, cx| match event {
                IssueEvent::Command(action, revision) => {
                    if let Some(zapper) = this.selected_zapper() {
                        let mut action = action.clone();
                        if let ZapperAction::Delegate { model, .. } = &mut action {
                            if let Some(config) = this
                                .chats
                                .iter()
                                .find(|c| c["id"] == zapper.coordinator_chat_id)
                                .and_then(|c| c.get("config"))
                            {
                                if let Some(harness) = config["harness"].as_str() {
                                    *model = ZapperModel {
                                        harness: harness.into(),
                                        model: config["model"].as_str().map(str::to_owned),
                                    };
                                }
                            }
                        }
                        this.zappers.issue_command = true;
                        this.send_zapper(
                            zapper.id,
                            Some(*revision),
                            zapper.config.home_device_id,
                            action,
                            cx,
                        );
                    }
                }
                IssueEvent::OpenThread(id) => this.open_zapper_chat(id, cx),
                IssueEvent::OpenDocument(file) => {
                    this.zappers.document = Some(file.clone());
                    cx.notify();
                }
                IssueEvent::OpenPullRequest(url) => {
                    this.zappers.pull_request = Some(url.clone());
                    this.open_pr_url(url.clone(), String::new(), cx);
                }
                // The browser starts threads from the Threads page's own button.
                IssueEvent::NewThread => {
                    if this.zappers.tabs.select(ZapperTab::Threads) {
                        cx.notify();
                    }
                }
                IssueEvent::ShowIssues => {
                    if this.zappers.tabs.select(ZapperTab::Issues) {
                        cx.notify();
                    }
                }
            })
            .detach();
            cx.observe(&panel, |_, _, cx| cx.notify()).detach();
            self.zappers.issues = Some(panel);
        }
        let panel = self.zappers.issues.as_ref().unwrap().clone();
        // Same inputs the native host supplies: each thread's live state and
        // the signed-in person's name for their comments.
        let states: std::collections::HashMap<String, crate::zapper_issues::ThreadState> = zapper
            .workers
            .iter()
            .map(|w| {
                use crate::zapper_issues::ThreadState;
                let state = if self.zapper_thread_archived(&w.chat_id) {
                    ThreadState::Archived
                } else {
                    match self.zapper_thread_status(&w.chat_id) {
                        "working" => ThreadState::Working,
                        "awaitingInput" | "errored" => ThreadState::NeedsYou,
                        _ => ThreadState::Idle,
                    }
                };
                (w.chat_id.clone(), state)
            })
            .collect();
        let user = ["name", "email"].iter().find_map(|key| {
            self.cloud_account["user"][key]
                .as_str()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        });
        panel.update(cx, |p, cx| {
            p.thread_states(states, cx);
            p.user_name(user, cx);
            p.sync(zapper.clone(), overview, self.zappers.pending.is_some(), cx);
            p.files(self.zappers.files.clone(), cx);
            p.thread_titles(
                self.chats
                    .iter()
                    .filter_map(|c| {
                        Some((
                            c["id"].as_str()?.to_owned(),
                            c["title"].as_str()?.to_owned(),
                        ))
                    })
                    .collect(),
                cx,
            );
        });
        panel.into_any_element()
    }
    fn zapper_overview(&self, zapper: &Zapper, cx: &mut Context<Self>) -> AnyElement {
        let working = zapper
            .workers
            .iter()
            .filter(|w| self.zapper_thread_status(&w.chat_id) == "working")
            .count();
        let asks = zuse_protocol::presentation::zapper_panel::needs_you_items(
            &zapper.issues,
            &zapper.decisions,
        );
        let (color, status) = match zapper.status {
            ZapperStatus::Active => (GREEN, "Active"),
            ZapperStatus::Paused => (AMBER, "Paused"),
            ZapperStatus::Stopped => (FAINT, "Stopped"),
            ZapperStatus::Archived => (FAINT, "Archived"),
        };
        let toggle = if zapper.status == ZapperStatus::Active {
            ZapperStatus::Paused
        } else {
            ZapperStatus::Active
        };
        let z = zapper.clone();
        let hero = row()
            .items_start()
            .gap(px(12.))
            .px(px(4.))
            .pt(px(6.))
            .child(crate::zapper_avatar::zapper_face(
                &crate::zapper_avatar::ZapperFace::of(zapper),
                40.,
            ))
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
                            .child(zapper.config.name.clone()),
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
                matches!(zapper.status, ZapperStatus::Active | ZapperStatus::Paused),
                |d| {
                    d.child(
                        crate::popover::button(
                            "zapper-pause",
                            if toggle == ZapperStatus::Paused {
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
                            this.zapper_action(&z, ZapperAction::SetStatus { status: toggle }, cx)
                        })),
                    )
                },
            );
        let stats = row()
            .rounded(px(10.))
            .border_1()
            .border_color(rgb(BORDER))
            .child(stat(
                "Threads",
                zapper
                    .workers
                    .iter()
                    .filter(|w| !self.zapper_thread_archived(&w.chat_id))
                    .count(),
                false,
            ))
            .child(div().w(px(1.)).h(px(34.)).bg(rgb(BORDER)))
            .child(stat("Working", working, false))
            .child(div().w(px(1.)).h(px(34.)).bg(rgb(BORDER)))
            .child(stat("Needs you", asks.len(), !asks.is_empty()));
        let mut view = column()
            .gap(px(16.))
            .child(hero)
            .when(!zapper.config.purpose.is_empty(), |d| {
                d.child(
                    div()
                        .px(px(4.))
                        .text_size(px(12.5))
                        .line_height(px(19.))
                        .text_color(rgb(MUTED))
                        .child(zapper.config.purpose.clone()),
                )
            })
            .child(stats);
        let mut needs = group();
        for item in &asks {
            use zuse_protocol::presentation::zapper_panel::{NeedsYouItem, NeedsYouTarget};
            let (id, glyph) = match &item {
                NeedsYouItem::Decision { id, .. } => (format!("decision-{id}"), "info-circle"),
                NeedsYouItem::Issue { id, .. } => (format!("asked-{id}"), "info-circle"),
            };
            let z = zapper.clone();
            let decisions = zapper.decisions.clone();
            let target = item.target();
            needs = needs.child(
                list_row(
                    SharedString::from(id),
                    glyph,
                    AMBER,
                    item.title(),
                    Some("Answer"),
                )
                .on_click(cx.listener(move |this, _, window, cx| match &target {
                    NeedsYouTarget::DecisionForm(id) => {
                        if let Some(decision) = decisions.iter().find(|d| &d.id == id) {
                            this.open_zapper_form(
                                Some(z.clone()),
                                FormKind::Decision(decision.clone()),
                                window,
                                cx,
                            )
                        }
                    }
                    NeedsYouTarget::IssueTab(id) => {
                        this.zappers.tabs.select(ZapperTab::Issues);
                        this.zappers.pull_request = None;
                        if let Some(panel) = &this.zappers.issues {
                            panel.update(cx, |p, cx| p.show_issue(id.clone(), cx));
                        }
                        cx.notify();
                    }
                })),
            );
        }
        view = view.child(column().gap(px(6.)).child(section("Needs you")).child(
            if asks.is_empty() {
                empty_state(
                    "check-circle",
                    "All clear",
                    "Questions and decisions waiting on you show up here.",
                )
            } else {
                needs.into_any_element()
            },
        ));
        let current = zapper.coordinator_chat_id.clone();
        view.child(
            column().gap(px(6.)).child(section("Coordinator")).child(
                group()
                    .child(
                        list_row(
                            "zapper-current-conversation",
                            "chat-round-line",
                            MUTED,
                            "Current conversation".into(),
                            None,
                        )
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_zapper_chat(&current, cx)),
                        ),
                    )
                    .children(
                        zapper
                            .previous_coordinator_chats
                            .iter()
                            .enumerate()
                            .rev()
                            .map(|(index, id)| {
                                let id = id.clone();
                                list_row(
                                    SharedString::from(format!("zapper-old-{id}")),
                                    "history",
                                    FAINT,
                                    format!("Earlier conversation {}", index + 1),
                                    None,
                                )
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        this.open_zapper_chat(&id, cx)
                                    }),
                                )
                            }),
                    )
                    .child({
                        let z = zapper.clone();
                        list_row(
                            "fresh-zapper",
                            "restart",
                            FAINT,
                            "Start a fresh conversation".into(),
                            None,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.zapper_action(&z, ZapperAction::FreshConversation, cx)
                        }))
                    }),
            ),
        )
        .into_any_element()
    }
    fn zapper_threads(&self, zapper: &Zapper, cx: &mut Context<Self>) -> AnyElement {
        let mut threads = group();
        let workers: Vec<_> = zapper
            .workers
            .iter()
            .filter(|w| !self.zapper_thread_archived(&w.chat_id))
            .collect();
        let empty = workers.is_empty();
        let count = workers.len();
        for worker in workers {
            let id = worker.chat_id.clone();
            let (dot, status) = if worker.queued() {
                (Hsla::from(rgb(FAINT)).opacity(0.5), "Queued")
            } else {
                thread_state(self.zapper_thread_status(&id))
            };
            let title = self
                .chats
                .iter()
                .find(|c| c["id"] == id)
                .and_then(|c| c["title"].as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| worker.objective.clone());
            let open = id.clone();
            let menu = id.clone();
            let open_menu = self.zappers.menu.as_ref().is_some_and(|(m, _)| m == &id);
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
                                this.zappers.menu = if this
                                    .zappers
                                    .menu
                                    .as_ref()
                                    .is_some_and(|(m, _)| m == &menu)
                                {
                                    None
                                } else {
                                    Some((menu.clone(), window.mouse_position()))
                                };
                                cx.notify();
                            },
                        )),
                    )
                    .relative()
                    .on_click(cx.listener(move |this, _, _, cx| this.open_zapper_chat(&open, cx))),
            );
        }
        let mut view = column().gap(px(16.)).child(
            column()
                .gap(px(6.))
                .child(section(format!("Threads · {count}")))
                .child(if empty {
                    empty_state(
                        "chat-round-line",
                        "No threads yet",
                        "Threads this Zapper starts or adopts appear here.",
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
                    && !zapper_owns_chat(&self.zappers.items, c["id"].as_str().unwrap_or_default())
                    && zapper
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
                let z = zapper.clone();
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
                        this.zapper_action(
                            &z,
                            ZapperAction::AdoptWorker {
                                worker: ZapperWorker {
                                    chat_id: chat["id"].as_str().unwrap_or_default().into(),
                                    device_id: chat["deviceId"].as_str().unwrap_or_default().into(),
                                    space_id: chat["spaceId"].as_str().unwrap_or_default().into(),
                                    objective: title.clone(),
                                    followups: 0,
                                    queue_position: None,
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
    fn zapper_thread_menu(&self, zapper: &Zapper, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::popover;
        let (id, at) = self.zappers.menu.clone()?;
        zapper
            .workers
            .iter()
            .any(|w| w.chat_id == id)
            .then_some(())?;
        let item = |key: &'static str,
                    glyph,
                    label,
                    destructive,
                    action: Option<fn(String) -> ZapperAction>| {
            let id = id.clone();
            let z = zapper.clone();
            popover::item(
                SharedString::from(format!("zapper-thread-{key}")),
                glyph,
                label,
                destructive,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.zappers.menu = None;
                match action {
                    Some(action) => this.zapper_action(&z, action(id.clone()), cx),
                    None => this.open_zapper_form(
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
        let menu = popover::menu("zapper-thread-menu", 200.)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.zappers.menu = None;
                cx.notify();
            }))
            .child(
                popover::item("zapper-thread-open", "arrow-up-right", "Open thread", false)
                    .on_click(cx.listener(move |this, _, _, cx| this.open_zapper_chat(&open, cx))),
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
                Some(|chat_id| ZapperAction::StopWorker { chat_id }),
            ))
            .child(item(
                "release",
                "arrow-up-right",
                "Release from Zapper",
                false,
                Some(|chat_id| ZapperAction::ReleaseWorker { chat_id }),
            ))
            .child(item(
                "archive",
                "archive-minimalistic",
                "Archive",
                true,
                Some(|chat_id| ZapperAction::ArchiveWorker { chat_id }),
            ));
        Some(popover::anchored_at(at - point(px(180.), px(-10.)), menu))
    }
    fn zapper_drive(&self, zapper: &Zapper, cx: &mut Context<Self>) -> AnyElement {
        let files = if self.zappers.history {
            vec![]
        } else {
            self.zappers.files.clone()
        };
        let mut list = group();
        for file in files.iter().cloned() {
            let z = zapper.clone();
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
                        this.open_zapper_form(
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
                    "Rules, notes and the digest this Zapper keeps.",
                )
            } else {
                list.into_any_element()
            })
            .into_any_element()
    }
    fn zapper_history(&self, zapper: &Zapper, cx: &mut Context<Self>) -> AnyElement {
        let mut files = if self.zappers.history {
            self.zappers.files.clone()
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
                let z = zapper.clone();
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
                                        this.zapper_action(
                                            &z,
                                            ZapperAction::RestoreFile {
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
/// The Zapper's mark: its initial on a soft tile.
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
        .child(icon(glyph).size(px(12.)))
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
