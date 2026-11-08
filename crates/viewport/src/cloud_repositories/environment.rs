//! Project setup is versioned independently from any running computer or image cache.
use super::*;
use zuse_protocol::cloud_computers::*;
mod view;

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
    /// The recipe as last loaded or saved, for change tracking.
    saved: Option<EnvironmentRecipe>,
    adding_service: bool,
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
    /// Plain variables in the editable recipe (`None` until loaded).
    pub(super) fn variables(&self) -> Option<&BTreeMap<String, String>> {
        self.version.as_ref().map(|v| &v.recipe.environment)
    }
    /// Sets or removes a plain variable; saved with the rest of the recipe.
    pub(super) fn set_variable(&mut self, name: &str, value: Option<String>) -> bool {
        let Some(version) = self
            .version
            .as_mut()
            .filter(|v| v.source != EnvironmentSource::Repository)
        else {
            return false;
        };
        match value {
            Some(value) => version.recipe.environment.insert(name.to_owned(), value),
            None => version.recipe.environment.remove(name),
        };
        true
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
        let initialize =
            cx.new(|cx| TextInput::script("sudo apt-get install -y jq\npnpm install", window, cx));
        let record_command = cx.new(|cx| {
            TextInput::script(
                "Paste a command you've run successfully on this computer",
                window,
                cx,
            )
        });
        let refresh = cx.new(|cx| TextInput::script("pnpm install --frozen-lockfile", window, cx));
        let service_name = cx.new(|cx| TextInput::new("Name, such as web", window, cx));
        let service_command = cx.new(|cx| TextInput::new("Command, such as pnpm dev", window, cx));
        let service_port = cx.new(|cx| TextInput::new("Port", window, cx));
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
            saved: None,
            adding_service: false,
            error: None,
            notice: None,
        }
    }
    pub fn reset(&mut self, cx: &mut Context<RepositoryPanel>) {
        self.project = None;
        self.version = None;
        self.startup.clear();
        self.saved = None;
        self.adding_service = false;
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
    pub(super) fn append_tested_command(&mut self, cx: &mut Context<Self>) {
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
        self.environment.notice = Some("Added to Install. Save to use it on new computers.".into());
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
    pub(super) fn select_environment(&mut self, id: String, cx: &mut Context<Self>) {
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
                self.environment.saved = Some(version.recipe.clone());
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
    pub(super) fn add_startup_service(&mut self, cx: &mut Context<Self>) {
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
        self.environment.adding_service = false;
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
    pub(super) fn save_environment(&mut self, cx: &mut Context<Self>) {
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
    /// Whether the editors differ from the saved recipe.
    pub(super) fn environment_dirty(&self, cx: &App) -> bool {
        let (Some(saved), Some(version)) = (&self.environment.saved, &self.environment.version)
        else {
            return false;
        };
        let text = |input: &Entity<TextInput>| input.read(cx).value(cx);
        let same = |value: String, saved: &Option<String>| {
            value.trim() == saved.as_deref().unwrap_or("").trim()
        };
        !same(
            text(&self.environment.initialize),
            &saved.scripts.initialize,
        ) || !same(text(&self.environment.refresh), &saved.scripts.refresh)
            || self.environment.startup != saved.startup
            || version.recipe.secret_bindings != saved.secret_bindings
            || version.recipe.environment != saved.environment
    }
    pub(super) fn discard_environment(&mut self, cx: &mut Context<Self>) {
        let Some(saved) = self.environment.saved.clone() else {
            return;
        };
        self.environment.initialize.update(cx, |input, cx| {
            input.set_value(saved.scripts.initialize.clone().unwrap_or_default(), cx)
        });
        self.environment.refresh.update(cx, |input, cx| {
            input.set_value(saved.scripts.refresh.clone().unwrap_or_default(), cx)
        });
        self.environment.startup = saved.startup.clone();
        if let Some(version) = &mut self.environment.version {
            version.recipe.secret_bindings = saved.secret_bindings.clone();
            version.recipe.environment = saved.environment.clone();
        }
        self.environment.adding_service = false;
        self.environment.recording = false;
        self.environment.error = None;
        self.environment.notice = None;
        cx.notify();
    }
}
