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
mod form;
mod github;
mod image_status;
mod images;
mod machine;
mod onboarding;
mod overview;
mod plan;
mod presentation;
mod repository_palette;
mod secrets;
pub use image_status::ImageReadiness;
pub use machine::StartSetupChat;
/// What a running image build is doing, for cards outside Settings.
pub fn phase_label(phase: &str) -> &'static str {
    machine::phase_copy(phase)
}
/// How far along a running image build is, by phase.
pub fn phase_fraction(phase: &str) -> f32 {
    machine::build_fraction(phase)
}
pub use onboarding::{FirstChatRepository, StartChat};
gpui::actions!(
    cloud_setup_overlay,
    [CloseRepositoryPicker, CloseImageHistory, SaveChanges]
);
pub type Owner = (String, Option<String>);
/// Delay before the `attempt`th fallback status read while the account feed
/// is disconnected: doubles from `first`, capped at a minute. While the feed
/// is connected, `machine-image` hints replace these reads entirely.
pub(crate) fn fallback_delay(first: std::time::Duration, attempt: u32) -> std::time::Duration {
    first
        .saturating_mul(1u32 << attempt.min(6))
        .min(std::time::Duration::from_secs(60))
}
/// Which part of Cloud setup the panel renders. `All` stacks every group, as
/// the browser's single Cloud page does; the desktop gives each its own page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CloudPage {
    #[default]
    All,
    Overview,
    Repositories,
    Environments,
    Machine,
    Agents,
}
/// Asks the host to show another Cloud page (Overview rows link to them).
#[derive(Clone, Copy, Debug)]
pub struct OpenPage(pub CloudPage);
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
    machine: machine::MachineView,
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
    /// Code from the OAuth browser that confirms a fresh GitHub identity.
    github_code: Entity<TextInput>,
    error: Option<String>,
    notice: Option<String>,
    picker: bool,
    /// The open overflow ("…") menu, by its trigger id.
    overflow: Option<SharedString>,
    /// The open menu's trigger and items, for keyboard focus.
    menu_trigger: FocusHandle,
    menu_focus: Vec<FocusHandle>,
    /// The window's size at the last render, to place the unsaved bar.
    viewport: Size<Pixels>,
    usage_only: bool,
    storage_usage: Option<i64>,
    picker_focus: FocusHandle,
    picker_trigger: FocusHandle,
    /// The palette's highlighted entry, and whether to scroll it into view.
    palette_active: Option<repository_palette::Item>,
    palette_reveal: bool,
    palette_scroll: ScrollHandle,
    search: Entity<TextInput>,
    selected: BTreeSet<String>,
    queue: Vec<GithubRepository>,
    /// Repositories being added, by lowercase name, shown before they land.
    adding: BTreeSet<String>,
    /// Every repository the last GitHub catalog listed (lowercase); `None`
    /// until the first arrives. Later arrivals are announced.
    known_repos: Option<BTreeSet<String>>,
    /// Repositories that arrived while the panel was open, marked "new".
    fresh_repos: BTreeSet<String>,
    toast: Option<SharedString>,
    toast_timer: Option<Task<()>>,
    keys: BTreeMap<String, String>,
    onboarding: onboarding::Onboarding,
    /// The repository the first chat starts on, by project id.
    first_chat_project: Option<String>,
    /// The debounced look at whether the image should build by itself.
    image_check: Option<Task<()>>,
    /// The account state of the last automatic build request, so one the
    /// server drops is never requested again in a loop.
    image_auto: Option<(Owner, i64, usize)>,
    /// The host's settled answer to "is this account subscribed?"; `None`
    /// until it knows, when the panel's own entitlement read decides.
    host_subscribed: Option<bool>,
    /// The plan from the account's entitlements, and its billing summary.
    plan_tier: Option<(plan::Tier, bool)>,
    plan_read: bool,
    plan_summary: Option<Value>,
    page: CloudPage,
}
impl EventEmitter<Request> for RepositoryPanel {}
impl EventEmitter<OpenPage> for RepositoryPanel {}
impl EventEmitter<StartChat> for RepositoryPanel {}
impl EventEmitter<machine::StartSetupChat> for RepositoryPanel {}
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
            KeyBinding::new("secondary-s", SaveChanges, Some("CloudSettingsPanel")),
        ]);
        let search = cx.new(|cx| {
            TextInput::new("Search repositories", window, cx).with_single_line_metrics(13., 18.)
        });
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        let github_code = cx.new(|cx| TextInput::new("Code from your browser", window, cx));
        cx.observe(&github_code, |_, _, cx| cx.notify()).detach();
        Self {
            agents: agents::AgentView::new(window, cx),
            environment: environment::EnvironmentView::new(window, cx),
            machine: machine::MachineView::new(window, cx),
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
            github_code,
            error: None,
            notice: None,
            picker: false,
            overflow: None,
            menu_trigger: cx.focus_handle(),
            menu_focus: (0..8).map(|_| cx.focus_handle()).collect(),
            viewport: Size::default(),
            usage_only: false,
            storage_usage: None,
            picker_focus: cx.focus_handle(),
            picker_trigger: cx.focus_handle(),
            palette_active: None,
            palette_reveal: false,
            palette_scroll: ScrollHandle::new(),
            search,
            selected: BTreeSet::new(),
            queue: vec![],
            adding: BTreeSet::new(),
            known_repos: None,
            fresh_repos: BTreeSet::new(),
            toast: None,
            toast_timer: None,
            keys: BTreeMap::new(),
            onboarding: Default::default(),
            first_chat_project: None,
            image_check: None,
            image_auto: None,
            host_subscribed: None,
            plan_tier: None,
            plan_read: false,
            plan_summary: None,
            page: CloudPage::All,
        }
    }
    /// Cmd/Ctrl+S: save whichever form on this page has edits.
    fn save_changes(&mut self, cx: &mut Context<Self>) {
        if !self.can_manage() {
            return;
        }
        if self.machine_dirty(cx) {
            self.save_machine(cx);
        } else if self.environment_dirty(cx) {
            self.save_environment(cx);
        }
    }
    pub fn set_page(&mut self, page: CloudPage, cx: &mut Context<Self>) {
        if self.page != page {
            self.page = page;
            self.picker = false;
            cx.notify();
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
        self.machine.reset(cx);
        self.secrets.reset(cx);
        self.storage_usage = None;
        self.onboarding = Default::default();
        self.first_chat_project = None;
        self.image_check = None;
        self.image_auto = None;
        self.plan_tier = None;
        self.plan_read = false;
        self.plan_summary = None;
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
        self.overflow = None;
        self.palette_active = None;
        self.selected.clear();
        self.queue.clear();
        self.adding.clear();
        self.known_repos = None;
        self.fresh_repos.clear();
        self.toast = None;
        self.toast_timer = None;
        self.keys.clear();
        self.search
            .update(cx, |s, cx| s.set_value(String::new(), cx));
        // A code typed for one account's GitHub operation never carries over.
        self.github_code
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
        self.load_machine(cx);
        self.refresh_image(cx);
        self.call(
            "plan-entitlements",
            methods::CLOUD_BILLING_ENTITLEMENTS,
            json!({}),
            cx,
        );
        self.call(
            "plan-summary",
            methods::CLOUD_BILLING_SUMMARY,
            json!({}),
            cx,
        );
        cx.notify();
    }
    /// The host's settled subscription answer (`None` while it is still
    /// checking). Onboarding only ever appears for subscribed accounts.
    pub fn set_cloud_subscribed(&mut self, subscribed: Option<bool>, cx: &mut Context<Self>) {
        if self.host_subscribed != subscribed {
            self.host_subscribed = subscribed;
            cx.notify();
        }
    }
    fn cloud_subscribed(&self) -> bool {
        self.host_subscribed
            .unwrap_or(self.plan_read && self.plan_tier.is_some())
    }
    /// Whether the first-run onboarding fills the Overview page right now.
    pub fn onboarding_active(&self) -> bool {
        matches!(self.page, CloudPage::Overview | CloudPage::All)
            && !self.agent_recovery_open()
            && self.onboarding_visible()
    }
    pub(crate) fn plan_glance(&self) -> Option<plan::PlanGlance> {
        let (tier, trial) = self.plan_tier?;
        Some(plan::PlanGlance::new(
            tier,
            trial,
            self.plan_summary.as_ref(),
        ))
    }
    fn receive_plan(&mut self, kind: &str, result: Result<Value, String>, owner: &Owner) {
        // Plan reads only decorate onboarding and the Machine page; a failed
        // read never surfaces as a page error.
        let Ok(value) = result else {
            if kind == "plan-entitlements" {
                self.plan_read = true;
            }
            return;
        };
        if value["accountId"].as_str() != Some(owner.0.as_str())
            || value["organizationId"].as_str() != owner.1.as_deref()
        {
            return;
        }
        if kind == "plan-entitlements" {
            self.plan_read = true;
            self.plan_tier =
                plan::tier_from_entitlements(&value["data"], chrono::Utc::now().timestamp_millis());
        } else {
            self.plan_summary = Some(value["data"].clone());
        }
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
        if pending.kind.starts_with("plan-") {
            self.receive_plan(pending.kind, result, &pending.owner);
            cx.notify();
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
                if pending.kind.starts_with("machine") {
                    self.machine.error = Some(e);
                    cx.notify();
                    return;
                }
                if pending.kind.starts_with("agent") {
                    self.agents.request_failed(pending.kind);
                    self.agents.error = Some(e);
                    self.agents.poll = None;
                    cx.notify();
                    return;
                }
                self.error = Some(e);
                self.queue.clear();
                self.adding.clear();
                // A failed add doesn't start a setup chat later on its own.
                self.machine.setup_after_add = false;
                self.machine.setup_repository = None;
                cx.notify();
                return;
            }
        };
        if value["accountId"].as_str() != Some(pending.owner.0.as_str())
            || value["organizationId"].as_str() != pending.owner.1.as_deref()
        {
            self.error = Some("Your account changed. Refresh repositories.".into());
            self.queue.clear();
            self.adding.clear();
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
        if pending.kind.starts_with("machine") {
            self.receive_machine(data, cx);
            // Automatic requests say nothing; the image row shows progress.
            if matches!(pending.kind, "machine-init" | "machine-auto") {
                self.machine.notice = None;
            }
            return;
        }
        if pending.kind.starts_with("agent") {
            self.receive_agents(pending.kind, data, cx);
            return;
        }
        match pending.kind {
            "github-install" => self.github_started(data, cx),
            "github-status" => self.github_status(data, cx),
            "github-select" | "github-confirm" | "github-cancel" | "github-disconnect" => {
                self.github_operation = None;
                self.github_poll = None;
                self.github_code
                    .update(cx, |input, cx| input.set_value(String::new(), cx));
                self.notice = Some(
                    if matches!(pending.kind, "github-select" | "github-confirm") {
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
                    self.schedule_image_check(cx);
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
                    Ok(github) => {
                        self.note_catalog(&github, cx);
                        self.github = Some(github);
                    }
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
                        let identity = project
                            .repository_identity
                            .replace("github.com/", "")
                            .to_lowercase();
                        self.keys.remove(&identity);
                        self.adding.remove(&identity);
                        self.projects.retain(|p| p.project_id != project.project_id);
                        self.notice = Some("Repository added.".into());
                        if self.machine.setup_after_add && self.machine.setup_repository.is_none() {
                            // The first one picked is added first.
                            self.machine.setup_repository =
                                Some(machine::StartSetupChat::on(&project));
                        }
                        self.projects.push(project);
                        self.connect_next(cx);
                        // Start once every picked repository is in.
                        if self.machine.setup_after_add
                            && self.queue.is_empty()
                            && !self.pending.values().any(|p| p.kind == "connect")
                            && let Some(start) = self.machine.setup_repository.take()
                        {
                            self.machine.setup_after_add = false;
                            cx.emit(start);
                        }
                    }
                    self.bind_environment(cx);
                    // The server rebuilds the image for the new repository
                    // set; refresh its status once the last one lands.
                    if self.queue.is_empty()
                        && !self
                            .pending
                            .values()
                            .any(|p| p.kind == "connect" || p.kind.starts_with("machine"))
                    {
                        self.load_machine(cx);
                    }
                }
                Err(_) => {
                    self.error = Some(
                        "The repository result could not be verified. Refresh before retrying."
                            .into(),
                    );
                    self.queue.clear();
                    self.adding.clear();
                }
            },
            _ => {}
        }
        cx.notify();
    }
    pub fn interrupted(&mut self, cx: &mut Context<Self>) {
        self.pending.clear();
        self.queue.clear();
        self.adding.clear();
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
                    | "github-confirm"
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
#[cfg(test)]
mod owner_reset_tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn switching_owner_clears_a_typed_github_confirmation_code(cx: &mut TestAppContext) {
        cx.update(crate::initialize);
        let window = cx.add_window(RepositoryPanel::new);
        window
            .update(cx, |panel, _, cx| {
                panel.bind_owner(Some(("account-a".into(), None)), cx);
                panel
                    .github_code
                    .update(cx, |input, cx| input.set_value("ABCD-EFGH".into(), cx));
                panel.bind_owner(Some(("account-b".into(), None)), cx);
                assert!(panel.github_code.read(cx).value(cx).is_empty());
            })
            .unwrap();
    }
    #[gpui::test]
    fn configure_with_agent_starts_a_setup_chat_on_the_first_repository(cx: &mut TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(crate::initialize);
        let window = cx.add_window(RepositoryPanel::new);
        let panel = window.root(cx).unwrap();
        let started = Rc::new(RefCell::new(Vec::<String>::new()));
        let opened = Rc::new(RefCell::new(0));
        cx.update(|cx| {
            let started = started.clone();
            cx.subscribe(&panel, move |_, event: &StartSetupChat, _| {
                started
                    .borrow_mut()
                    .push(event.repository.project_id.clone())
            })
            .detach();
            let opened = opened.clone();
            cx.subscribe(&panel, move |_, _: &OpenPage, _| *opened.borrow_mut() += 1)
                .detach();
        });
        window
            .update(cx, |panel, window, cx| {
                panel.bind_owner(Some(("account-a".into(), None)), cx);
                // No repository and no GitHub yet: connect one first.
                panel.configure_with_agent(window, cx);
                assert!(!panel.machine.setup_after_add);
                panel.projects = serde_json::from_value(json!([
                    {"projectId":"project_1","repositoryIdentity":"github.com/a/one","repositoryUrl":"https://github.com/a/one.git","displayName":"one","defaultBranch":"main","visibility":"private","state":"connected","activeBuilds":{},"latestBuilds":{},"createdAt":1,"updatedAt":1},
                    {"projectId":"project_2","repositoryIdentity":"github.com/a/two","repositoryUrl":"https://github.com/a/two.git","displayName":"two","defaultBranch":"main","visibility":"private","state":"connected","activeBuilds":{},"latestBuilds":{},"createdAt":2,"updatedAt":2}
                ]))
                .unwrap();
                panel.configure_with_agent(window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(*opened.borrow(), 1);
        assert_eq!(*started.borrow(), ["project_1"]);

        // With no repository, the chat starts on the first one picked, once
        // every picked repository has been added.
        let project = |id: &str, n: i64| json!({"accountId":"account-a","organizationId":null,"data":{"projectId":id,"repositoryIdentity":format!("github.com/a/{id}"),"repositoryUrl":"https://github.com/a/x.git","displayName":id,"defaultBranch":"main","visibility":"private","state":"connected","activeBuilds":{},"latestBuilds":{},"createdAt":n,"updatedAt":n}});
        window
            .update(cx, |panel, _, cx| {
                panel.projects.clear();
                panel.machine.setup_after_add = true;
                panel.call("connect", methods::CLOUD_PROJECT_CONNECT, json!({}), cx);
                let first = panel.next;
                panel.call("connect", methods::CLOUD_PROJECT_CONNECT, json!({}), cx);
                let second = panel.next;
                panel.receive(first, Ok(project("picked_first", 3)), cx);
                panel.receive(second, Ok(project("picked_second", 4)), cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(*started.borrow(), ["project_1", "picked_first"]);
    }
    #[test]
    fn fallback_reads_back_off_to_a_minute() {
        let waits: Vec<_> = (0..7)
            .map(|n| fallback_delay(std::time::Duration::from_secs(2), n).as_secs())
            .collect();
        assert_eq!(waits, vec![2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(
            fallback_delay(std::time::Duration::from_secs(10), 40).as_secs(),
            60
        );
    }
    #[gpui::test]
    fn a_connected_feed_replaces_machine_build_polling_with_hinted_reads(cx: &mut TestAppContext) {
        cx.update(crate::initialize);
        let window = cx.add_window(RepositoryPanel::new);
        window
            .update(cx, |panel, _, cx| {
                panel.bind_owner(Some(("account".into(), None)), cx);
                panel.loaded = true;
                let building = |panel: &mut RepositoryPanel, cx: &mut Context<RepositoryPanel>| {
                    panel.receive_machine(
                        json!({"revision":1,"recipe":{"setupScript":"true"},"createdAt":1,"imageState":"building"}),
                        cx,
                    );
                };
                let reads = |panel: &RepositoryPanel| {
                    let mut kinds: Vec<_> = panel.pending.values().map(|p| p.kind).collect();
                    kinds.sort();
                    kinds
                };
                // Disconnected: a running build is polled.
                building(panel, cx);
                assert!(panel.machine.poll.is_some());
                // Connecting stops the timer and reads once, for missed hints.
                panel.set_account_feed_live(true, cx);
                assert!(panel.machine.poll.is_none());
                assert_eq!(reads(panel), vec!["image", "machine-read"]);
                panel.pending.clear();
                // While connected a running build schedules no timer…
                building(panel, cx);
                assert!(panel.machine.poll.is_none());
                assert!(reads(panel).is_empty());
                // …and other kinds don't read it either.
                panel.account_event("conversation", "w", cx);
                assert!(reads(panel).is_empty());
                // A hint reads both views once.
                panel.account_event("machine-image", "uimg", cx);
                assert_eq!(reads(panel), vec!["image", "machine-read"]);
                panel.pending.clear();
                // Losing the feed brings the fallback poll back.
                panel.set_account_feed_live(false, cx);
                assert!(panel.machine.poll.is_some());
            })
            .unwrap();
    }
}
