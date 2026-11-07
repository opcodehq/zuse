//! Coordinator chrome; commands use the same engine authority as external clients.
use super::*;
use zuse_protocol::presentation::zot_panel::{ZotTab, ZotTabs, zot_owns_chat};
use zuse_protocol::zots::*;

#[derive(Default)]
pub(super) struct ZotUi {
    pub form: Option<Form>,
    tabs: ZotTabs,
    files: Vec<ZotFile>,
    loaded: Option<(String, u32, u32)>,
    events: Vec<ZotEvent>,
    loading: bool,
    pending: bool,
    retry: Option<ZotCommand>,
    history: bool,
    confirm_delete: bool,
}
pub(super) struct Form {
    id: String,
    zot: Option<Zot>,
    file: Option<ZotFile>,
    decision: Option<ZotDecision>,
    worker: Option<Option<String>>,
    inputs: Vec<Entity<ComposerInput>>,
    repositories: Vec<ZotRepository>,
    home: String,
    worker_device: String,
    _subscriptions: Vec<Subscription>,
}
fn action(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    theme: &Theme,
    callback: impl Fn(&mut Shell, &gpui::ClickEvent, &mut Window, &mut Context<Shell>) + 'static,
    cx: &mut Context<Shell>,
) -> AnyElement {
    div()
        .id(id.into())
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(4.0))
        .cursor_pointer()
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.text)
        .hover(|s| s.bg(theme.accent.opacity(0.12)))
        .child(label.into())
        .on_click(cx.listener(callback))
        .into_any_element()
}

