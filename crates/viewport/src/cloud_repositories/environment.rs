//! Project setup is versioned independently from any running computer or image cache.
use super::*;
use zuse_protocol::cloud_computers::*;

pub(super) struct EnvironmentView {
    project: Option<String>,
    version: Option<EnvironmentVersion>,
    initialize: Entity<TextInput>,
    record_command: Entity<TextInput>,
    recording: bool,
    refresh: Entity<TextInput>,
    service_name: Entity<TextInput>,
    service_command: Entity<TextInput>,
    service_port: Entity<TextInput>,
    startup: Vec<StartupService>,
    pub error: Option<String>,
    notice: Option<String>,
}
impl EnvironmentView {
    pub(super) fn repository_managed(&self) -> bool {
        self.version
            .as_ref()
            .is_none_or(|version| version.source == EnvironmentSource::Repository)
    }
    pub(super) fn secret_selected(&self, id: &str) -> bool {
        self.version.as_ref().is_some_and(|version| {
            version
                .recipe
                .secret_bindings
                .iter()
                .any(|binding| binding == id)
        })
    }
    pub(super) fn select_secret(&mut self, id: String, selected: bool) {
        if let Some(version) = &mut self.version {
            if version.source == EnvironmentSource::Repository {
                return;
            }
            version
                .recipe
                .secret_bindings
                .retain(|binding| binding != &id);
            if selected {
                version.recipe.secret_bindings.push(id);
            }
        }
    }
    pub fn new(window: &mut Window, cx: &mut Context<RepositoryPanel>) -> Self {
        let initialize = cx.new(|cx| {
            TextInput::multiline(
                "For example: sudo apt-get install -y jq && npm ci",
                window,
                cx,
            )
        });
        let record_command = cx.new(|cx| {
            TextInput::multiline(
                "Paste the installation command you tested on this computer",
                window,
                cx,
            )
        });
        let refresh = cx.new(|cx| TextInput::multiline("For example: npm ci", window, cx));
        let service_name = cx.new(|cx| TextInput::new("Service name", window, cx));
        let service_command =
            cx.new(|cx| TextInput::new("Command, such as npm run dev", window, cx));
        let service_port = cx.new(|cx| TextInput::new("Port (optional)", window, cx));
        for input in [
            &initialize,
            &record_command,
            &refresh,
            &service_name,
            &service_command,
            &service_port,
        ] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        Self {
            project: None,
            version: None,
            initialize,
            record_command,
            recording: false,
            refresh,
            service_name,
            service_command,
            service_port,
            startup: vec![],
            error: None,
            notice: None,
        }
    }
    pub fn reset(&mut self, cx: &mut Context<RepositoryPanel>) {
        self.project = None;
        self.version = None;
        self.startup.clear();
        self.recording = false;
        self.error = None;
        self.notice = None;
        for input in [
            &self.initialize,
            &self.record_command,
            &self.refresh,
            &self.service_name,
            &self.service_command,
            &self.service_port,
        ] {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
    }
}
impl RepositoryPanel {
    pub(super) fn environment_event(&mut self, project: &str, cx: &mut Context<Self>) {
        if self.environment.project.as_deref() == Some(project) {
            self.environment.notice = Some(
                "Saved project setup changed. Reload saved settings before changing it.".into(),
            );
            if !self
                .pending
                .values()
                .any(|pending| pending.kind.starts_with("secret-"))
            {
                self.load_workload_secrets(project.into(), cx);
            }
            cx.notify();
        }
    }
    pub(crate) fn record_installation_command(
        &mut self,
        project: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(project) =
            project.filter(|project| self.projects.iter().any(|p| p.project_id == *project))
        {
            if self.environment.project.as_ref() != Some(&project) {
                self.select_environment(project, cx);
            }
        }
        self.environment.recording = true;
        cx.notify();
    }
    fn append_tested_command(&mut self, cx: &mut Context<Self>) {
        if self
            .environment
            .version
            .as_ref()
            .is_none_or(|version| version.source == EnvironmentSource::Repository)
        {
            return;
        }
        let command = self.environment.record_command.read(cx).value(cx);
        if command.trim().is_empty() || command.len() > 32768 || command.contains('\0') {
            self.environment.error = Some("Enter an installation command, up to 32 KiB.".into());
            cx.notify();
            return;
        }
        let current = self.environment.initialize.read(cx).value(cx);
        let next = if current.trim().is_empty() {
            command.trim().to_owned()
        } else {
            format!("{}\n{}", current.trim_end(), command.trim())
        };
        self.environment
            .initialize
            .update(cx, |input, cx| input.set_value(next, cx));
        self.environment
            .record_command
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.environment.recording = false;
        self.environment.error = None;
        self.environment.notice = Some(
            "Command added to Initialize. Save environment to apply it to future computers.".into(),
        );
        cx.notify();
    }
    pub(super) fn bind_environment(&mut self, cx: &mut Context<Self>) {
        if self
            .environment
            .project
            .as_ref()
            .is_some_and(|id| self.projects.iter().any(|p| &p.project_id == id))
        {
            return;
        }
        self.environment.reset(cx);
        if let Some(id) = self.projects.first().map(|p| p.project_id.clone()) {
            self.select_environment(id, cx);
        }
    }
    fn select_environment(&mut self, id: String, cx: &mut Context<Self>) {
        let recording = self.environment.recording;
        self.environment.reset(cx);
        self.environment.recording = recording;
        self.environment.project = Some(id.clone());
        self.load_workload_secrets(id.clone(), cx);
        self.call(
            "environment-read",
            methods::CLOUD_PROJECT_ENVIRONMENT,
            json!({"projectId":id}),
            cx,
        );
        cx.notify();
    }
    pub(super) fn receive_environment(&mut self, data: Value, cx: &mut Context<Self>) {
        match serde_json::from_value::<EnvironmentVersion>(data) {
            Ok(version) if Some(&version.project_id) == self.environment.project.as_ref() => {
                self.environment.initialize.update(cx, |input, cx| {
                    input.set_value(
                        version
                            .recipe
                            .scripts
                            .initialize
                            .clone()
                            .unwrap_or_default(),
                        cx,
                    )
                });
                self.environment.refresh.update(cx, |input, cx| {
                    input.set_value(
                        version.recipe.scripts.refresh.clone().unwrap_or_default(),
                        cx,
                    )
                });
                self.environment.startup = version.recipe.startup.clone();
                self.environment.version = Some(version);
                self.environment.error = None;
            }
            Ok(_) => {}
            Err(_) => {
                self.environment.error =
                    Some("Environment setup could not be verified. Reload before saving.".into())
            }
        }
        cx.notify();
    }
    fn add_startup_service(&mut self, cx: &mut Context<Self>) {
        let name = self
            .environment
            .service_name
            .read(cx)
            .value(cx)
            .trim()
            .to_owned();
        let command = self.environment.service_command.read(cx).value(cx);
        let port = self.environment.service_port.read(cx).value(cx);
        let port = if port.trim().is_empty() {
            None
        } else {
            match port.trim().parse::<u16>() {
                Ok(p) if p > 0 => Some(p),
                _ => {
                    self.environment.error = Some("Enter a port between 1 and 65535.".into());
                    cx.notify();
                    return;
                }
            }
        };
        let mut recipe = EnvironmentRecipe::default();
        recipe.startup = self.environment.startup.clone();
        recipe.startup.push(StartupService {
            name,
            command,
            port,
            timeout_seconds: 60,
        });
        if recipe.validate().is_err() {
            self.environment.error = Some(
                "Use a unique service name, a command and at most 16 startup services.".into(),
            );
            cx.notify();
            return;
        }
        self.environment.startup = recipe.startup;
        self.environment.error = None;
        for input in [
            &self.environment.service_name,
            &self.environment.service_command,
            &self.environment.service_port,
        ] {
            input.update(cx, |input, cx| input.set_value(String::new(), cx));
        }
        cx.notify();
    }
    fn save_environment(&mut self, cx: &mut Context<Self>) {
        let Some(version) = self.environment.version.as_ref() else {
            return;
        };
        if version.source == EnvironmentSource::Repository
            || self
                .pending
                .values()
                .any(|p| p.kind.starts_with("environment"))
        {
            return;
        }
        let project = version.project_id.clone();
        let revision = version.revision;
        let mut recipe = version.recipe.clone();
        let initialize = self.environment.initialize.read(cx).value(cx);
        let refresh = self.environment.refresh.read(cx).value(cx);
        recipe.scripts = SetupCommands {
            initialize: (!initialize.trim().is_empty()).then_some(initialize),
            refresh: (!refresh.trim().is_empty()).then_some(refresh),
        };
        recipe.startup = self.environment.startup.clone();
        if recipe.validate().is_err() {
            self.environment.error = Some("Check the setup commands and startup services.".into());
            cx.notify();
            return;
        }
        self.environment.error = None;
        self.environment.notice = None;
        self.call(
            "environment-save",
            methods::CLOUD_PROJECT_ENVIRONMENT_SAVE,
            json!({"projectId":project,"expectedRevision":revision,"recipe":recipe}),
            cx,
        );
        cx.notify();
    }
    pub(super) fn render_environment(&self, cx: &mut Context<Self>) -> Div {
        let mut body = column().gap(px(10.)).p(px(12.));
        if self.projects.is_empty() {
            body=body.child(s::description_text("Choose a repository first. You can then add software, dependency installation and startup services."));
        } else {
            if self.projects.len() > 1 {
                let mut projects = row().flex_wrap().gap(px(6.));
                for project in &self.projects {
                    let id = project.project_id.clone();
                    let selected = self.environment.project.as_ref() == Some(&id);
                    projects = projects.child(
                        cs::button(
                            SharedString::from(format!("environment-{id}")),
                            project.display_name.clone(),
                            selected,
                            false,
                            false,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_environment(id.clone(), cx)
                        })),
                    );
                }
                body = body.child(projects);
            }
            let busy = self
                .pending
                .values()
                .any(|p| p.kind.starts_with("environment"));
            if let Some(version) = &self.environment.version {
                if version.source == EnvironmentSource::Repository {
                    body=body.child(s::description_text("Managed by .zuse/setup.json. Edit that file in the repository to change setup."));
                } else {
                    if self.environment.recording {
                        body=body.child(s::description_text("Record a tested installation command for future computers. Use secret bindings for credentials."))
                            .child(self.environment.record_command.clone())
                            .child(cs::button("record-cloud-command","Add to setup",false,busy,false).on_click(cx.listener(|this,_,_,cx|this.append_tested_command(cx))));
                    } else {
                        body = body.child(
                            cs::button(
                                "open-record-cloud-command",
                                "Record installation command…",
                                false,
                                busy,
                                false,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.environment.recording = true;
                                cx.notify();
                            })),
                        );
                    }
                    body = body
                        .child(s::description_text(
                            "Initialize · installs software and prepares a fresh computer",
                        ))
                        .child(self.environment.initialize.clone())
                        .child(s::description_text(
                            "Refresh dependencies · runs when preparation inputs change",
                        ))
                        .child(self.environment.refresh.clone());
                    body = body.child(s::description_text(
                        "Startup services · restart when a computer boots",
                    ));
                    for (index, service) in self.environment.startup.iter().enumerate() {
                        body = body.child(cs::setting_row(
                            index == 0,
                            service.name.clone(),
                            service.command.clone(),
                            Some(
                                cs::button(
                                    SharedString::from(format!("remove-startup-{index}")),
                                    "Remove",
                                    false,
                                    busy,
                                    false,
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if index < this.environment.startup.len() {
                                        this.environment.startup.remove(index);
                                    }
                                    cx.notify();
                                }))
                                .into_any_element(),
                            ),
                        ));
                    }
                    body = body
                        .child(
                            row()
                                .gap(px(8.))
                                .child(div().flex_1().child(self.environment.service_name.clone()))
                                .child(
                                    div()
                                        .w(px(120.))
                                        .child(self.environment.service_port.clone()),
                                ),
                        )
                        .child(self.environment.service_command.clone())
                        .child(
                            cs::button("add-startup-service", "Add service", false, busy, false)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.add_startup_service(cx)),
                                ),
                        )
                        .child(
                            cs::button(
                                "save-cloud-environment",
                                "Save environment",
                                true,
                                busy,
                                busy,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save_environment(cx))),
                        );
                }
                body = body.child(s::description_text(format!(
                    "Environment version {} · Changes apply to new computers, not running tasks.",
                    version.revision
                )));
            } else if busy {
                body = body.child(s::description_text("Loading environment setup…"));
            }
            if let Some(id) = self.environment.project.clone() {
                body = body.child(
                    cs::button(
                        "reload-cloud-environment",
                        "Reload saved settings",
                        false,
                        busy,
                        false,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_environment(id.clone(), cx)),
                    ),
                );
            }
        }
        if self.environment.project.is_some() {
            body = body.child(self.render_workload_secrets(cx));
        }
        if let Some(notice) = &self.environment.notice {
            body = body.child(s::description_text(notice.clone()));
        }
        if let Some(error) = &self.environment.error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(s::destructive_text())
                    .child(error.clone()),
            );
        }
        cs::group(
            "Environment",
            Some("Optional software and services. Zuse prepares each task computer automatically."),
            None,
            body,
        )
    }
}
