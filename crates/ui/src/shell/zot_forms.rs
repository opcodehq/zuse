//! Zot forms. New Zot and Zot settings are routed pages; documents, decisions
//! and threads float over the conversation they came from.
use super::zots::{zot_avatar, zot_mascot};
use super::*;
use crate::settings::widgets::{self, ActionTone};
use zuse_protocol::presentation::zot_mascots;
use zuse_protocol::zots::*;

pub(super) enum Kind {
    Config,
    File(ZotFile),
    Thread(Option<String>),
    Decision(ZotDecision),
}

pub(super) struct Form {
    pub(super) id: String,
    zot: Option<Zot>,
    kind: Kind,
    inputs: Vec<Entity<ComposerInput>>,
    repositories: Vec<ZotRepository>,
    /// The project a new thread starts in, when the Zot has several.
    space: Option<String>,
    home: String,
    worker_device: String,
    avatar: ZotAvatar,
    /// Projects that existed when "Add" was pressed; a new one is switched on.
    known_spaces: Option<Vec<String>>,
    focus_pending: bool,
    _subscriptions: Vec<Subscription>,
}

impl Form {
    fn target(&self) -> Option<String> {
        self.zot.as_ref().map(|z| z.id.clone())
    }
}

/// What a successful command does to the surface that sent it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ZotAfter {
    Stay,
    CloseDialog,
    /// The page's history entry becomes the new coordinator, so Back skips
    /// the finished form.
    Created,
    LeavePage,
    OpenCoordinator,
}

#[derive(Clone, Copy)]
enum Lines {
    Single,
    /// Long text where Enter starts a new line.
    Prose,
    /// A short message where Enter sends.
    Message,
}

fn fields(kind: &Kind, zot: Option<&Zot>) -> Vec<(&'static str, String, Lines)> {
    match kind {
        Kind::Config => vec![
            (
                "Name",
                zot.map(|z| z.config.name.clone()).unwrap_or_default(),
                Lines::Single,
            ),
            (
                "What this Zapper is responsible for",
                zot.map(|z| z.config.purpose.clone()).unwrap_or_default(),
                Lines::Prose,
            ),
        ],
        Kind::File(file) if file.version == 0 => vec![
            ("Name", String::new(), Lines::Single),
            ("Content", String::new(), Lines::Prose),
        ],
        Kind::File(file) => vec![("Content", file.content.clone(), Lines::Prose)],
        Kind::Decision(_) => vec![("Your answer", String::new(), Lines::Message)],
        Kind::Thread(Some(_)) => vec![("Message", String::new(), Lines::Message)],
        Kind::Thread(None) => vec![
            ("What should this thread do?", String::new(), Lines::Message),
            ("HEAD", String::new(), Lines::Single),
        ],
    }
}

/// A text box whose whole frame focuses its input, not just the first line.
fn text_box(input: Entity<ComposerInput>, min_height: f32, cx: &App) -> gpui::Div {
    let focus = input.focus_handle(cx);
    popover::dialog_field(input.into_any_element())
        .min_h(px(min_height))
        .cursor_text()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            let focus = focus.clone();
            // The input blurs on presses outside its own line box.
            window.defer(cx, move |window, cx| window.focus(&focus, cx));
        })
}

fn labelled(
    theme: &Theme,
    label: &'static str,
    input: Entity<ComposerInput>,
    min_height: f32,
    cx: &App,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(widgets::field_label(theme, label))
        .child(text_box(input, min_height, cx))
}

fn chip(
    id: impl Into<SharedString>,
    label: String,
    active: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.into())
        .role(gpui::Role::RadioButton)
        .aria_label(SharedString::from(label.clone()))
        .flex()
        .items_center()
        .gap(px(6.0))
        .h(px(26.0))
        .px(px(10.0))
        .rounded_full()
        .border_1()
        .cursor_pointer()
        .text_size(crate::typography::ui_rems(12.0))
        .when(active, |d| {
            d.border_color(theme.border_strong)
                .bg(theme.element_active)
                .text_color(theme.text_strong)
                .child(icon(icons::CHECK).size(px(11.0)))
        })
        .when(!active, |d| {
            d.border_color(theme.border)
                .text_color(theme.text_muted)
                .hover(|d| d.text_color(theme.text))
        })
        .child(label)
}