fn section(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .pb(px(4.0))
        .text_size(crate::typography::ui_rems(11.0))
        .text_color(theme.text_faint)
        .child(text.into())
}
fn muted(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .px(px(6.0))
        .py(px(4.0))
        .text_size(crate::typography::ui_rems(12.0))
        .text_color(theme.text_faint)
        .child(text.into())
}
#[allow(clippy::too_many_arguments)]
fn list_row(
    id: impl Into<SharedString>,
    glyph: &'static str,
    tint: gpui::Hsla,
    title: String,
    trailing: Option<String>,
    theme: &Theme,
    callback: impl Fn(&mut Shell, &gpui::ClickEvent, &mut Window, &mut Context<Shell>) + 'static,
    cx: &mut Context<Shell>,
) -> AnyElement {
    div()
        .id(id.into())
        .role(gpui::Role::Button)
        .aria_label(SharedString::from(title.clone()))
        .flex()
        .items_center()
        .gap(px(8.0))
        .px(px(6.0))
        .py(px(5.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(crate::typography::ui_rems(12.5))
        .text_color(theme.text)
        .hover(|d| d.bg(theme.element_hover))
        .child(icon(glyph).size(px(13.0)).flex_none().text_color(tint))
        .child(div().flex_1().min_w_0().truncate().child(title))
        .when_some(trailing, |d, t| {
            d.child(
                div()
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_faint)
                    .child(t),
            )
        })
        .on_click(cx.listener(callback))
        .into_any_element()
}

impl Shell {
    pub(super) fn selected_zot(&self, cx: &App) -> Option<Zot> {
        let state = self.state.read(cx);
        state
            .zots
            .iter()
            .find(|z| Some(&z.coordinator_chat_id) == state.selected_chat.as_ref())
            .cloned()
    }
    pub(super) fn render_zot_sidebar(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        if !state
            .engine()
            .is_some_and(|e| e.engine_info().supports(zuse_protocol::zots::CAPABILITY))
        {
            return Empty.into_any_element();
        }
        let zots = state.zots.clone();
        let selected = state.selected_chat.clone();
        let titles: std::collections::HashMap<String, String> = state
            .chats
            .iter()
            .filter_map(|c| Some((c.id.clone(), c.title.clone()?)))
            .collect();
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(26.0))
            .px(px(8.0))
            .text_size(crate::typography::ui_rems(11.0))
            .text_color(theme.text_faint)
            .child("Zots")
            .child(
                div()
                    .id("new-zot")
                    .role(gpui::Role::Button)
                    .aria_label("New Zot")
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .hover(|d| d.bg(theme.element_hover))
                    .tooltip(crate::settings::widgets::text_tooltip("New Zot"))
                    .child(
                        icon(icons::PLUS)
                            .size(px(13.0))
                            .text_color(theme.text_muted),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.open_zot_form(None, None, cx))),
            );
        let mut list = div()
            .flex()
            .flex_col()
            .px(px(8.0))
            .pt(px(6.0))
            .pb(px(8.0))
            .gap(px(1.0))
            .child(header);
        for zot in zots {
            let id = zot.coordinator_chat_id.clone();
            let active = selected.as_deref() == Some(id.as_str())
                || zot
                    .previous_coordinator_chats
                    .iter()
                    .any(|c| Some(c) == selected.as_ref());
            let needs = zot.decisions.iter().filter(|d| d.answer.is_none()).count();
            let initial = zot
                .config
                .name
                .chars()
                .next()
                .unwrap_or('Z')
                .to_uppercase()
                .to_string();
            list = list.child(
                div()
                    .id(SharedString::from(format!("zot-{}", zot.id)))
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(zot.config.name.clone()))
                    .h(px(28.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(if active {
                        theme.text_strong
                    } else {
                        theme.text
                    })
                    .when(active, |d| d.bg(theme.bg_selected))
                    .when(!active, |d| d.hover(|d| d.bg(theme.element_hover)))
                    .child(
                        div()
                            .size(px(18.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.0))
                            .bg(theme.accent_wash)
                            .text_size(crate::typography::ui_rems(10.0))
                            .text_color(theme.accent_strong)
                            .child(initial),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(zot.config.name.clone()),
                    )
                    .when(zot.status != ZotStatus::Active, |d| {
                        d.child(
                            div()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_faint)
                                .child(format!("{:?}", zot.status)),
                        )
                    })
                    .when(needs > 0, |d| {
                        d.child(
                            div()
                                .px(px(5.0))
                                .rounded_full()
                                .bg(theme.warning_muted)
                                .text_size(crate::typography::ui_rems(10.0))
                                .text_color(theme.warning)
                                .child(needs.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.route = Route::Chat;
                        this.zot_ui.form = None;
                        this.state
                            .update(cx, |s, cx| s.select_chat(Some(id.clone()), cx));
                        cx.notify();
                    })),
            );
            // A Zot's threads live under it instead of under their projects.
            let count = zot.workers.len();
            for (index, worker) in zot.workers.iter().enumerate() {
                let chat = worker.chat_id.clone();
                let current = selected.as_deref() == Some(chat.as_str());
                let dot = self.zot_status_color(&chat, theme, cx);
                let title = titles
                    .get(&chat)
                    .cloned()
                    .unwrap_or_else(|| worker.objective.clone());
                list = list.child(
                    div()
                        .id(SharedString::from(format!("zot-thread-{chat}")))
                        .role(gpui::Role::Button)
                        .aria_label(SharedString::from(title.clone()))
                        .h(px(26.0))
                        .ml(px(14.0))
                        .pl(px(10.0))
                        .pr(px(6.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .relative()
                        .rounded(px(7.0))
                        .cursor_pointer()
                        .text_size(crate::typography::ui_rems(12.5))
                        .text_color(if current {
                            theme.text_strong
                        } else {
                            theme.text_muted
                        })
                        .when(current, |d| d.bg(theme.bg_selected))
                        .when(!current, |d| {
                            d.hover(|d| d.bg(theme.element_hover).text_color(theme.text))
                        })
                        // Tree guide: a vertical rule joining the threads to their Zot.
                        .child(
                            div()
                                .absolute()
                                .left(px(0.0))
                                .top(px(0.0))
                                .w(px(1.0))
                                .h(px(if index + 1 == count { 13.0 } else { 26.0 }))
                                .bg(theme.border),
                        )
                        .child(
                            div()
                                .absolute()
                                .left(px(0.0))
                                .top(px(13.0))
                                .w(px(6.0))
                                .h(px(1.0))
                                .bg(theme.border),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(title))
                        .child(div().size(px(6.0)).flex_none().rounded_full().bg(dot))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.route = Route::Chat;
                            this.zot_ui.form = None;
                            this.state
                                .update(cx, |s, cx| s.select_chat(Some(chat.clone()), cx));
                            cx.notify();
                        })),
                );
            }
        }
        list.into_any_element()
    }
    /// Live run state of one thread, ignoring rows a crashed host left behind.
    fn zot_thread_status(&self, chat: &str, cx: &App) -> Option<zuse_protocol::SessionStatus> {
        self.state
            .read(cx)
            .sessions
            .iter()
            .find(|s| s.chat_id == chat && (chrono::Utc::now() - s.updated_at).num_seconds() < 45)
            .map(|s| s.status.clone())
    }
    fn zot_status_color(&self, chat: &str, theme: &Theme, cx: &App) -> gpui::Hsla {
        use zuse_protocol::SessionStatus as S;
        match self.zot_thread_status(chat, cx) {
            Some(S::Working) => theme.busy,
            Some(S::AwaitingInput) => theme.warning,
            Some(S::Errored) => theme.danger,
            _ => theme.text_faint.opacity(0.5),
        }
    }
    fn open_zot_form(&mut self, zot: Option<Zot>, file: Option<ZotFile>, cx: &mut Context<Self>) {
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
        let values = if let Some(file) = file.as_ref().filter(|f| f.version == 0) {
            vec![("Name", file.path.clone()), ("Content", String::new())]
        } else if let Some(file) = &file {
            vec![("Content", file.content.clone())]
        } else {
            vec![
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
                    "Model (empty uses agent default)",
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
            ]
        };
        let mut subscriptions = vec![];
        let inputs = values
            .into_iter()
            .map(|(label, value)| {
                let input = cx.new(|cx| ComposerInput::new(label, cx));
                input.update(cx, |i, cx| i.set_text(value, cx));
                subscriptions.push(
                    cx.subscribe(&input, |_: &mut Shell, _, _: &ComposerInputEvent, cx| {
                        cx.notify()
                    }),
                );
                input
            })
            .collect();
        self.zot_ui.form = Some(Form {
            id: zot
                .as_ref()
                .map(|z| z.id.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            zot,
            file,
            decision: None,
            worker: None,
            inputs,
            repositories,
            home,
            worker_device,
            _subscriptions: subscriptions,
        });
        self.route = Route::Chat;
        cx.notify();
    }
    fn submit_zot_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.zot_ui.form.as_ref() else {
            return;
        };
        let values: Vec<String> = form
            .inputs
            .iter()
            .map(|i| i.read(cx).text().to_owned())
            .collect();
        let (id, revision, action) = if let Some(decision) = &form.decision {
            let zot = form.zot.as_ref().unwrap();
            let mut decision = decision.clone();
            decision.answer = Some(values[0].clone());
            (
                zot.id.clone(),
                Some(zot.revision),
                ZotAction::Decide { decision },
            )
        } else if let Some(worker) = &form.worker {
            let zot = form.zot.as_ref().unwrap();
            let action = if let Some(chat_id) = worker {
                ZotAction::MessageWorker {
                    chat_id: chat_id.clone(),
                    text: values[0].clone(),
                    automatic: false,
                }
            } else {
                let Some(repository) = form
                    .repositories
                    .iter()
                    .find(|r| r.device_id == form.worker_device)
                else {
                    self.show_notice(
                        crate::toast::ToastKind::Error,
                        "Select a repository on the worker environment",
                        cx,
                    );
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
            };
            (zot.id.clone(), Some(zot.revision), action)
        } else if let Some(file) = &form.file {
            let zot = form.zot.as_ref().unwrap();
            let (path, content) = if file.version == 0 {
                (values[0].trim().to_owned(), values[1].clone())
            } else {
                (file.path.clone(), values[0].clone())
            };
            if path.is_empty() {
                self.show_notice(crate::toast::ToastKind::Error, "Name the document", cx);
                return;
            }
            (
                zot.id.clone(),
                Some(zot.revision),
                ZotAction::WriteFile {
                    path,
                    content,
                    expected_version: file.version,
                },
            )
        } else {
            let (Ok(max_concurrent_workers), Ok(max_followups)) =
                (values[4].parse(), values[5].parse())
            else {
                self.show_notice(
                    crate::toast::ToastKind::Error,
                    "Enter valid worker limits",
                    cx,
                );
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
            if let Some(zot) = &form.zot {
                (
                    zot.id.clone(),
                    Some(zot.revision),
                    ZotAction::Configure { config },
                )
            } else {
                (form.id.clone(), None, ZotAction::Create { config })
            }
        };
        self.send_zot_command(id, revision, action, true, cx);
    }
    fn send_zot_command(
        &mut self,
        id: String,
        expected_revision: Option<u32>,
        action: ZotAction,
        close_form: bool,
        cx: &mut Context<Self>,
    ) {
        if self.zot_ui.pending {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let home = self
            .zot_ui
            .form
            .as_ref()
            .map(|f| f.home.clone())
            .or_else(|| {
                self.state
                    .read(cx)
                    .zots
                    .iter()
                    .find(|z| z.id == id)
                    .map(|z| z.config.home_device_id.clone())
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
                        this.zot_ui.loaded = None;
                        if close_form {
                            this.zot_ui.form = None;
                        }
                        if close_form {
                            if let Ok(Some(zot)) = serde_json::from_value::<Option<Zot>>(value) {
                                this.state.update(cx, |s, cx| {
                                    s.select_chat(Some(zot.coordinator_chat_id), cx)
                                });
                            }
                        }
                    }
                    Err(error) => {
                        this.show_notice(crate::toast::ToastKind::Error, error.to_string(), cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    pub(super) fn render_zot_form(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let form = self.zot_ui.form.as_ref().unwrap();
        let file = form.file.as_ref().map(|f| f.path.clone());
        let inputs = form.inputs.clone();
        let labels: Vec<&str> = if form.file.as_ref().is_some_and(|f| f.version == 0) {
            vec!["Name", "Content"]
        } else if form.file.is_some() {
            vec!["Content"]
        } else if form.decision.is_some() {
            vec!["Your answer"]
        } else if let Some(worker) = &form.worker {
            if worker.is_some() {
                vec!["Message"]
            } else {
                vec!["Objective", "Agent", "Model", "Base branch"]
            }
        } else {
            vec![
                "Name",
                "Purpose",
                "Agent",
                "Model",
                "Concurrent workers",
                "Automatic follow-ups",
            ]
        };
        let devices = self.state.read(cx).devices.clone();
        let spaces = self.state.read(cx).spaces.clone();
        let selected = form.repositories.clone();
        let home = form.home.clone();
        let worker_device = form.worker_device.clone();
        let title = file
            .filter(|path| !path.is_empty() && form.file.as_ref().is_some_and(|f| f.version > 0))
            .unwrap_or_else(|| {
                if form.file.is_some() {
                    return "New document".into();
                }
                if form.decision.is_some() {
                    "Answer decision".into()
                } else if form.worker.is_some() {
                    "Worker task".into()
                } else if form.zot.is_some() {
                    "Zot settings".into()
                } else {
                    "Create a Zot".into()
                }
            });
        div()
            .id("zot-form")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .p(px(24.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(title)
            .children(inputs.into_iter().enumerate().map(|(index, input)| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(labels[index])
                    .child(div().min_h(px(36.0)).child(input))
            }))
            .when(form.file.is_none() && form.decision.is_none(), |view| {
                view.child("Home environment")
                    .children(devices.iter().map(|device| {
                        let id = device.id.clone();
                        action(
                            format!("zot-home-{id}"),
                            format!("{}{}", if id == home { "✓ " } else { "" }, device.name),
                            theme,
                            move |this, _, _, cx| {
                                if let Some(f) = this.zot_ui.form.as_mut() {
                                    if f.zot.is_none() {
                                        f.home = id.clone();
                                    }
                                }
                                cx.notify();
                            },
                            cx,
                        )
                    }))
                    .child("Default worker environment")
                    .children(devices.iter().map(|device| {
                        let id = device.id.clone();
                        action(
                            format!("zot-worker-{id}"),
                            format!(
                                "{}{}",
                                if id == worker_device { "✓ " } else { "" },
                                device.name
                            ),
                            theme,
                            move |this, _, _, cx| {
                                if let Some(f) = this.zot_ui.form.as_mut() {
                                    f.worker_device = id.clone();
                                }
                                cx.notify();
                            },
                            cx,
                        )
                    }))
                    .child("Repositories")
                    .children(spaces.into_iter().map(|space| {
                        let checked = selected.iter().any(|r| r.space_id == space.id);
                        let id = space.id.clone();
                        action(
                            format!("zot-repo-{id}"),
                            format!(
                                "{}{}",
                                if checked { "✓ " } else { "" },
                                space.name.unwrap_or(space.path)
                            ),
                            theme,
                            move |this, _, _, cx| {
                                if let Some(f) = this.zot_ui.form.as_mut() {
                                    if f.repositories.iter().any(|r| r.space_id == id) {
                                        f.repositories.retain(|r| r.space_id != id);
                                    } else {
                                        f.repositories.push(ZotRepository {
                                            space_id: id.clone(),
                                            device_id: space.device_id.clone(),
                                        });
                                    }
                                }
                                cx.notify();
                            },
                            cx,
                        )
                    }))
            })
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(action(
                        "zot-save",
                        if self.zot_ui.pending {
                            "Saving…"
                        } else {
                            "Save"
                        },
                        theme,
                        |this, _, _, cx| this.submit_zot_form(cx),
                        cx,
                    ))
                    .child(action(
                        "zot-cancel",
                        "Cancel",
                        theme,
                        |this, _, _, cx| {
                            this.zot_ui.form = None;
                            cx.notify();
                        },
                        cx,
                    )),
            )
            .into_any_element()
    }
    fn zot_tab_bar(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.zot_ui.tabs;
        let selected_bg = theme.element_active;
        let mut strip = div().flex().items_center().gap(px(2.0));
        for tab in ZotTab::ALL {
            let active = tab == tabs.active;
            let path = match tab {
                ZotTab::Overview => icons::WIDGET,
                ZotTab::Threads => icons::CHAT_ROUND_LINE,
                ZotTab::Drive => icons::FOLDER_OPEN,
                ZotTab::History => icons::HISTORY,
            };
            let label = tab.label();
            let width = tab.label_width();
            let color = if active {
                theme.text_strong
            } else {
                theme.text_muted
            };
            let base = div()
                .id(SharedString::from(format!("zot-tab-{label}")))
                .role(gpui::Role::Tab)
                .aria_label(label)
                .h(px(28.0))
                .px(px(7.0))
                .flex()
                .items_center()
                .rounded(px(7.0))
                .cursor_pointer()
                .text_size(crate::typography::ui_rems(12.5))
                .text_color(color)
                .when(!active, |d| {
                    d.hover(|d| d.bg(theme.element_hover).text_color(theme.text))
                        .tooltip(crate::settings::widgets::text_tooltip(label))
                })
                .child(icon(path).size(px(15.0)).text_color(color))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.zot_ui.tabs.select(tab) {
                        let history = tab == ZotTab::History;
                        if this.zot_ui.history != history {
                            this.zot_ui.history = history;
                            this.zot_ui.loaded = None;
                        }
                        cx.notify();
                    }
                }));
            // The active tab grows its label; the one it replaced folds back.
            let reveal = move |el: gpui::Stateful<gpui::Div>, r: f32| {
                el.bg(selected_bg.opacity(r)).when(r > 0.0, |el| {
                    el.child(
                        div()
                            .ml(px(6.0 * r))
                            .max_w(px(width * r))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .opacity(r)
                            .child(label),
                    )
                })
            };
            let animating = tabs.epoch > 0 && (active || tabs.previous == Some(tab));
            strip = strip.child(if animating {
                base.with_animation(
                    SharedString::from(format!("zot-tab-{label}-{}", tabs.epoch)),
                    RESIZE.animation(),
                    move |el, t| reveal(el, tabs.reveal(tab, t)),
                )
                .into_any_element()
            } else {
                reveal(base, tabs.reveal(tab, 1.0)).into_any_element()
            });
        }
        let icon_button = |id: &'static str, path: &'static str, tip: &'static str| {
            div()
                .id(id)
                .role(gpui::Role::Button)
                .aria_label(tip)
                .size(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.0))
                .cursor_pointer()
                .hover(|d| d.bg(theme.element_hover))
                .tooltip(crate::settings::widgets::text_tooltip(tip))
                .child(icon(path).size(px(15.0)).text_color(theme.text_muted))
        };
        let zot = self.selected_zot(cx);
        let new_doc = zot.clone();
        div()
            .flex()
            .items_center()
            .h(px(44.0))
            .flex_none()
            .px(px(8.0))
            .border_b_1()
            .border_color(theme.border)
            .child(strip)
            .child(div().flex_1())
            .when(tabs.active == ZotTab::Drive, |d| {
                d.child(
                    icon_button("zot-new-document", icons::DOCUMENT_ADD, "New document").on_click(
                        cx.listener(move |this, _, _, cx| {
                            let file = ZotFile {
                                path: String::new(),
                                version: 0,
                                content: String::new(),
                            };
                            this.open_zot_form(new_doc.clone(), Some(file), cx)
                        }),
                    ),
                )
            })
            .child(
                icon_button("zot-settings", icons::SETTINGS, "Zot settings").on_click(
                    cx.listener(move |this, _, _, cx| this.open_zot_form(zot.clone(), None, cx)),
                ),
            )
            .into_any_element()
    }

    pub(super) fn render_zot_inspector(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(zot) = self.selected_zot(cx) else {
            return Empty.into_any_element();
        };
        let theme = Theme::of(cx).clone();
        let key = (zot.id.clone(), zot.revision, zot.activity_revision);
        if self.zot_ui.loaded.as_ref() != Some(&key) && !self.zot_ui.loading {
            if let Some(engine) = self.state.read(cx).engine().cloned() {
                self.zot_ui.loading = true;
                let request = serde_json::json!({"zotId":zot.id,"targetDeviceId":zot.config.home_device_id,"history":self.zot_ui.history});
                cx.spawn(async move |this, cx| {
                    let result = engine
                        .client()
                        .call(zuse_protocol::control::ZOT_FILES, request.clone())
                        .await;
                    let events = engine
                        .client()
                        .call(zuse_protocol::control::ZOT_EVENTS, request)
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        this.zot_ui.loading = false;
                        if let Ok(value) = events {
                            if let Ok(events) = serde_json::from_value(value) {
                                this.zot_ui.events = events;
                            }
                        }
                        if let Ok(value) = result {
                            if let Ok(files) = serde_json::from_value(value) {
                                this.zot_ui.files = files;
                                this.zot_ui.loaded = Some(key);
                            }
                        }
                        cx.notify();
                    });
                })
                .detach();
            }
        }
        let files = if self
            .zot_ui
            .loaded
            .as_ref()
            .is_some_and(|(id, _, _)| id == &zot.id)
        {
            self.zot_ui.files.clone()
        } else {
            vec![]
        };
        let body = match self.zot_ui.tabs.active {
            ZotTab::Overview => self.zot_overview(&zot, &theme, cx),
            ZotTab::Threads => self.zot_threads(&zot, &theme, cx),
            ZotTab::Drive => self.zot_drive(&zot, files, &theme, cx),
            ZotTab::History => self.zot_history(&zot, files, &theme, cx),
        };
        div()
            .id("zot-inspector")
            .w(px(340.0))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(theme.border)
            .child(self.zot_tab_bar(&theme, cx))
            .child(
                div()
                    .id("zot-inspector-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(12.0))
                    .flex()
                    .flex_col()
                    .gap(px(14.0))
                    .child(body),
            )
            .into_any_element()
    }

    fn zot_overview(&mut self, zot: &Zot, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let working = zot
            .workers
            .iter()
            .filter(|w| {
                self.zot_thread_status(&w.chat_id, cx)
                    == Some(zuse_protocol::SessionStatus::Working)
            })
            .count();
        let (status_color, status) = match zot.status {
            ZotStatus::Active => (theme.success, "Active"),
            ZotStatus::Paused => (theme.warning, "Paused"),
            ZotStatus::Stopped => (theme.text_faint, "Stopped"),
            ZotStatus::Archived => (theme.text_faint, "Archived"),
        };
        let id = zot.id.clone();
        let rev = zot.revision;
        let toggle = if zot.status == ZotStatus::Active {
            ZotStatus::Paused
        } else {
            ZotStatus::Active
        };
        let mut view = div().flex().flex_col().gap(px(14.0)).child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(15.0))
                        .text_color(theme.text_strong)
                        .child(zot.config.name.clone()),
                )
                .when(!zot.config.purpose.is_empty(), |d| {
                    d.child(
                        div()
                            .text_size(crate::typography::ui_rems(12.5))
                            .text_color(theme.text_muted)
                            .child(zot.config.purpose.clone()),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .text_size(crate::typography::ui_rems(11.5))
                        .text_color(theme.text_faint)
                        .child(div().size(px(6.0)).rounded_full().bg(status_color))
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
                                d.child(action(
                                    "zot-pause",
                                    if toggle == ZotStatus::Paused {
                                        "Pause"
                                    } else {
                                        "Resume"
                                    },
                                    theme,
                                    move |this, _, _, cx| {
                                        this.send_zot_command(
                                            id.clone(),
                                            Some(rev),
                                            ZotAction::SetStatus { status: toggle },
                                            false,
                                            cx,
                                        )
                                    },
                                    cx,
                                ))
                            },
                        ),
                ),
        );
        let mut needs = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section("Needs you", theme));
        let mut empty = true;
        for decision in zot.decisions.iter().filter(|d| d.answer.is_none()).cloned() {
            empty = false;
            let z = zot.clone();
            needs = needs.child(list_row(
                format!("zot-decision-{}", decision.id),
                icons::INFO_CIRCLE,
                theme.warning,
                decision.question.clone(),
                Some("Answer".into()),
                theme,
                move |this, _, _, cx| {
                    this.open_zot_form(Some(z.clone()), None, cx);
                    let input = cx.new(|cx| ComposerInput::new("Your answer", cx));
                    if let Some(form) = this.zot_ui.form.as_mut() {
                        form.inputs = vec![input];
                        form.decision = Some(decision.clone());
                    }
                },
                cx,
            ));
        }
        for event in self
            .zot_ui
            .events
            .clone()
            .into_iter()
            .filter(|e| e.zot_id == zot.id && !e.acknowledged && e.kind != "settled")
        {
            empty = false;
            let id = event.worker_chat_id.clone();
            needs = needs.child(list_row(
                format!("zot-event-{}", event.id),
                icons::CHAT_ROUND_LINE,
                theme.text_muted,
                event.summary,
                None,
                theme,
                move |this, _, _, cx| {
                    this.state
                        .update(cx, |s, cx| s.select_chat(Some(id.clone()), cx));
                },
                cx,
            ));
        }
        if empty {
            needs = needs.child(muted("Nothing needs you right now.", theme));
        }
        view = view.child(needs);
        let mut manage = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section("Manage", theme));
        for (key, label, action_kind) in [
            ("zot-fresh", "Start a fresh conversation", 0),
            ("zot-stop", "Stop Zot", 1),
            ("zot-archive", "Archive Zot and threads", 2),
        ] {
            let z = zot.clone();
            manage = manage.child(action(
                key,
                label,
                theme,
                move |this, _, _, cx| {
                    let (action, close) = match action_kind {
                        0 => (ZotAction::FreshConversation, true),
                        1 => (
                            ZotAction::SetStatus {
                                status: ZotStatus::Stopped,
                            },
                            false,
                        ),
                        _ => (
                            ZotAction::SetStatus {
                                status: ZotStatus::Archived,
                            },
                            false,
                        ),
                    };
                    this.send_zot_command(z.id.clone(), Some(z.revision), action, close, cx)
                },
                cx,
            ));
        }
        if self.zot_ui.confirm_delete {
            let z = zot.clone();
            manage = manage
                .child(muted(
                    "Delete this Zot? Its threads and documents are kept.",
                    theme,
                ))
                .child(action(
                    "zot-delete-confirm",
                    "Delete Zot",
                    theme,
                    move |this, _, _, cx| {
                        this.zot_ui.confirm_delete = false;
                        this.send_zot_command(
                            z.id.clone(),
                            Some(z.revision),
                            ZotAction::Delete,
                            false,
                            cx,
                        );
                    },
                    cx,
                ));
        } else {
            manage = manage.child(action(
                "zot-delete",
                "Delete…",
                theme,
                |this, _, _, cx| {
                    this.zot_ui.confirm_delete = true;
                    cx.notify();
                },
                cx,
            ));
        }
        view.child(manage).into_any_element()
    }

    fn zot_threads(&mut self, zot: &Zot, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let new_zot = zot.clone();
        let mut threads = div().flex().flex_col().gap(px(2.0)).child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(section(format!("Threads · {}", zot.workers.len()), theme))
                .child(action(
                    "zot-new-worker",
                    "New thread",
                    theme,
                    move |this, _, _, cx| this.open_zot_worker_form(new_zot.clone(), None, cx),
                    cx,
                )),
        );
        if zot.workers.is_empty() {
            threads = threads.child(muted(
                "Threads this Zot starts or adopts appear here.",
                theme,
            ));
        }
        for worker in &zot.workers {
            let id = worker.chat_id.clone();
            use zuse_protocol::SessionStatus as S;
            let status = match self.zot_thread_status(&id, cx) {
                Some(S::Working) => "Working",
                Some(S::AwaitingInput) => "Waiting for input",
                Some(S::Errored) => "Failed",
                Some(S::Idle) => "Idle",
                None => "Not running",
            };
            let dot = self.zot_status_color(&id, theme, cx);
            let open = id.clone();
            let mut actions = div().flex().gap(px(2.0)).pl(px(18.0));
            let z = zot.clone();
            let message = id.clone();
            actions = actions.child(action(
                format!("zot-message-{id}"),
                "Message",
                theme,
                move |this, _, _, cx| {
                    this.open_zot_worker_form(z.clone(), Some(message.clone()), cx)
                },
                cx,
            ));
            for (label, kind) in [("Stop", 0), ("Archive", 1), ("Release", 2)] {
                let chat_id = id.clone();
                let z = zot.clone();
                actions = actions.child(action(
                    format!("zot-worker-{kind}-{id}"),
                    label,
                    theme,
                    move |this, _, _, cx| {
                        let chat_id = chat_id.clone();
                        let action = match kind {
                            0 => ZotAction::StopWorker { chat_id },
                            1 => ZotAction::ArchiveWorker { chat_id },
                            _ => ZotAction::ReleaseWorker { chat_id },
                        };
                        this.send_zot_command(z.id.clone(), Some(z.revision), action, false, cx);
                    },
                    cx,
                ));
            }
            threads = threads.child(
                div()
                    .flex()
                    .flex_col()
                    .py(px(4.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("zot-open-{id}")))
                            .role(gpui::Role::Button)
                            .aria_label(SharedString::from(worker.objective.clone()))
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .px(px(6.0))
                            .py(px(5.0))
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .hover(|d| d.bg(theme.element_hover))
                            .child(div().size(px(7.0)).flex_none().rounded_full().bg(dot))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .truncate()
                                            .text_size(crate::typography::ui_rems(13.0))
                                            .text_color(theme.text)
                                            .child(worker.objective.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(crate::typography::ui_rems(11.0))
                                            .text_color(theme.text_faint)
                                            .child(status),
                                    ),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |s, cx| s.select_chat(Some(open.clone()), cx));
                            })),
                    )
                    .child(actions),
            );
        }
        let mut view = div().flex().flex_col().gap(px(14.0)).child(threads);
        let mut coordinator = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section("Coordinator conversations", theme));
        let current = zot.coordinator_chat_id.clone();
        coordinator = coordinator.child(list_row(
            "zot-current-conversation",
            icons::CHAT_ROUND_LINE,
            theme.text_muted,
            "Current conversation".to_owned(),
            None,
            theme,
            move |this, _, _, cx| {
                this.state
                    .update(cx, |s, cx| s.select_chat(Some(current.clone()), cx));
            },
            cx,
        ));
        for (index, id) in zot.previous_coordinator_chats.iter().enumerate().rev() {
            let id = id.clone();
            coordinator = coordinator.child(list_row(
                format!("zot-history-{id}"),
                icons::HISTORY,
                theme.text_faint,
                format!("Conversation {}", index + 1),
                None,
                theme,
                move |this, _, _, cx| {
                    this.state
                        .update(cx, |s, cx| s.select_chat(Some(id.clone()), cx));
                },
                cx,
            ));
        }
        view = view.child(coordinator);
        let candidates: Vec<_> = {
            let state = self.state.read(cx);
            state
                .chats
                .iter()
                .filter(|chat| {
                    chat.parent_chat_id.is_none()
                        && !chat.archived
                        && !zot_owns_chat(&state.zots, &chat.id)
                        && zot.config.repositories.iter().any(|r| {
                            r.device_id == chat.device_id
                                && Some(&r.space_id) == chat.space_id.as_ref()
                        })
                })
                .take(16)
                .cloned()
                .collect()
        };
        if !candidates.is_empty() {
            let mut adopt = div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(section("Add an existing thread", theme));
            for chat in candidates {
                let z = zot.clone();
                let title = chat.title.clone().unwrap_or_else(|| chat.id.clone());
                adopt = adopt.child(list_row(
                    format!("zot-adopt-{}", chat.id),
                    icons::PLUS,
                    theme.text_faint,
                    title.clone(),
                    None,
                    theme,
                    move |this, _, _, cx| {
                        this.send_zot_command(
                            z.id.clone(),
                            Some(z.revision),
                            ZotAction::AdoptWorker {
                                worker: ZotWorker {
                                    chat_id: chat.id.clone(),
                                    device_id: chat.device_id.clone(),
                                    space_id: chat.space_id.clone().unwrap_or_default(),
                                    objective: title.clone(),
                                    followups: 0,
                                },
                            },
                            false,
                            cx,
                        );
                    },
                    cx,
                ));
            }
            view = view.child(adopt);
        }
        view.into_any_element()
    }

    fn zot_drive(
        &mut self,
        zot: &Zot,
        files: Vec<ZotFile>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut view = div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(section(format!("Drive · {}", files.len()), theme));
        if files.is_empty() {
            view = view.child(muted(
                if self.zot_ui.loading {
                    "Loading documents…"
                } else {
                    "Rules, notes and the digest this Zot keeps live here."
                },
                theme,
            ));
        }
        for file in files {
            let z = zot.clone();
            let preview: String = file.content.lines().take(6).collect::<Vec<_>>().join("\n");
            view = view.child(
                div()
                    .id(SharedString::from(format!("zot-edit-{}", file.path)))
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(file.path.clone()))
                    .p(px(10.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface_card)
                    .cursor_pointer()
                    .hover(|d| d.border_color(theme.border_strong))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                icon(icons::DOCUMENT)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(12.5))
                                    .text_color(theme.text)
                                    .child(file.path.clone()),
                            )
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_faint)
                                    .child(format!("v{}", file.version)),
                            ),
                    )
                    .when(!preview.trim().is_empty(), |d| {
                        d.child(
                            div()
                                .text_size(crate::typography::ui_rems(11.5))
                                .text_color(theme.text_muted)
                                .child(preview),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_zot_form(Some(z.clone()), Some(file.clone()), cx)
                    })),
            );
        }
        view.into_any_element()
    }

    fn zot_history(
        &mut self,
        zot: &Zot,
        files: Vec<ZotFile>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let latest = files.iter().fold(
            std::collections::HashMap::<String, u32>::new(),
            |mut versions, file| {
                versions
                    .entry(file.path.clone())
                    .and_modify(|v| *v = (*v).max(file.version))
                    .or_insert(file.version);
                versions
            },
        );
        let mut view = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(section("Document versions", theme));
        if files.is_empty() {
            view = view.child(muted(
                if self.zot_ui.loading {
                    "Loading history…"
                } else {
                    "No versions yet."
                },
                theme,
            ));
        }
        let mut files = files;
        files.sort_by(|a, b| a.path.cmp(&b.path).then(b.version.cmp(&a.version)));
        for file in files {
            let current = *latest.get(&file.path).unwrap_or(&file.version);
            let z = zot.clone();
            view = view.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(6.0))
                    .py(px(5.0))
                    .child(
                        icon(icons::DOCUMENT)
                            .size(px(13.0))
                            .text_color(theme.text_faint),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.5))
                            .text_color(theme.text)
                            .child(file.path.clone()),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(if current == file.version {
                                format!("v{} · current", file.version)
                            } else {
                                format!("v{}", file.version)
                            }),
                    )
                    .when(current > file.version, |d| {
                        d.child(action(
                            format!("zot-restore-{}-{}", file.path, file.version),
                            "Restore",
                            theme,
                            move |this, _, _, cx| {
                                this.send_zot_command(
                                    z.id.clone(),
                                    Some(z.revision),
                                    ZotAction::RestoreFile {
                                        path: file.path.clone(),
                                        version: file.version,
                                        expected_version: current,
                                    },
                                    false,
                                    cx,
                                )
                            },
                            cx,
                        ))
                    }),
            );
        }
        view.into_any_element()
    }
    fn open_zot_worker_form(&mut self, zot: Zot, worker: Option<String>, cx: &mut Context<Self>) {
        self.open_zot_form(Some(zot.clone()), None, cx);
        let values = if worker.is_some() {
            vec![("Message", String::new())]
        } else {
            vec![
                ("Objective", String::new()),
                ("Agent", zot.config.coordinator_model.harness),
                (
                    "Model",
                    zot.config.coordinator_model.model.unwrap_or_default(),
                ),
                ("Base branch", "HEAD".into()),
            ]
        };
        let mut subscriptions = vec![];
        let inputs = values
            .into_iter()
            .map(|(label, value)| {
                let input = cx.new(|cx| ComposerInput::new(label, cx));
                input.update(cx, |i, cx| i.set_text(value, cx));
                subscriptions.push(
                    cx.subscribe(&input, |_: &mut Shell, _, _: &ComposerInputEvent, cx| {
                        cx.notify()
                    }),
                );
                input
            })
            .collect();
        if let Some(form) = self.zot_ui.form.as_mut() {
            form.inputs = inputs;
            form.worker = Some(worker);
            form._subscriptions = subscriptions;
        }
        cx.notify();
    }
}
