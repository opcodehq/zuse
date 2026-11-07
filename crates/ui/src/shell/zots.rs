//! Coordinator chrome; commands use the same engine authority as external clients.
use super::*;
use zuse_protocol::zots::*;

#[derive(Default)]
pub(super) struct ZotUi {
    pub form: Option<Form>,
    tab: usize,
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
        div()
            .flex()
            .flex_col()
            .px(px(8.0))
            .py(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("Zots")
                    .child(action(
                        "new-zot",
                        "+",
                        theme,
                        |this, _, _, cx| this.open_zot_form(None, None, cx),
                        cx,
                    )),
            )
            .children(zots.into_iter().map(|zot| {
                let id = zot.coordinator_chat_id.clone();
                let needs = zot.decisions.iter().filter(|d| d.answer.is_none()).count();
                let label = format!(
                    "{}{}",
                    zot.config.name,
                    if needs > 0 {
                        format!(" · {needs} needs you")
                    } else {
                        String::new()
                    }
                );
                action(
                    format!("zot-{}", zot.id),
                    label,
                    theme,
                    move |this, _, _, cx| {
                        this.route = Route::Chat;
                        this.zot_ui.form = None;
                        this.state
                            .update(cx, |s, cx| s.select_chat(Some(id.clone()), cx));
                        cx.notify();
                    },
                    cx,
                )
            }))
            .into_any_element()
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
        let values = if let Some(file) = &file {
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
            (
                zot.id.clone(),
                Some(zot.revision),
                ZotAction::WriteFile {
                    path: file.path.clone(),
                    content: values[0].clone(),
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
        let labels: Vec<&str> = if form.file.is_some() {
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
        let title = file.unwrap_or_else(|| {
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
        let tab = self.zot_ui.tab;
        let mut view = div()
            .id("zot-inspector")
            .w(px(320.0))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .p(px(12.0))
            .border_l_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                div()
                    .flex()
                    .children(["Overview", "Digest", "Files"].into_iter().enumerate().map(
                        |(tab, label)| {
                            action(
                                format!("zot-tab-{tab}"),
                                label,
                                &theme,
                                move |this, _, _, cx| {
                                    this.zot_ui.tab = tab;
                                    cx.notify();
                                },
                                cx,
                            )
                        },
                    )),
            );
        if tab == 0 {
            view = view.child(format!("{:?}", zot.status));
            view = view.child("Needs you");
            for event in self
                .zot_ui
                .events
                .clone()
                .into_iter()
                .filter(|e| e.zot_id == zot.id && !e.acknowledged && e.kind != "settled")
            {
                let id = event.worker_chat_id.clone();
                view = view.child(action(
                    format!("zot-event-{}", event.id),
                    event.summary,
                    &theme,
                    move |this, _, _, cx| {
                        this.state
                            .update(cx, |s, cx| s.select_chat(Some(id.clone()), cx));
                    },
                    cx,
                ));
            }
            let decisions: Vec<_> = zot
                .decisions
                .iter()
                .filter(|d| d.answer.is_none())
                .collect();
            if decisions.is_empty() {
                view = view.child("No outstanding decisions");
            }
            for decision in decisions {
                let zot = zot.clone();
                let decision = decision.clone();
                view = view.child(action(
                    format!("zot-decision-{}", decision.id),
                    decision.question.clone(),
                    &theme,
                    move |this, _, _, cx| {
                        this.open_zot_form(Some(zot.clone()), None, cx);
                        let input = cx.new(|cx| ComposerInput::new("Your answer", cx));
                        if let Some(form) = this.zot_ui.form.as_mut() {
                            form.inputs = vec![input];
                            form.decision = Some(decision.clone());
                        }
                    },
                    cx,
                ));
            }
            if !zot.previous_coordinator_chats.is_empty() {
                view = view.child("Previous conversations");
            }
            for (index, id) in zot.previous_coordinator_chats.iter().enumerate() {
                let id = id.clone();
                view = view.child(action(
                    format!("zot-history-{id}"),
                    format!("Conversation {}", index + 1),
                    &theme,
                    move |this, _, _, cx| {
                        this.state
                            .update(cx, |s, cx| s.select_chat(Some(id.clone()), cx));
                    },
                    cx,
                ));
            }
            view = view.child("Workers");
            let new_zot = zot.clone();
            view = view.child(action(
                "zot-new-worker",
                "New worker",
                &theme,
                move |this, _, _, cx| this.open_zot_worker_form(new_zot.clone(), None, cx),
                cx,
            ));
            for worker in &zot.workers {
                let id = worker.chat_id.clone();
                let status = self
                    .state
                    .read(cx)
                    .sessions
                    .iter()
                    .find(|s| {
                        s.chat_id == id && (chrono::Utc::now() - s.updated_at).num_seconds() < 45
                    })
                    .map(|s| format!("{:?}", s.status))
                    .unwrap_or_else(|| "Unknown".into());
                view = view.child(action(
                    format!("zot-open-{id}"),
                    format!("{} · {status}", worker.objective),
                    &theme,
                    move |this, _, _, cx| {
                        this.state
                            .update(cx, |s, cx| s.select_chat(Some(id.clone()), cx));
                    },
                    cx,
                ));
                let id = worker.chat_id.clone();
                let z = zot.clone();
                view = view.child(action(
                    format!("zot-message-{id}"),
                    "Message",
                    &theme,
                    move |this, _, _, cx| {
                        this.open_zot_worker_form(z.clone(), Some(id.clone()), cx)
                    },
                    cx,
                ));
                for (label, kind) in [("Stop", 0), ("Archive", 1), ("Release", 2)] {
                    let id = worker.chat_id.clone();
                    let z = zot.clone();
                    view = view.child(action(
                        format!("zot-worker-{kind}-{id}"),
                        label,
                        &theme,
                        move |this, _, _, cx| {
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
                            this.send_zot_command(
                                z.id.clone(),
                                Some(z.revision),
                                action,
                                false,
                                cx,
                            );
                        },
                        cx,
                    ));
                }
            }
            let candidates: Vec<_> = self
                .state
                .read(cx)
                .chats
                .iter()
                .filter(|chat| {
                    chat.parent_chat_id.is_none()
                        && chat.id != zot.coordinator_chat_id
                        && zot.config.repositories.iter().any(|r| {
                            r.device_id == chat.device_id
                                && Some(&r.space_id) == chat.space_id.as_ref()
                        })
                        && !self
                            .state
                            .read(cx)
                            .zots
                            .iter()
                            .any(|z| z.workers.iter().any(|w| w.chat_id == chat.id))
                })
                .take(16)
                .cloned()
                .collect();
            if !candidates.is_empty() {
                view = view.child("Adopt existing conversation");
            }
            for chat in candidates {
                let z = zot.clone();
                let title = chat.title.clone().unwrap_or_else(|| chat.id.clone());
                view = view.child(action(
                    format!("zot-adopt-{}", chat.id),
                    title.clone(),
                    &theme,
                    move |this, _, _, cx| {
                        this.send_zot_command(
                            z.id.clone(),
                            Some(z.revision),
                            ZotAction::AdoptWorker {
                                worker: ZotWorker {
                                    chat_id: chat.id.clone(),
                                    device_id: chat.device_id.clone(),
                                    space_id: chat.space_id.clone().unwrap(),
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
            let id = zot.id.clone();
            let rev = zot.revision;
            let status = if zot.status == ZotStatus::Active {
                ZotStatus::Paused
            } else {
                ZotStatus::Active
            };
            view = view.child(action(
                "zot-pause",
                if status == ZotStatus::Paused {
                    "Pause"
                } else {
                    "Resume"
                },
                &theme,
                move |this, _, _, cx| {
                    this.send_zot_command(
                        id.clone(),
                        Some(rev),
                        ZotAction::SetStatus { status },
                        false,
                        cx,
                    )
                },
                cx,
            ));
            let id = zot.id.clone();
            let rev = zot.revision;
            view = view.child(action(
                "zot-stop",
                "Stop",
                &theme,
                move |this, _, _, cx| {
                    this.send_zot_command(
                        id.clone(),
                        Some(rev),
                        ZotAction::SetStatus {
                            status: ZotStatus::Stopped,
                        },
                        false,
                        cx,
                    )
                },
                cx,
            ));
            let id = zot.id.clone();
            let rev = zot.revision;
            view = view.child(action(
                "zot-fresh",
                "Fresh conversation",
                &theme,
                move |this, _, _, cx| {
                    this.send_zot_command(
                        id.clone(),
                        Some(rev),
                        ZotAction::FreshConversation,
                        true,
                        cx,
                    )
                },
                cx,
            ));
            let z = zot.clone();
            view = view.child(action(
                "zot-archive",
                "Archive Zot and workers",
                &theme,
                move |this, _, _, cx| {
                    this.send_zot_command(
                        z.id.clone(),
                        Some(z.revision),
                        ZotAction::SetStatus {
                            status: ZotStatus::Archived,
                        },
                        false,
                        cx,
                    )
                },
                cx,
            ));
            if self.zot_ui.confirm_delete {
                let z = zot.clone();
                view = view
                    .child("Delete this Zot? Worker conversations and files will be kept.")
                    .child(action(
                        "zot-delete-confirm",
                        "Delete Zot",
                        &theme,
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
                view = view.child(action(
                    "zot-delete",
                    "Delete…",
                    &theme,
                    |this, _, _, cx| {
                        this.zot_ui.confirm_delete = true;
                        cx.notify();
                    },
                    cx,
                ));
            }
        } else {
            view = view.child(action(
                "zot-history",
                if self.zot_ui.history {
                    "Current versions"
                } else {
                    "Version history"
                },
                &theme,
                |this, _, _, cx| {
                    this.zot_ui.history = !this.zot_ui.history;
                    this.zot_ui.loaded = None;
                    cx.notify();
                },
                cx,
            ));
            let latest = self.zot_ui.files.iter().fold(
                std::collections::HashMap::<String, u32>::new(),
                |mut versions, file| {
                    versions
                        .entry(file.path.clone())
                        .and_modify(|v| *v = (*v).max(file.version))
                        .or_insert(file.version);
                    versions
                },
            );
            for file in files
                .into_iter()
                .filter(|f| tab == 2 || f.path == "DIGEST.md")
            {
                view = view
                    .child(format!("{} · version {}", file.path, file.version))
                    .child(file.content.clone());
                let zot = zot.clone();
                let version = *latest.get(&file.path).unwrap_or(&file.version);
                if version > file.version {
                    view = view.child(action(
                        format!("zot-restore-{}-{}", file.path, file.version),
                        "Restore as new version",
                        &theme,
                        move |this, _, _, cx| {
                            this.send_zot_command(
                                zot.id.clone(),
                                Some(zot.revision),
                                ZotAction::RestoreFile {
                                    path: file.path.clone(),
                                    version: file.version,
                                    expected_version: version,
                                },
                                false,
                                cx,
                            )
                        },
                        cx,
                    ));
                } else {
                    view = view.child(action(
                        format!("zot-edit-{}", file.path),
                        "Edit",
                        &theme,
                        move |this, _, _, cx| {
                            this.open_zot_form(Some(zot.clone()), Some(file.clone()), cx)
                        },
                        cx,
                    ));
                }
            }
        }
        let editing = zot.clone();
        view.child(action(
            "zot-settings",
            "Settings",
            &theme,
            move |this, _, _, cx| this.open_zot_form(Some(editing.clone()), None, cx),
            cx,
        ))
        .into_any_element()
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