impl Shell {
    /// On the New Zot page.
    pub(super) fn creating_zot(&self) -> bool {
        matches!(self.route, Route::Zot)
    }

    /// Whether the right panel is editing the selected Zot.
    pub(super) fn zot_editor_open(&self, cx: &App) -> bool {
        matches!(self.route, Route::Chat)
            && self.selected_zot(cx).is_some_and(|zot| {
                self.zot_ui
                    .page
                    .as_ref()
                    .and_then(|page| page.target())
                    .is_some_and(|target| target == zot.id)
            })
    }

    pub(super) fn open_zot_editor(&mut self, zot: Zot, cx: &mut Context<Self>) {
        self.zot_ui.page = Some(self.build_zot_form(Some(zot), Kind::Config, false, cx));
        cx.notify();
    }

    fn build_zot_form(
        &mut self,
        zot: Option<Zot>,
        kind: Kind,
        dialog: bool,
        cx: &mut Context<Self>,
    ) -> Form {
        let state = self.state.read(cx);
        let home = zot
            .as_ref()
            .map(|z| z.config.home_device_id.clone())
            .or_else(|| state.local_device_id.clone())
            .unwrap_or_default();
        let repositories = zot
            .as_ref()
            .map(|z| z.config.repositories.clone())
            .unwrap_or_else(|| {
                state
                    .selected_space_row()
                    .map(|s| {
                        vec![ZotRepository {
                            device_id: s.device_id.clone(),
                            space_id: s.id.clone(),
                        }]
                    })
                    .unwrap_or_default()
            });
        let worker_device = zot
            .as_ref()
            .map(|z| z.config.worker_device_id.clone())
            .unwrap_or_else(|| home.clone());
        let id = zot
            .as_ref()
            .map(|z| z.id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let avatar = zot
            .as_ref()
            .map(zot_avatar)
            .unwrap_or_else(|| zot_mascots::default_avatar(&id));
        let mut subscriptions = vec![];
        let inputs = fields(&kind, zot.as_ref())
            .into_iter()
            .map(|(placeholder, value, lines)| {
                let input = cx.new(|cx| {
                    match lines {
                        Lines::Single => {
                            ComposerInput::with_context(placeholder, "ProjectForm", cx)
                                .with_single_line()
                        }
                        Lines::Prose => ComposerInput::new(placeholder, cx).with_enter_newline(),
                        Lines::Message => ComposerInput::new(placeholder, cx),
                    }
                    .with_tab_stop()
                });
                input.update(cx, |input, cx| input.set_text(value, cx));
                subscriptions.push(cx.subscribe(
                    &input,
                    move |this: &mut Shell, _, event: &ComposerInputEvent, cx| match event {
                        ComposerInputEvent::Submitted => this.submit_zot_form(dialog, cx),
                        ComposerInputEvent::Edited => cx.notify(),
                        _ => {}
                    },
                ));
                input
            })
            .collect();
        Form {
            id,
            zot,
            kind,
            inputs,
            repositories,
            space: None,
            home,
            worker_device,
            avatar,
            known_spaces: None,
            focus_pending: true,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn open_new_zot(&mut self, cx: &mut Context<Self>) {
        self.nav.push(NavEntry::NewZot);
        self.show_new_zot(cx);
    }

    /// Land on New Zot, keeping a draft the user walked away from.
    pub(super) fn show_new_zot(&mut self, cx: &mut Context<Self>) {
        match self.zot_ui.page.as_mut() {
            Some(page) if page.target().is_none() => page.focus_pending = true,
            _ => self.zot_ui.page = Some(self.build_zot_form(None, Kind::Config, false, cx)),
        }
        self.zot_ui.dialog = None;
        self.set_voice_stage_open(false, cx);
        self.set_route(Route::Zot, cx);
        cx.notify();
    }

    /// Discard the page and return to wherever it was opened from.
    pub(super) fn leave_zot_page(&mut self, cx: &mut Context<Self>) {
        self.zot_ui.page = None;
        if !matches!(self.route, Route::Zot) {
            return;
        }
        if self.nav.can_back() {
            self.navigate_back(cx);
        } else {
            self.apply_nav(NavEntry::Chat(String::new()), cx);
        }
    }

    pub(super) fn open_zot_dialog(&mut self, zot: Zot, kind: Kind, cx: &mut Context<Self>) {
        self.zot_ui.dialog = Some(self.build_zot_form(Some(zot), kind, true, cx));
        cx.notify();
    }

    fn zot_default_model(&self, cx: &App) -> ZotModel {
        let chosen = crate::settings::default_model(cx).map(|preferred| ZotModel {
            harness: String::from(preferred.provider.clone()),
            model: Some(preferred.model.clone()),
        });
        chosen
            .or_else(|| {
                let config = self.state.read(cx).selected_chat_row()?.config.clone()?;
                Some(ZotModel {
                    harness: String::from(config.harness),
                    model: config.model,
                })
            })
            .unwrap_or_else(|| ZotModel {
                harness: "claude-code".into(),
                model: None,
            })
    }

    pub(super) fn zot_thread_model(&self, zot: &Zot, cx: &App) -> ZotModel {
        self.state
            .read(cx)
            .chats
            .iter()
            .find(|c| c.id == zot.coordinator_chat_id)
            .and_then(|c| c.config.clone())
            .map(|config| ZotModel {
                harness: String::from(config.harness),
                model: config.model,
            })
            .unwrap_or_else(|| zot.config.coordinator_model.clone())
    }

    pub(super) fn submit_zot_form(&mut self, dialog: bool, cx: &mut Context<Self>) {
        let form = if dialog {
            self.zot_ui.dialog.as_ref()
        } else {
            self.zot_ui.page.as_ref()
        };
        let Some(form) = form else {
            return;
        };
        let values: Vec<String> = form
            .inputs
            .iter()
            .map(|i| i.read(cx).text().to_owned())
            .collect();
        let error = |this: &mut Self, text: &'static str, cx: &mut Context<Self>| {
            this.show_notice(crate::toast::ToastKind::Error, text, cx)
        };
        // Revisions also move for decisions and worker activity, so a form that
        // has been open a while still writes against the current one.
        let latest = form.zot.as_ref().and_then(|zot| {
            self.state
                .read(cx)
                .zots
                .iter()
                .find(|z| z.id == zot.id)
                .cloned()
        });
        if form.zot.is_some() && latest.is_none() {
            error(self, "This Zapper no longer exists", cx);
            return;
        }
        let revision = latest.as_ref().map(|z| z.revision);
        let mut after = if dialog {
            ZotAfter::CloseDialog
        } else {
            ZotAfter::LeavePage
        };
        let action = match &form.kind {
            Kind::Config => {
                let name = values[0].trim().to_owned();
                if name.is_empty() {
                    error(self, "Give this Zapper a name", cx);
                    return;
                }
                let (home_device_id, repositories, worker_device_id) = (
                    form.home.clone(),
                    form.repositories.clone(),
                    form.worker_device.clone(),
                );
                let purpose = values[1].clone();
                let avatar = Some(form.avatar.clone());
                match latest {
                    Some(zot) => ZotAction::Configure {
                        config: ZotConfig {
                            name,
                            purpose,
                            home_device_id,
                            repositories,
                            worker_device_id,
                            avatar,
                            ..zot.config
                        },
                    },
                    None => {
                        after = ZotAfter::Created;
                        ZotAction::Create {
                            config: ZotConfig {
                                name,
                                purpose,
                                home_device_id,
                                repositories,
                                // The coordinator's composer changes this later.
                                coordinator_model: self.zot_default_model(cx),
                                worker_device_id,
                                max_concurrent_workers: 4,
                                max_followups: 3,
                                avatar,
                            },
                        }
                    }
                }
            }
            Kind::File(file) => {
                let (path, content) = if file.version == 0 {
                    (values[0].trim().to_owned(), values[1].clone())
                } else {
                    (file.path.clone(), values[0].clone())
                };
                if path.is_empty() {
                    error(self, "Name the document", cx);
                    return;
                }
                ZotAction::WriteFile {
                    path,
                    content,
                    expected_version: file.version,
                }
            }
            Kind::Decision(decision) => {
                let answer = values[0].trim().to_owned();
                if answer.is_empty() {
                    error(self, "Write an answer first", cx);
                    return;
                }
                let mut decision = decision.clone();
                decision.answer = Some(answer);
                ZotAction::Decide { decision }
            }
            Kind::Thread(Some(chat_id)) => {
                let text = values[0].trim().to_owned();
                if text.is_empty() {
                    error(self, "Write a message first", cx);
                    return;
                }
                ZotAction::MessageWorker {
                    chat_id: chat_id.clone(),
                    text,
                    automatic: false,
                }
            }
            Kind::Thread(None) => {
                let objective = values[0].trim().to_owned();
                if objective.is_empty() {
                    error(self, "Describe what the thread should do", cx);
                    return;
                }
                let repository = form
                    .repositories
                    .iter()
                    .find(|r| Some(&r.space_id) == form.space.as_ref())
                    .or_else(|| {
                        form.repositories
                            .iter()
                            .find(|r| r.device_id == form.worker_device)
                    })
                    .or(form.repositories.first());
                let Some(repository) = repository else {
                    error(self, "Add a repository to this Zapper first", cx);
                    return;
                };
                let zot = latest.as_ref().or(form.zot.as_ref()).unwrap();
                let base = values[1].trim();
                ZotAction::Delegate {
                    issue_id: None,
                    worker: ZotWorker {
                        chat_id: String::new(),
                        device_id: repository.device_id.clone(),
                        space_id: repository.space_id.clone(),
                        objective,
                        followups: 0,
                    },
                    model: self.zot_thread_model(zot, cx),
                    base: if base.is_empty() {
                        "HEAD".into()
                    } else {
                        base.to_owned()
                    },
                }
            }
        };
        let id = form.id.clone();
        self.send_zot_command(id, revision, action, after, cx);
    }

    pub(super) fn send_zot_command(
        &mut self,
        id: String,
        expected_revision: Option<u32>,
        action: ZotAction,
        after: ZotAfter,
        cx: &mut Context<Self>,
    ) {
        let from_issue_panel = std::mem::take(&mut self.zot_ui.issue_command);
        if self.zot_ui.pending {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let home = self
            .state
            .read(cx)
            .zots
            .iter()
            .find(|z| z.id == id)
            .map(|z| z.config.home_device_id.clone())
            .or_else(|| {
                self.zot_ui
                    .page
                    .as_ref()
                    .filter(|page| page.id == id)
                    .map(|page| page.home.clone())
            });
        let mut command = ZotCommand {
            command_id: uuid::Uuid::new_v4().to_string(),
            zot_id: id,
            expected_revision,
            action,
        };
        if let ZotAction::Delegate { worker, .. } = &mut command.action {
            worker.chat_id = format!("zot-worker-{}", command.command_id);
        }
        if let Some(previous) = &self.zot_ui.retry {
            let mut comparable = command.clone();
            comparable.command_id = previous.command_id.clone();
            comparable.expected_revision = previous.expected_revision;
            if let ZotAction::Delegate { worker, .. } = &mut comparable.action {
                worker.chat_id = format!("zot-worker-{}", previous.command_id);
            }
            if serde_json::to_value(&comparable).ok() == serde_json::to_value(previous).ok() {
                command = previous.clone();
            }
        }
        self.zot_ui.retry = Some(command.clone());
        let mut params = serde_json::to_value(command).unwrap();
        if let Some(home) = home {
            params["targetDeviceId"] = serde_json::json!(home);
        }
        self.zot_ui.pending = true;
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(zuse_protocol::control::ZOT_COMMAND, params)
                .await;
            let _ = this.update(cx, |this, cx| {
                this.zot_ui.pending = false;
                match result {
                    Ok(value) => {
                        this.zot_ui.retry = None;
                        if let Some(panel) =
                            this.zot_ui.issues.as_ref().filter(|_| from_issue_panel)
                        {
                            panel.update(cx, |p, cx| p.committed(cx));
                        }
                        this.zot_ui.loaded = None;
                        let coordinator = serde_json::from_value::<Option<Zot>>(value)
                            .ok()
                            .flatten()
                            .map(|zot| zot.coordinator_chat_id);
                        this.finish_zot_command(after, coordinator, cx);
                    }
                    Err(error) => {
                        // The engine answered: nothing was written, so the
                        // next attempt is a new command at the current revision.
                        if !zuse_protocol::zots::command_outcome_unknown(&error.to_string()) {
                            this.zot_ui.retry = None;
                        }
                        if let Some(panel) =
                            this.zot_ui.issues.as_ref().filter(|_| from_issue_panel)
                        {
                            panel.update(cx, |p, cx| p.failed(error.to_string(), cx));
                        }
                        this.show_notice(crate::toast::ToastKind::Error, error.to_string(), cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_zot_command(
        &mut self,
        after: ZotAfter,
        coordinator: Option<String>,
        cx: &mut Context<Self>,
    ) {
        match after {
            ZotAfter::Stay => {}
            ZotAfter::CloseDialog => self.zot_ui.dialog = None,
            ZotAfter::LeavePage => self.leave_zot_page(cx),
            ZotAfter::Created => {
                self.zot_ui.page = None;
                if let Some(chat) = coordinator.filter(|_| matches!(self.route, Route::Zot)) {
                    let entry = NavEntry::Chat(chat);
                    self.nav.replace(entry.clone());
                    self.apply_nav(entry, cx);
                }
            }
            ZotAfter::OpenCoordinator => {
                if let Some(chat) = coordinator {
                    self.open_chat(chat, cx);
                }
            }
        }
    }

    fn zot_avatar_picker(
        &self,
        current: &ZotAvatar,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let set = |avatar: ZotAvatar| {
            cx.listener(move |this: &mut Shell, _: &gpui::ClickEvent, _, cx| {
                if let Some(form) = this.zot_ui.page.as_mut() {
                    form.avatar = avatar.clone();
                }
                cx.notify();
            })
        };
        let mut mascots = div().flex().flex_wrap().gap(px(6.0));
        for mascot in zot_mascots::MASCOTS {
            let avatar = ZotAvatar {
                mascot: mascot.id.into(),
                color: current.color.clone(),
            };
            let active = mascot.id == current.mascot;
            mascots = mascots.child(
                div()
                    .id(SharedString::from(format!("zot-mascot-{}", mascot.id)))
                    .role(gpui::Role::RadioButton)
                    .aria_label(mascot.name)
                    .tooltip(widgets::text_tooltip(mascot.name))
                    .size(px(46.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(9.0))
                    .border_1()
                    .cursor_pointer()
                    .when(active, |d| {
                        d.border_color(theme.accent).bg(theme.element_active)
                    })
                    .when(!active, |d| {
                        d.border_color(theme.border)
                            .hover(|d| d.bg(theme.element_hover).border_color(theme.border_strong))
                    })
                    .child(zot_mascot(&avatar, 2.0))
                    .on_click(set(avatar.clone())),
            );
        }
        let mut colors = div().flex().items_center().gap(px(6.0));
        for color in zot_mascots::COLORS {
            let active = color.id == current.color;
            colors = colors.child(
                div()
                    .id(SharedString::from(format!("zot-color-{}", color.id)))
                    .role(gpui::Role::RadioButton)
                    .aria_label(color.id)
                    .size(px(22.0))
                    .p(px(3.0))
                    .rounded(px(5.0))
                    .border_1()
                    .cursor_pointer()
                    .border_color(if active {
                        theme.accent
                    } else {
                        gpui::transparent_black()
                    })
                    .child(div().size_full().rounded(px(2.0)).bg(gpui::rgb(color.body)))
                    .on_click(set(ZotAvatar {
                        mascot: current.mascot.clone(),
                        color: color.id.into(),
                    })),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(mascots)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(colors)
                    .child(
                        widgets::ghost_action(theme)
                            .id("zot-shuffle")
                            .role(gpui::Role::Button)
                            .child(icon(icons::REFRESH).size(px(13.0)))
                            .child("Shuffle")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(form) = this.zot_ui.page.as_mut() {
                                    form.avatar = zot_mascots::default_avatar(
                                        &uuid::Uuid::new_v4().to_string(),
                                    );
                                }
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn zot_repository_card(
        &self,
        selected: &[ZotRepository],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (spaces, devices, local) = {
            let state = self.state.read(cx);
            (
                state.spaces.clone(),
                state.devices.clone(),
                state.local_device_id.clone(),
            )
        };
        if spaces.is_empty() {
            return widgets::section_card(theme)
                .mt(px(0.0))
                .p(px(16.0))
                .text_size(crate::typography::text_sm())
                .text_color(theme.text_muted)
                .child("No projects yet. Add one from GitHub or a folder on this device.")
                .into_any_element();
        }
        let mut card = widgets::section_card(theme).mt(px(0.0));
        for (index, space) in spaces.into_iter().enumerate() {
            let checked = selected.iter().any(|r| r.space_id == space.id);
            let id = space.id.clone();
            let device = space.device_id.clone();
            let remote = devices
                .iter()
                .find(|d| d.id == device && Some(&d.id) != local.as_ref())
                .map(|d| d.name.clone());
            let name = space.name.clone().unwrap_or_else(|| {
                space
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&space.path)
                    .to_owned()
            });
            card = card.child(
                div()
                    .id(SharedString::from(format!("zot-repo-{id}")))
                    .role(gpui::Role::CheckBox)
                    .aria_label(SharedString::from(name.clone()))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(12.0))
                    .py(px(8.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(theme.element_hover))
                    .when(index > 0, |d| {
                        d.border_t_1().border_color(widgets::row_divider(theme))
                    })
                    .child(
                        icon(icons::FOLDER)
                            .size(px(15.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(crate::typography::text_sm())
                                    .text_color(theme.text_strong)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(11.5))
                                    .text_color(theme.text_faint)
                                    .child(match remote {
                                        Some(device) => format!("{device} · {}", space.path),
                                        None => space.path.clone(),
                                    }),
                            ),
                    )
                    .child(widgets::toggle_switch(
                        theme,
                        checked,
                        format!("zot-repo-{id}"),
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(form) = this.zot_ui.page.as_mut() {
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
        card.into_any_element()
    }

    fn zot_device_chips(
        &self,
        selected: &str,
        home: bool,
        locked: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let devices = self.state.read(cx).devices.clone();
        let mut chips = div().flex().flex_wrap().justify_end().gap(px(4.0));
        for device in devices {
            let active = selected == device.id;
            let id = device.id.clone();
            chips = chips.child(
                chip(
                    format!("zot-device-{home}-{id}"),
                    device.name,
                    active,
                    theme,
                )
                .when(locked && !active, |d| d.opacity(0.4))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = this.zot_ui.page.as_mut() {
                        if !home {
                            form.worker_device = id.clone();
                        } else if form.zot.is_none() {
                            form.home = id.clone();
                        }
                    }
                    cx.notify();
                })),
            );
        }
        chips.into_any_element()
    }

    /// The fields shared by the New Zot page and the panel editor.
    fn zot_form_fields(
        &mut self,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let theme = Theme::of(cx).clone();
        let spaces = self.state.read(cx).spaces.clone();
        let form = self.zot_ui.page.as_mut().unwrap();
        if std::mem::take(&mut form.focus_pending) {
            window.focus(&form.inputs[0].focus_handle(cx), cx);
        }
        if let Some(known) = &form.known_spaces {
            let added: Vec<_> = spaces.iter().filter(|s| !known.contains(&s.id)).collect();
            if !added.is_empty() {
                form.repositories
                    .extend(added.into_iter().map(|space| ZotRepository {
                        space_id: space.id.clone(),
                        device_id: space.device_id.clone(),
                    }));
                form.known_spaces = None;
            }
        }
        let avatar = form.avatar.clone();
        let name = form.inputs[0].clone();
        let purpose = form.inputs[1].clone();
        let editing = form.zot.is_some();
        let selected = form.repositories.clone();
        let (home, worker_device) = (form.home.clone(), form.worker_device.clone());
        let multiple_devices = self.state.read(cx).devices.len() > 1;
        let identity = div()
            .flex()
            .items_center()
            .gap(px(14.0))
            .child(
                div()
                    .size(px(if compact { 60.0 } else { 80.0 }))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(14.0))
                    .bg(theme.element_hover)
                    .child(zot_mascot(&avatar, if compact { 3.0 } else { 4.0 })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(labelled(&theme, "Name", name, 36.0, cx)),
            );
        let character = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(widgets::field_label(&theme, "Character"))
            .child(self.zot_avatar_picker(&avatar, &theme, cx));
        let repositories = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(widgets::field_label(&theme, "Repositories"))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.5))
                                    .text_color(theme.text_faint)
                                    .child("Threads this Zapper starts can work in the ones you switch on."),
                            ),
                    )
                    .child(
                        widgets::action_button(&theme, ActionTone::Outlined)
                            .id("zot-add-project")
                            .role(gpui::Role::Button)
                            .child(icon(icons::PLUS).size(px(12.0)))
                            .child(if compact { "Add" } else { "Add from GitHub or folder" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                let known = this
                                    .state
                                    .read(cx)
                                    .spaces
                                    .iter()
                                    .map(|s| s.id.clone())
                                    .collect();
                                if let Some(form) = this.zot_ui.page.as_mut() {
                                    form.known_spaces = Some(known);
                                }
                                this.open_project_setup(None, cx);
                            })),
                    ),
            )
            .child(self.zot_repository_card(&selected, &theme, cx));
        let environment = multiple_devices.then(|| {
            let row = |title: &'static str, detail: &'static str, control: AnyElement, first| {
                widgets::card_row(&theme, first)
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(widgets::row_title(&theme, title))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.5))
                                    .text_color(theme.text_faint)
                                    .child(detail),
                            ),
                    )
                    .child(control)
            };
            let host = self.zot_device_chips(&home, true, editing, &theme, cx);
            let workers = self.zot_device_chips(&worker_device, false, false, &theme, cx);
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(widgets::field_label(&theme, "Devices"))
                .child(
                    widgets::section_card(&theme)
                        .mt(px(0.0))
                        .child(row("Runs on", "Where the coordinator lives", host, true))
                        .child(row(
                            "Threads run on",
                            "Default for new threads",
                            workers,
                            false,
                        )),
                )
        });
        div()
            .flex()
            .flex_col()
            .gap(px(if compact { 18.0 } else { 24.0 }))
            .child(identity)
            .child(character)
            .child(labelled(&theme, "Purpose", purpose, 96.0, cx))
            .child(repositories)
            .children(environment)
    }

    pub(super) fn render_zot_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.zot_ui.page.is_none() {
            self.zot_ui.page = Some(self.build_zot_form(None, Kind::Config, false, cx));
        }
        let theme = Theme::of(cx).clone();
        let saving = self.zot_ui.pending;
        let column = || widgets::page_column().max_w(px(640.0));
        let header = div()
            .flex()
            .flex_col()
            .child(widgets::page_header(&theme, "New Zapper", None))
            .child(widgets::page_subtitle(
                &theme,
                "A Zapper is your captain: it runs work across your repositories, asks when it needs a decision and keeps its notes between conversations.",
            ));
        let fields = self.zot_form_fields(false, window, cx);
        let submit = if saving {
            "Creating…"
        } else {
            "Create Zapper"
        };
        div()
            .id("zot-page")
            .debug_selector(|| "zot-page".into())
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .child(
                div()
                    .id("zot-page-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        column()
                            .pt(px(16.0))
                            .pb(px(32.0))
                            .gap(px(24.0))
                            .child(header)
                            .child(fields),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        column()
                            .pt(px(12.0))
                            .pb(px(12.0))
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                widgets::text_action(&theme, ActionTone::Quiet, "Cancel")
                                    .id("zot-cancel")
                                    .role(gpui::Role::Button)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.leave_zot_page(cx)),
                                    ),
                            )
                            .child(
                                popover::btn_primary(&theme, submit)
                                    .id("zot-save")
                                    .role(gpui::Role::Button)
                                    .when(saving, |d| d.opacity(0.6))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.submit_zot_form(false, cx)
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Editing lives in the right panel, beside the conversation it configures.
    pub(super) fn render_zot_editor(
        &mut self,
        zot: &Zot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let saving = self.zot_ui.pending;
        let fields = self.zot_form_fields(true, window, cx);
        let manage = self.zot_manage(zot, &theme, cx);
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("zot-editor-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(18.0))
                    .py(px(16.0))
                    .flex()
                    .flex_col()
                    .gap(px(22.0))
                    .child(fields)
                    .child(manage),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .p(px(10.0))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        widgets::text_action(&theme, ActionTone::Quiet, "Cancel")
                            .id("zot-editor-cancel")
                            .role(gpui::Role::Button)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zot_ui.page = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_primary(
                            &theme,
                            if saving { "Saving…" } else { "Save changes" },
                        )
                        .id("zot-editor-save")
                        .role(gpui::Role::Button)
                        .when(saving, |d| d.opacity(0.6))
                        .on_click(cx.listener(|this, _, _, cx| this.submit_zot_form(false, cx))),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_zot_dialog(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let saving = self.zot_ui.pending;
        let form = self.zot_ui.dialog.as_mut()?;
        if std::mem::take(&mut form.focus_pending) {
            window.focus(&form.inputs[0].focus_handle(cx), cx);
        }
        let inputs = form.inputs.clone();
        let (title, submit, width): (String, &str, f32) = match &form.kind {
            Kind::File(file) if file.version == 0 => ("New document".into(), "Save", 560.0),
            Kind::File(file) => (file.path.clone(), "Save", 560.0),
            Kind::Decision(_) => ("Answer".into(), "Send answer", 420.0),
            Kind::Thread(Some(_)) => ("Message thread".into(), "Send", 420.0),
            Kind::Thread(None) => ("New thread".into(), "Start thread", 460.0),
            Kind::Config => (String::new(), "", 0.0),
        };
        let question = match &form.kind {
            Kind::Decision(decision) => Some(decision.question.clone()),
            _ => None,
        };
        let mut body = div().mt(px(14.0)).flex().flex_col().gap(px(12.0));
        body = match &form.kind {
            Kind::File(file) if file.version == 0 => body
                .child(labelled(&theme, "Name", inputs[0].clone(), 36.0, cx))
                .child(labelled(&theme, "Content", inputs[1].clone(), 260.0, cx)),
            Kind::File(_) => body.child(text_box(inputs[0].clone(), 300.0, cx)),
            Kind::Thread(None) => body
                .child(labelled(&theme, "Task", inputs[0].clone(), 84.0, cx))
                .child(labelled(&theme, "Start from", inputs[1].clone(), 36.0, cx)),
            _ => body.child(text_box(inputs[0].clone(), 84.0, cx)),
        };
        let projects = matches!(form.kind, Kind::Thread(None)) && form.repositories.len() > 1;
        if projects {
            let state = self.state.read(cx);
            let names: Vec<(String, String)> = form
                .repositories
                .iter()
                .filter_map(|repository| {
                    let space = state.spaces.iter().find(|s| s.id == repository.space_id)?;
                    let name = space
                        .name
                        .clone()
                        .or_else(|| space.path.rsplit('/').next().map(str::to_owned))?;
                    Some((space.id.clone(), name))
                })
                .collect();
            let current = form.space.clone().or_else(|| {
                form.repositories
                    .iter()
                    .find(|r| r.device_id == form.worker_device)
                    .map(|r| r.space_id.clone())
            });
            let mut chips = div().flex().flex_wrap().gap(px(4.0));
            for (id, name) in names {
                let active = current.as_ref() == Some(&id);
                chips = chips.child(
                    chip(format!("zot-thread-space-{id}"), name, active, &theme).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if let Some(form) = this.zot_ui.dialog.as_mut() {
                                form.space = Some(id.clone());
                            }
                            cx.notify();
                        }),
                    ),
                );
            }
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(widgets::field_label(&theme, "Project"))
                    .child(chips),
            );
        }
        let card = popover::dialog_card(&theme)
            .id("zot-dialog-card")
            .role(gpui::Role::Dialog)
            .aria_label(SharedString::from(title.clone()))
            .w(px(width))
            .child(popover::dialog_title(&theme, &title))
            .when_some(question, |d, question| {
                d.child(
                    div()
                        .mt(px(6.0))
                        .child(popover::dialog_body(&theme, question)),
                )
            })
            .child(body)
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .justify_end()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        widgets::text_action(&theme, ActionTone::Quiet, "Cancel")
                            .id("zot-dialog-cancel")
                            .role(gpui::Role::Button)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.zot_ui.dialog = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        popover::btn_primary(&theme, submit)
                            .id("zot-dialog-save")
                            .role(gpui::Role::Button)
                            .when(saving, |d| d.opacity(0.6))
                            .on_click(cx.listener(|this, _, _, cx| this.submit_zot_form(true, cx))),
                    ),
            )
            .into_any_element();
        Some(popover::modal("zot-dialog", viewport, card))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_zot_asks_only_for_name_and_purpose() {
        let labels: Vec<_> = fields(&Kind::Config, None)
            .into_iter()
            .map(|(label, value, _)| {
                assert!(value.is_empty());
                label
            })
            .collect();
        assert_eq!(labels, ["Name", "What this Zapper is responsible for"]);
    }

    #[test]
    fn new_documents_are_named_but_existing_ones_edit_content() {
        let file = |version| ZotFile {
            path: "NOTES.md".into(),
            version,
            content: "kept".into(),
        };
        assert_eq!(fields(&Kind::File(file(0)), None).len(), 2);
        let existing = fields(&Kind::File(file(2)), None);
        assert_eq!(existing.len(), 1);
        assert_eq!(existing[0].1, "kept");
    }
}
