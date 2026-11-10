//! Environments page: pick a repository, then its setup commands, services
//! and secrets, with one save bar for the whole recipe.
use super::super::form;
use super::*;

impl RepositoryPanel {
    fn environment_tabs(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let mut tabs = row()
            .id("environment-repositories")
            .role(Role::TabList)
            .aria_label("Repository")
            .flex_wrap()
            .gap(px(6.));
        for project in &self.projects {
            let id = project.project_id.clone();
            let selected = self.environment.project.as_ref() == Some(&id);
            let identity = project
                .repository_identity
                .trim_start_matches("github.com/")
                .to_owned();
            let avatar_url = self.github.as_ref().and_then(|g| {
                g.repositories
                    .iter()
                    .find(|r| r.name_with_owner.eq_ignore_ascii_case(&identity))
                    .and_then(|r| r.owner_avatar_url.clone())
            });
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("environment-{id}")))
                    .role(Role::Tab)
                    .aria_label(SharedString::from(identity.clone()))
                    .aria_selected(selected)
                    .tab_index(0)
                    .h(px(30.))
                    .pl(px(6.))
                    .pr(px(10.))
                    .rounded(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .cursor_pointer()
                    .border_1()
                    .text_size(px(12.))
                    .when(selected, |v| {
                        v.bg(s::frame_bg())
                            .border_color(palette::rgb(TEXT).opacity(0.18))
                            .text_color(palette::rgb(TEXT))
                    })
                    .when(!selected, |v| {
                        v.border_color(palette::rgb(BORDER).opacity(0.6))
                            .text_color(palette::rgb(MUTED))
                            .hover(|v| v.text_color(palette::rgb(TEXT)))
                    })
                    .child(super::super::presentation::avatar(
                        &identity,
                        avatar_url.as_deref(),
                        18.,
                    ))
                    .child(identity)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.environment.project.as_ref() != Some(&id) {
                            this.select_environment(id.clone(), cx)
                        }
                    })),
            );
        }
        tabs
    }

    fn environment_commands(&self, busy: bool, cx: &mut Context<Self>) -> Div {
        let mut body = column().gap(px(16.)).p(px(12.));
        if self.environment.recording {
            body = body.child(
                column()
                    .gap(px(8.))
                    .p(px(10.))
                    .rounded(px(8.))
                    .bg(s::frame_bg().opacity(0.5))
                    .child(
                        row()
                            .items_baseline()
                            .gap(px(8.))
                            .child(s::title_text("Add a tested command"))
                            .child(s::description_text("It's appended to Install.")),
                    )
                    .child(form::script_box(self.environment.record_command.clone()))
                    .child(
                        row()
                            .gap(px(8.))
                            .justify_end()
                            .child(
                                cs::button("cancel-record-command", "Cancel", false, busy, false)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.environment.recording = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                cs::button(
                                    "record-cloud-command",
                                    "Add to Install",
                                    true,
                                    busy,
                                    false,
                                )
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.append_tested_command(cx)),
                                ),
                            ),
                    ),
            );
        }
        body = body
            .child(form::script_field(
                "Install",
                "Runs once on each fresh computer",
                self.environment.initialize.clone(),
            ))
            .child(form::script_field(
                "Refresh dependencies",
                "Runs when the lockfile or setup changes",
                self.environment.refresh.clone(),
            ));
        form::section(
            "Setup commands",
            "Run before the agent starts, from the repository root. Use secrets below for tokens.",
            (!self.environment.recording).then(|| {
                cs::button(
                    "open-record-cloud-command",
                    "Add tested command",
                    false,
                    busy,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.environment.recording = true;
                    cx.notify();
                }))
                .into_any_element()
            }),
            body,
        )
    }

    fn environment_services(&self, busy: bool, cx: &mut Context<Self>) -> Div {
        let mut body = column();
        if self.environment.startup.is_empty() && !self.environment.adding_service {
            body = body.child(form::empty(
                "None yet. Add one if the agent needs a dev server or database running.",
            ));
        }
        for (index, service) in self.environment.startup.iter().enumerate() {
            body = body.child(
                row()
                    .items_center()
                    .gap(px(12.))
                    .px(px(12.))
                    .py(px(10.))
                    .when(index > 0, |v| {
                        v.border_t_1().border_color(palette::rgb(BORDER))
                    })
                    .child(
                        div()
                            .flex_none()
                            .size(px(8.))
                            .rounded_full()
                            .bg(palette::tone(
                                hsla(83. / 360., 0.72, 0.46, 1.),
                                hsla(83. / 360., 0.74, 0.43, 1.),
                            )),
                    )
                    .child(
                        column()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(
                                row()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(s::title_text(service.name.clone()))
                                    .when_some(service.port, |v, port| {
                                        v.child(cs::badge(format!(":{port}"), false, true))
                                    }),
                            )
                            .child(form::mono(service.command.clone())),
                    )
                    .child(
                        cs::icon_button(
                            SharedString::from(format!("remove-startup-{index}")),
                            "trash-bin-minimalistic",
                            format!("Remove {}", service.name),
                            busy,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if index < this.environment.startup.len() {
                                this.environment.startup.remove(index);
                            }
                            cx.notify();
                        })),
                    ),
            );
        }
        if self.environment.adding_service {
            body = body.child(
                column()
                    .gap(px(8.))
                    .p(px(12.))
                    .when(!self.environment.startup.is_empty(), |v| {
                        v.border_t_1().border_color(palette::rgb(BORDER))
                    })
                    .child(
                        row()
                            .gap(px(8.))
                            .child(form::line_field(self.environment.service_name.clone()))
                            .child(
                                div()
                                    .w(px(96.))
                                    .flex_none()
                                    .flex()
                                    .child(form::line_field(self.environment.service_port.clone())),
                            ),
                    )
                    .child(row().child(form::line_field(self.environment.service_command.clone())))
                    .child(
                        row()
                            .gap(px(8.))
                            .justify_end()
                            .child(
                                cs::button("cancel-startup-service", "Cancel", false, busy, false)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.environment.adding_service = false;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                cs::button("add-startup-service", "Add service", true, busy, false)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.add_startup_service(cx)),
                                    ),
                            ),
                    ),
            );
        }
        form::section(
            "Services",
            "Long-running processes that start with the computer and restart if it reboots.",
            (!self.environment.adding_service).then(|| {
                cs::icon_label_button(
                    "open-startup-service",
                    "plus",
                    "Add service",
                    false,
                    busy,
                    false,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.environment.adding_service = true;
                    cx.notify();
                }))
                .into_any_element()
            }),
            body,
        )
    }

    pub(in crate::cloud_repositories) fn render_environment(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut root = column().gap(px(16.));
        if self.projects.is_empty() {
            return root.child(form::section(
                "No repositories yet",
                "Add a repository first. Then give it setup commands, services and secrets here.",
                Some(
                    cs::button(
                        "environment-add-repository",
                        "Add repository",
                        true,
                        false,
                        false,
                    )
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(OpenPage(CloudPage::Repositories))))
                    .into_any_element(),
                ),
                div(),
            ));
        }
        if self.projects.len() > 1 {
            root = root.child(self.environment_tabs(cx));
        }
        let busy = self
            .pending
            .values()
            .any(|p| p.kind.starts_with("environment"));
        match &self.environment.version {
            None => {
                root = root.child(form::section(
                    "Setup commands",
                    if busy {
                        "Loading this repository's setup…"
                    } else {
                        "This repository's setup couldn't be loaded."
                    },
                    None,
                    div(),
                ));
            }
            Some(version) if version.source == EnvironmentSource::Repository => {
                root = root.child(form::section(
                    "Managed by the repository",
                    "This repository's .zuse/setup.json defines its setup. Edit that file to change commands or services.",
                    None,
                    div(),
                ));
                if self.environment.project.is_some() {
                    root = root.child(self.render_variables(window, cx));
                }
            }
            Some(version) => {
                let dirty = self.environment_dirty(cx);
                root = root
                    .child(self.environment_commands(busy, cx))
                    .child(self.environment_services(busy, cx));
                if self.environment.project.is_some() {
                    root = root.child(self.render_variables(window, cx));
                }
                if let Some(notice) = &self.environment.notice {
                    root = root.child(form::message(notice, false));
                }
                if let Some(error) = &self.environment.error {
                    root = root.child(form::message(error, true));
                }
                return root.child(form::save_bar(
                    "cloud-environment",
                    format!(
                        "Version {} · Changes apply to new computers, not running ones.",
                        version.revision
                    ),
                    dirty,
                    busy,
                    |this, cx| this.discard_environment(cx),
                    |this, cx| this.save_environment(cx),
                    cx,
                ));
            }
        }
        if let Some(error) = &self.environment.error {
            root = root.child(form::message(error, true));
        }
        root
    }
}
