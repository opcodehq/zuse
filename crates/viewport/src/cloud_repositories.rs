//! One repository settings component shared by desktop and browser.
use crate::{
    assets::icon,
    cloud_settings::style as cs,
    input::TextInput,
    palette, settings_style as s,
    shell::{BORDER, MUTED, TEXT},
    shell::{column, row},
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use zuse_protocol::{
    cloud_projects::{CloudProject, GithubCatalog, GithubRepository, ProjectList},
    methods,
};
mod agent_login_binding;
mod agent_recovery;
mod agents;
mod environment;
mod github;
mod guide;
mod images;
mod presentation;
mod secrets;
gpui::actions!(
    cloud_setup_overlay,
    [CloseRepositoryPicker, CloseImageHistory]
);
pub type Owner = (String, Option<String>);
#[derive(Clone)]
pub enum Request {
    Call {
        id: u64,
        method: &'static str,
        params: Value,
    },
    Cancel {
        id: u64,
    },
}
struct Pending {
    kind: &'static str,
    owner: Owner,
}
pub struct RepositoryPanel {
    agents: agents::AgentView,
    environment: environment::EnvironmentView,
    secrets: secrets::SecretsView,
    image: images::ImageView,
    owner: Option<Owner>,
    account_feed_live: bool,
    next: u64,
    pending: BTreeMap<u64, Pending>,
    loaded: bool,
    projects: Vec<CloudProject>,
    github: Option<GithubCatalog>,
    github_operation: Option<Value>,
    github_native: bool,
    github_connection_state: String,
    github_disconnect_confirm: bool,
    github_poll: Option<Task<()>>,
    error: Option<String>,
    notice: Option<String>,
    picker: bool,
    usage_only: bool,
    storage_usage: Option<i64>,
    picker_focus: FocusHandle,
    picker_trigger: FocusHandle,
    picker_refresh: FocusHandle,
    picker_add: FocusHandle,
    choice_focus: BTreeMap<String, FocusHandle>,
    search: Entity<TextInput>,
    selected: BTreeSet<String>,
    queue: Vec<GithubRepository>,
    keys: BTreeMap<String, String>,
    guide: guide::GuideState,
}
impl EventEmitter<Request> for RepositoryPanel {}
impl RepositoryPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        images::bind_keys(cx);
        cx.bind_keys([
            KeyBinding::new(
                "escape",
                CloseRepositoryPicker,
                Some("CloudRepositoryPicker"),
            ),
            KeyBinding::new("escape", CloseImageHistory, Some("CloudImageHistory")),
        ]);
        let search = cx.new(|cx| TextInput::new("Search repositories…", window, cx));
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        Self {
            agents: agents::AgentView::new(window, cx),
            environment: environment::EnvironmentView::new(window, cx),
            secrets: secrets::SecretsView::new(window, cx),
            image: Default::default(),
            owner: None,
            account_feed_live: false,
            next: 0,
            pending: BTreeMap::new(),
            loaded: false,
            projects: vec![],
            github: None,
            github_operation: None,
            github_native: false,
            github_connection_state: String::new(),
            github_disconnect_confirm: false,
            github_poll: None,
            error: None,
            notice: None,
            picker: false,
            usage_only: false,
            storage_usage: None,
            picker_focus: cx.focus_handle(),
            picker_trigger: cx.focus_handle(),
            picker_refresh: cx.focus_handle(),
            picker_add: cx.focus_handle(),
            choice_focus: BTreeMap::new(),
            search,
            selected: BTreeSet::new(),
            queue: vec![],
            keys: BTreeMap::new(),
            guide: Default::default(),
        }
    }
    pub fn bind_owner(&mut self, owner: Option<Owner>, cx: &mut Context<Self>) {
        if self.owner == owner {
            return;
        }
        for id in self.pending.keys() {
            cx.emit(Request::Cancel { id: *id });
        }
        self.pending.clear();
        self.image = Default::default();
        self.agents.reset(cx);
        self.environment.reset(cx);
        self.secrets.reset(cx);
        self.storage_usage = None;
        self.owner = owner;
        self.loaded = false;
        self.projects.clear();
        self.github = None;
        self.github_operation = None;
        self.github_native = false;
        self.github_connection_state.clear();
        self.github_disconnect_confirm = false;
        self.github_poll = None;
        self.error = None;
        self.notice = None;
        self.picker = false;
        self.choice_focus.clear();
        self.selected.clear();
        self.queue.clear();
        self.keys.clear();
        self.search
            .update(cx, |s, cx| s.set_value(String::new(), cx));
        cx.notify();
    }
    pub fn set_usage(&mut self, usage: bool, cost: Option<i64>, cx: &mut Context<Self>) -> bool {
        if self.storage_usage != cost {
            self.storage_usage = cost;
            cx.notify();
        }
        if self.usage_only != usage {
            self.usage_only = usage;
            if usage && self.loaded {
                self.refresh_image(cx);
            }
            cx.notify();
        }
        if usage && !self.loaded && self.owner.is_some() {
            self.refresh(cx);
        }
        self.image.has_storage()
    }
    fn call(
        &mut self,
        kind: &'static str,
        method: &'static str,
        mut params: Value,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self.owner.clone() else {
            return;
        };
        params["expectedAccountId"] = json!(owner.0);
        params["expectedOrganizationId"] = json!(owner.1);
        self.next += 1;
        let id = self.next;
        self.pending.insert(id, Pending { kind, owner });
        cx.emit(Request::Call { id, method, params });
    }
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.repository_busy() || self.owner.is_none() {
            return;
        }
        self.loaded = true;
        self.error = None;
        self.call("list", methods::CLOUD_PROJECTS, json!({}), cx);
        self.call("github", methods::CLOUD_GITHUB, json!({}), cx);
        self.call("agents", methods::CLOUD_AGENT_CONNECTIONS, json!({}), cx);
        self.refresh_image(cx);
        cx.notify();
    }
    pub fn receive(&mut self, id: u64, result: Result<Value, String>, cx: &mut Context<Self>) {
        let Some(pending) = self.pending.remove(&id) else {
            return;
        };
        if self.owner.as_ref() != Some(&pending.owner) {
            return;
        }
        if matches!(pending.kind, "image" | "image-build" | "image-delete") {
            self.receive_image(pending, result, cx);
            return;
        }
        let value = match result {
            Ok(v) => v,
            Err(e) => {
                if pending.kind.starts_with("secret-") {
                    self.secrets.error = Some(e);
                    cx.notify();
                    return;
                }
                if pending.kind.starts_with("environment") {
                    self.environment.error = Some(e);
                    cx.notify();
                    return;
                }
                if pending.kind.starts_with("agent") {
                    self.agents.error = Some(e);
                    self.agents.poll = None;
                    cx.notify();
                    return;
                }
                self.error = Some(e);
                self.queue.clear();
                cx.notify();
                return;
            }
        };
        if value["accountId"].as_str() != Some(pending.owner.0.as_str())
            || value["organizationId"].as_str() != pending.owner.1.as_deref()
        {
            self.error = Some("Your account changed. Refresh repositories.".into());
            self.queue.clear();
            cx.notify();
            return;
        }
        let data = value["data"].clone();
        if pending.kind.starts_with("secret-") {
            self.receive_workload_secrets(pending.kind, data, cx);
            return;
        }
        if pending.kind.starts_with("environment") {
            self.receive_environment(data, cx);
            return;
        }
        if pending.kind.starts_with("agent") {
            self.receive_agents(pending.kind, data, cx);
            return;
        }
        match pending.kind {
            "github-install" => self.github_started(data, cx),
            "github-status" => self.github_status(data, cx),
            "github-select" | "github-cancel" | "github-disconnect" => {
                self.github_operation = None;
                self.github_poll = None;
                self.notice = Some(
                    if pending.kind == "github-select" {
                        "GitHub connected. Choose your repositories below."
                    } else if pending.kind == "github-disconnect" {
                        "GitHub disconnected."
                    } else {
                        "GitHub connection cancelled."
                    }
                    .into(),
                );
                self.refresh(cx);
            }
            "list" => match serde_json::from_value::<ProjectList>(data) {
                Ok(list) => {
                    self.projects = list.projects;
                    self.bind_environment(cx);
                }
                Err(_) => {
                    self.error =
                        Some("Repository list is unavailable. Refresh and try again.".into())
                }
            },
            "github" => {
                self.github_native = data["nativeAuthority"] == true;
                self.github_connection_state =
                    data["connectionState"].as_str().unwrap_or("").to_owned();
                self.github_disconnect_confirm = false;
                match serde_json::from_value::<GithubCatalog>(data) {
                    Ok(github) => self.github = Some(github),
                    Err(_) => {
                        self.error = Some(
                            "GitHub repository list is unavailable. Refresh and try again.".into(),
                        )
                    }
                }
            }
            "connect" | "remove" => match serde_json::from_value::<CloudProject>(data) {
                Ok(project) => {
                    self.refresh_image(cx);
                    if pending.kind == "remove" {
                        self.projects.retain(|p| p.project_id != project.project_id);
                        self.notice =
                            Some("Repository removed. Existing chats are preserved.".into());
                    } else {
                        self.keys.remove(
                            &project
                                .repository_identity
                                .replace("github.com/", "")
                                .to_lowercase(),
                        );
                        self.projects.retain(|p| p.project_id != project.project_id);
                        self.projects.push(project);
                        self.notice = Some("Repository added.".into());
                        self.connect_next(cx);
                    }
                    self.bind_environment(cx);
                }
                Err(_) => {
                    self.error = Some(
                        "The repository result could not be verified. Refresh before retrying."
                            .into(),
                    );
                    self.queue.clear();
                }
            },
            _ => {}
        }
        cx.notify();
    }
    pub fn interrupted(&mut self, cx: &mut Context<Self>) {
        self.pending.clear();
        self.queue.clear();
        self.image.interrupted();
        self.error =
            Some("Account connection interrupted. Refresh to check repository changes.".into());
        cx.notify();
    }
    fn repository_busy(&self) -> bool {
        self.pending.values().any(|p| {
            matches!(
                p.kind,
                "list"
                    | "github"
                    | "github-install"
                    | "github-select"
                    | "github-cancel"
                    | "github-disconnect"
                    | "connect"
                    | "remove"
            )
        })
    }
    fn can_manage(&self) -> bool {
        self.owner.as_ref().is_some_and(|(_, org)| {
            org.is_none()
                || self
                    .github
                    .as_ref()
                    .and_then(|g| g.can_manage_installations)
                    == Some(true)
        })
    }
    fn connect_next(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.queue.pop() else {
            return;
        };
        let key = self
            .keys
            .entry(repo.name_with_owner.to_lowercase())
            .or_insert_with(|| uuid::Uuid::new_v4().to_string())
            .clone();
        self.call("connect",methods::CLOUD_PROJECT_CONNECT,json!({"repositoryUrl":repo.https_url,"defaultBranch":repo.default_branch,"visibility":if repo.is_private{"private"}else{"public"},"idempotencyKey":key}),cx);
    }
}
