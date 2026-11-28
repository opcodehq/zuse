//! Scoped read-only sidebar metadata. Saved rows are provisional, never compute authority.
use super::*;
use futures::StreamExt;
use serde_json::{Value, json};
#[derive(Clone, Debug, PartialEq, Eq)]
struct Scope {
    account: String,
    organization: Option<String>,
}
impl Scope {
    fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            account: value["user"]["id"].as_str()?.to_owned(),
            organization: value["orgId"].as_str().map(str::to_owned),
        })
    }
    fn params(&self) -> Value {
        json!({"expectedAccountId":self.account,"expectedOrganizationId":self.organization})
    }
    fn matches(&self, state: &AppState) -> bool {
        state.cloud_user_id() == Some(self.account.as_str())
            && state.cloud_organization_id() == self.organization.as_deref()
    }
    fn accepts(&self, value: &Value) -> bool {
        value["accountId"].as_str() == Some(self.account.as_str())
            && value["organizationId"].as_str() == self.organization.as_deref()
    }
    fn owner(&self) -> String {
        self.organization
            .as_ref()
            .map(|v| format!("organization:{v}"))
            .unwrap_or_else(|| self.account.clone())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Projects,
    Workspaces,
}
impl Part {
    fn method(self) -> &'static str {
        match self {
            Self::Projects => methods::CLOUD_PROJECTS,
            Self::Workspaces => methods::CLOUD_WORKSPACES,
        }
    }
}
#[derive(Default)]
pub(super) struct Catalog {
    scope: Option<Scope>,
    origin: Option<String>,
    projects: Vec<Value>,
    workspaces: Vec<Value>,
    projects_fresh: bool,
    workspaces_fresh: bool,
    denied: bool,
    loading: bool,
    error: Option<String>,
    refresh: Arc<tokio::sync::Notify>,
    /// Bumped whenever a fresh workspace list lands (after a `workspace`
    /// account event) or a task is archived here, so views of archived
    /// tasks reload without polling.
    workspaces_revision: u64,
    /// Tasks archived from this sidebar, by workspace id. They stay out of
    /// every list (saved, fresh, or refreshed by a hint) until a fresh list
    /// shows the server recorded the archive, so a list read before the
    /// archive landed cannot put the row back.
    pending_archive: HashMap<String, PendingArchive>,
    /// Chats this app just created, by workspace id, until a fresh list has
    /// them. The account list is read on events; one read before the create
    /// landed (or no event at all) must not leave a new chat out of the
    /// sidebar after its pending row is gone.
    created: Vec<CreatedWorkspace>,
    /// Each cloud chat's live activity, by workspace id: what its open
    /// conversations (in front, in a tab, or still running in the
    /// background) report, and whether a turn finished out of sight. The
    /// sidebar and tabs read it for every chat, whichever one is open.
    activity: HashMap<String, ChatActivity>,
}
/// What the sidebar knows live about one cloud chat.
#[derive(Default)]
struct ChatActivity {
    /// Each open conversation's report, by its view.
    live: HashMap<u64, CloudLiveActivity>,
    /// A turn ended while none of its conversations was in front.
    unseen: bool,
}
/// One open cloud conversation's live report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CloudLiveActivity {
    /// Its agent is mid-turn.
    pub working: bool,
    /// Its agent is waiting on the user.
    pub awaiting_input: bool,
    /// It sees its computer up.
    pub online: bool,
}
/// A cloud chat's status for its sidebar row and tab, across every open
/// conversation of its computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CloudChatActivity {
    /// Live reports, combined; `None` when no conversation of it is open.
    pub live: Option<CloudLiveActivity>,
    /// A turn finished while it was not in front.
    pub unseen: bool,
}
/// A chat the server confirmed creating, shown until a fresh list has it.
struct CreatedWorkspace {
    row: Value,
    at: std::time::Instant,
}
/// How long a created chat stays listed while fresh lists still omit it.
/// Lists normally catch up within one read; past this the server's list
/// wins (the chat was deleted elsewhere, say).
const CREATED_CONFIRM_WINDOW: std::time::Duration = std::time::Duration::from_secs(120);
/// A task hidden while its archive is in flight or not yet listed.
struct PendingArchive {
    /// The row as it was listed, restored if the archive fails.
    row: Value,
    /// When the server acknowledged the archive. Until then the row is
    /// always hidden; afterwards for at most [`ARCHIVE_CONFIRM_WINDOW`].
    acknowledged: Option<std::time::Instant>,
}
/// How long an acknowledged archive keeps hiding a row that fresh lists still
/// show as active. Lists normally catch up within seconds; past this the
/// server's list wins.
const ARCHIVE_CONFIRM_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);
fn archived_row(row: &Value) -> bool {
    matches!(
        row["state"].as_str(),
        Some("deleted" | "archived" | "archiving")
    ) || matches!(row["desiredState"].as_str(), Some("deleted" | "archived"))
}
fn access_denied(error: &RpcError) -> bool {
    matches!(error,RpcError::Capability(code) if code=="workspace_access_denied"||code=="workspace_not_found")
        || matches!(error,RpcError::Failed(message) if matches!(message.as_str(),"You do not have access to this workspace."|"Cloud could not verify your session. Reconnect in Settings → General."))
}
/// What each project is called in the sidebar and the pickers.
#[derive(Default, Debug)]
pub(crate) struct ProjectLabels {
    /// By local project key (`view::project_key`).
    pub local: HashMap<String, String>,
    /// By cloud project id; a project linked to a local checkout shares
    /// that checkout's label.
    pub cloud: HashMap<String, String>,
}
impl AppState {
    /// Labels for every local project and `cloud_projects`: short names,
    /// with `owner/repo` only where two repositories share a name.
    pub(crate) fn project_labels(&self, cloud_projects: &[Value]) -> ProjectLabels {
        use zuse_protocol::presentation::{ProjectLabelInput, cloud_project_group, project_labels};
        enum Entry {
            Local(String),
            Cloud(String),
        }
        let mut entries = Vec::new();
        let mut inputs = Vec::new();
        let mut aliases = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for space in &self.spaces {
            let key = zuse_protocol::view::project_key(space);
            if !seen.insert(key.clone()) {
                continue;
            }
            let representative = self.representative_space(space);
            inputs.push(ProjectLabelInput {
                name: representative.display_name().to_owned(),
                repository: representative
                    .remote_repository
                    .clone()
                    .or_else(|| representative.repository_id.clone()),
            });
            entries.push(Entry::Local(key));
        }
        for project in cloud_projects {
            let Some(group) = cloud_project_group(project, &self.spaces) else {
                continue;
            };
            if group.local_space.is_some() {
                aliases.push((group.project_id, group.key));
                continue;
            }
            inputs.push(ProjectLabelInput {
                name: group.name,
                repository: project["repositoryIdentity"].as_str().map(str::to_owned),
            });
            entries.push(Entry::Cloud(group.project_id));
        }
        let mut labels = ProjectLabels::default();
        for (entry, label) in entries.into_iter().zip(project_labels(&inputs)) {
            match entry {
                Entry::Local(key) => labels.local.insert(key, label),
                Entry::Cloud(id) => labels.cloud.insert(id, label),
            };
        }
        for (id, key) in aliases {
            if let Some(label) = labels.local.get(&key).cloned() {
                labels.cloud.insert(id, label);
            }
        }
        labels
    }
    pub(crate) fn cloud_sidebar_projects(&self) -> Vec<Value> {
        if self
            .cloud_sidebar
            .scope
            .as_ref()
            .is_some_and(|s| s.matches(self))
        {
            self.cloud_sidebar.projects.clone()
        } else {
            vec![]
        }
    }
    pub(crate) fn cloud_sidebar_workspaces(&self) -> Vec<Value> {
        if self
            .cloud_sidebar
            .scope
            .as_ref()
            .is_some_and(|s| s.matches(self))
        {
            let mut rows = self.cloud_sidebar.workspaces.clone();
            rows.extend(self.unlisted_created().cloned());
            rows
        } else {
            vec![]
        }
    }
    /// Created chats the current list does not have yet.
    fn unlisted_created(&self) -> impl Iterator<Item = &Value> {
        let catalog = &self.cloud_sidebar;
        catalog
            .created
            .iter()
            .map(|created| &created.row)
            .filter(|row| {
                !catalog
                    .workspaces
                    .iter()
                    .any(|listed| listed["workspaceId"] == row["workspaceId"])
                    && !row["workspaceId"]
                        .as_str()
                        .is_some_and(|id| catalog.pending_archive.contains_key(id))
            })
    }
    /// The server confirmed creating a chat: list it now, from the create
    /// response and its launch request, and read the account list again so
    /// the server's own row replaces it. It stays until a fresh list has it.
    pub(crate) fn note_created_cloud_workspace(
        &mut self,
        workspace: &Value,
        launch: &Value,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = workspace["workspaceId"]
            .as_str()
            .filter(|id| !id.is_empty())
        else {
            return;
        };
        if self.cloud_sidebar.scope.is_none() {
            return;
        }
        let now = chrono::Utc::now().timestamp_millis();
        let mut row = workspace.clone();
        let fill = |row: &mut Value, key: &str, value: Value| {
            if row[key].is_null() || row[key].as_str().is_some_and(|text| text.trim().is_empty()) {
                row[key] = value;
            }
        };
        fill(&mut row, "projectId", launch["projectId"].clone());
        fill(&mut row, "title", launch["firstMessage"].clone());
        fill(&mut row, "agent", launch["agent"].clone());
        fill(&mut row, "state", json!("provisioning"));
        fill(&mut row, "createdAt", json!(now));
        fill(&mut row, "updatedAt", json!(now));
        self.cloud_sidebar
            .created
            .retain(|created| created.row["workspaceId"].as_str() != Some(id));
        self.cloud_sidebar.created.push(CreatedWorkspace {
            row,
            at: std::time::Instant::now(),
        });
        self.retry_cloud_sidebar(cx);
    }
    /// The listed task row for `workspace`, without cloning the catalog.
    pub(crate) fn cloud_sidebar_workspace(&self, workspace: &str) -> Option<&Value> {
        self.cloud_sidebar
            .scope
            .as_ref()
            .is_some_and(|s| s.matches(self))
            .then(|| {
                self.cloud_sidebar
                    .workspaces
                    .iter()
                    .find(|row| row["workspaceId"].as_str() == Some(workspace))
                    .or_else(|| {
                        self.unlisted_created()
                            .find(|row| row["workspaceId"].as_str() == Some(workspace))
                    })
            })
            .flatten()
    }
    pub(crate) fn set_cloud_sidebar_title(
        &mut self,
        target: &CloudTarget,
        workspace: &Value,
        cx: &mut Context<Self>,
    ) {
        if self.cloud_user_id() != Some(target.subject.as_str())
            || self.cloud_organization_id() != target.organization.as_deref()
        {
            return;
        }
        for row in self.cloud_sidebar.workspaces.iter_mut().chain(
            self.cloud_sidebar
                .created
                .iter_mut()
                .map(|created| &mut created.row),
        ) {
            if row["workspaceId"].as_str() == Some(target.workspace.as_str())
                && row["revision"].as_i64() <= workspace["revision"].as_i64()
            {
                row["title"] = workspace["title"].clone();
                row["renamedTitle"] = workspace["renamedTitle"].clone();
                row["revision"] = workspace["revision"].clone();
            }
        }
        cx.notify();
    }
    /// The sidebar refreshes silently; tests still observe its phases.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn cloud_sidebar_loading(&self) -> bool {
        self.cloud_sidebar.loading
    }
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn cloud_sidebar_stale(&self) -> bool {
        !self.cloud_sidebar.projects_fresh || !self.cloud_sidebar.workspaces_fresh
    }
    pub(crate) fn cloud_sidebar_workspaces_revision(&self) -> u64 {
        self.cloud_sidebar.workspaces_revision
    }
    pub(crate) fn cloud_sidebar_error(&self) -> Option<&str> {
        self.cloud_sidebar.error.as_deref()
    }
    pub(crate) fn retry_cloud_sidebar(&mut self, cx: &mut Context<Self>) {
        self.cloud_sidebar.refresh.notify_one();
        self.cloud_sidebar.loading = true;
        cx.notify();
    }
    /// Archiving `workspace` starts: drop its row at once and keep it out of
    /// every list until the server confirms. False when it is not listed.
    pub(crate) fn begin_cloud_sidebar_archive(&mut self, workspace: &str) -> bool {
        let listed = self
            .cloud_sidebar
            .workspaces
            .iter()
            .position(|row| row["workspaceId"].as_str() == Some(workspace));
        let row = match listed {
            Some(index) => self.cloud_sidebar.workspaces.remove(index),
            // A chat created here that no list has yet. Its created record
            // stays (hidden while the archive is pending), so a failed
            // archive brings it back with its original retention.
            None => {
                let Some(created) = self
                    .cloud_sidebar
                    .created
                    .iter()
                    .find(|created| created.row["workspaceId"].as_str() == Some(workspace))
                else {
                    return false;
                };
                created.row.clone()
            }
        };
        self.cloud_sidebar.pending_archive.insert(
            workspace.to_owned(),
            PendingArchive {
                row,
                acknowledged: None,
            },
        );
        self.cloud_sidebar.workspaces_revision =
            self.cloud_sidebar.workspaces_revision.wrapping_add(1);
        true
    }
    /// The server accepted the archive. The row stays hidden until a fresh
    /// list no longer shows it active; a fresh list is asked for right away.
    pub(crate) fn acknowledge_cloud_sidebar_archive(
        &mut self,
        workspace: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(pending) = self.cloud_sidebar.pending_archive.get_mut(workspace) {
            pending.acknowledged = Some(std::time::Instant::now());
        }
        // Archived for good: a created record must not bring it back.
        self.cloud_sidebar
            .created
            .retain(|created| created.row["workspaceId"].as_str() != Some(workspace));
        self.cloud_sidebar.workspaces_revision =
            self.cloud_sidebar.workspaces_revision.wrapping_add(1);
        self.retry_cloud_sidebar(cx);
    }
    /// The archive failed: put the row back unless a list already has it.
    pub(crate) fn fail_cloud_sidebar_archive(&mut self, workspace: &str) -> bool {
        let Some(pending) = self.cloud_sidebar.pending_archive.remove(workspace) else {
            return false;
        };
        // A created chat no list has yet shows again from its created
        // record, which kept its retention.
        let created = self
            .cloud_sidebar
            .created
            .iter()
            .any(|created| created.row["workspaceId"].as_str() == Some(workspace));
        if !created
            && !self
                .cloud_sidebar
                .workspaces
                .iter()
                .any(|row| row["workspaceId"].as_str() == Some(workspace))
        {
            self.cloud_sidebar.workspaces.push(pending.row);
        }
        self.cloud_sidebar.workspaces_revision =
            self.cloud_sidebar.workspaces_revision.wrapping_add(1);
        true
    }
    #[cfg(test)]
    pub(crate) fn cloud_sidebar_archive_pending(&self, workspace: &str) -> bool {
        self.cloud_sidebar.pending_archive.contains_key(workspace)
    }
    /// One open conversation (`view`) of `workspace` reports its activity,
    /// or `None` once it closes. A turn that ends while that conversation is
    /// not in front leaves the chat unseen. Returns whether the chat's
    /// status changed.
    pub(crate) fn report_cloud_chat_activity(
        &mut self,
        workspace: &str,
        view: u64,
        report: Option<CloudLiveActivity>,
        in_front: bool,
    ) -> bool {
        let before = self.cloud_chat_activity(workspace);
        let entry = self
            .cloud_sidebar
            .activity
            .entry(workspace.to_owned())
            .or_default();
        let previous = match report {
            Some(report) => entry.live.insert(view, report),
            None => entry.live.remove(&view),
        };
        let was_busy = previous.is_some_and(|live| live.working || live.awaiting_input);
        let busy = report.is_some_and(|live| live.working || live.awaiting_input);
        if was_busy && !busy && !in_front {
            entry.unseen = true;
        }
        if entry.live.is_empty() && !entry.unseen {
            self.cloud_sidebar.activity.remove(workspace);
        }
        self.cloud_chat_activity(workspace) != before
    }
    /// The chat was opened in front: a finished turn is seen.
    pub(crate) fn mark_cloud_chat_seen(&mut self, workspace: &str) -> bool {
        let Some(entry) = self.cloud_sidebar.activity.get_mut(workspace) else {
            return false;
        };
        let changed = std::mem::take(&mut entry.unseen);
        if entry.live.is_empty() {
            self.cloud_sidebar.activity.remove(workspace);
        }
        changed
    }
    /// The live status of `workspace`'s chat, for its sidebar row and tab.
    pub(crate) fn cloud_chat_activity(&self, workspace: &str) -> CloudChatActivity {
        let Some(entry) = self.cloud_sidebar.activity.get(workspace) else {
            return CloudChatActivity::default();
        };
        CloudChatActivity {
            live: entry
                .live
                .values()
                .copied()
                .reduce(|all, one| CloudLiveActivity {
                    working: all.working || one.working,
                    awaiting_input: all.awaiting_input || one.awaiting_input,
                    online: all.online || one.online,
                }),
            unseen: entry.unseen,
        }
    }
    pub(super) fn clear_cloud_sidebar(&mut self) {
        self.cloud_sidebar = Catalog::default();
    }
    fn sidebar_error(&mut self, scope: &Scope, part: Part, error: &RpcError) {
        if !scope.matches(self) {
            return;
        }
        match part {
            Part::Projects => self.cloud_sidebar.projects_fresh = false,
            Part::Workspaces => self.cloud_sidebar.workspaces_fresh = false,
        }
        self.cloud_sidebar.error = Some(
            if access_denied(error) {
                "Cloud access changed. Reconnect in Settings → General."
            } else {
                "Showing saved cloud items. Reconnecting…"
            }
            .into(),
        );
        if access_denied(error) {
            self.cloud_sidebar.projects.clear();
            self.cloud_sidebar.workspaces.clear();
            self.cloud_sidebar.created.clear();
            self.cloud_sidebar.denied = true;
            self.cloud_sidebar.projects_fresh = false;
            self.cloud_sidebar.workspaces_fresh = false;
        }
    }
    fn apply_sidebar_part(&mut self, part: Part, value: &Value) -> bool {
        let Some(scope) = self.cloud_account.as_ref().and_then(Scope::from_value) else {
            return false;
        };
        if !scope.accepts(value) || self.cloud_sidebar.denied {
            return false;
        }
        let Some(origin) = value["apiOrigin"].as_str().filter(|v| !v.is_empty()) else {
            return false;
        };
        let provisional = value["provisional"] == true;
        if provisional
            && match part {
                Part::Projects => self.cloud_sidebar.projects_fresh,
                Part::Workspaces => self.cloud_sidebar.workspaces_fresh,
            }
        {
            return false;
        }
        if self
            .cloud_sidebar
            .origin
            .as_deref()
            .is_some_and(|v| v != origin)
        {
            if provisional
                && (self.cloud_sidebar.projects_fresh || self.cloud_sidebar.workspaces_fresh)
            {
                return false;
            }
            let refresh = self.cloud_sidebar.refresh.clone();
            self.clear_cloud_sidebar();
            self.cloud_sidebar.refresh = refresh;
        }
        self.cloud_sidebar.scope = Some(scope.clone());
        self.cloud_sidebar.origin = Some(origin.into());
        if provisional && value["cacheMiss"] == true {
            return true;
        }
        let rows = match part {
            Part::Projects => value["data"]["projects"]
                .as_array()
                .or_else(|| value["projects"].as_array()),
            Part::Workspaces => value["workspaces"].as_array(),
        };
        let Some(rows) = rows else {
            return false;
        };
        match part {
            Part::Projects => {
                let owner = scope.owner();
                self.cloud_sidebar.projects = rows
                    .iter()
                    .cloned()
                    .map(|mut row| {
                        row["accountId"] = json!(owner);
                        if row["providerId"].is_null() || row["providerId"] == "box" {
                            row["providerId"] = json!("boxd");
                        }
                        if provisional {
                            row["canCreate"] = json!(false);
                            row["provisional"] = json!(true);
                        }
                        row
                    })
                    .collect();
                self.cloud_sidebar.projects_fresh = !provisional;
            }
            Part::Workspaces => {
                // A fresh list confirms an acknowledged archive once it no
                // longer lists the task as active; one still unconfirmed past
                // its window yields to the server's list.
                if !provisional {
                    self.cloud_sidebar
                        .pending_archive
                        .retain(|workspace, pending| {
                            let Some(acknowledged) = pending.acknowledged else {
                                return true;
                            };
                            let active = rows.iter().any(|row| {
                                row["workspaceId"].as_str() == Some(workspace.as_str())
                                    && !archived_row(row)
                            });
                            active && acknowledged.elapsed() < ARCHIVE_CONFIRM_WINDOW
                        });
                }
                // A fresh list that has a created chat takes over its row;
                // one still missing it keeps the row for a while.
                if !provisional {
                    self.cloud_sidebar.created.retain(|created| {
                        let listed = rows
                            .iter()
                            .any(|row| row["workspaceId"] == created.row["workspaceId"]);
                        !listed && created.at.elapsed() < CREATED_CONFIRM_WINDOW
                    });
                }
                let pending = &self.cloud_sidebar.pending_archive;
                self.cloud_sidebar.workspaces = rows
                    .iter()
                    .filter(|row| {
                        !archived_row(row)
                            && !row["workspaceId"]
                                .as_str()
                                .is_some_and(|id| pending.contains_key(id))
                    })
                    .cloned()
                    .collect();
                self.cloud_sidebar.workspaces_fresh = !provisional;
                if !provisional {
                    self.cloud_sidebar.workspaces_revision =
                        self.cloud_sidebar.workspaces_revision.wrapping_add(1);
                }
            }
        }
        if !provisional && self.cloud_sidebar.projects_fresh && self.cloud_sidebar.workspaces_fresh
        {
            self.cloud_sidebar.error = None;
        }
        true
    }
    // Compatibility reducer for fixture/seed callers; the live pump uses independent parts.
    pub(crate) fn apply_cloud_sidebar_catalog(&mut self, context: &Value, list: &Value) -> bool {
        let Some(scope) = self.cloud_account.as_ref().and_then(Scope::from_value) else {
            return false;
        };
        if !scope.accepts(context) || !scope.accepts(list) || context["ownerId"] != scope.owner() {
            return false;
        }
        let mut context = context.clone();
        let mut list = list.clone();
        let origin = context["apiOrigin"]
            .as_str()
            .or_else(|| list["apiOrigin"].as_str())
            .unwrap_or("fixture://cloud-sidebar")
            .to_owned();
        context["apiOrigin"] = json!(origin);
        list["apiOrigin"] = json!(origin);
        self.apply_sidebar_part(Part::Projects, &context)
            | self.apply_sidebar_part(Part::Workspaces, &list)
    }
}
async fn request(
    handle: EngineHandle,
    scope: Scope,
    part: Part,
    provisional: bool,
    executor: gpui::BackgroundExecutor,
) -> (Part, bool, Result<Value, RpcError>) {
    let mut params = scope.params();
    if provisional {
        params["cacheOnly"] = json!(true);
    }
    let read = handle.client().call_scoped(part.method(), params);
    futures::pin_mut!(read);
    let result = tokio::select! { result=&mut read=>result, _=executor.timer(std::time::Duration::from_secs(30))=>Err(RpcError::Failed("Cloud sidebar request timed out.".into())) };
    (part, provisional, result)
}
/// How often the sidebar's workspace list is read again without an event.
const SIDEBAR_REVALIDATE: std::time::Duration = std::time::Duration::from_secs(60);
/// What is left of the wait for the interval read due at `due`.
fn revalidate_wait(due: std::time::Instant, now: std::time::Instant) -> std::time::Duration {
    due.saturating_duration_since(now)
}
/// Account event kinds that can change the sidebar's projects or workspaces.
/// Project rows change through `environment` events; there is no separate
/// project or catalog kind.
fn refreshes_catalog(kind: &str) -> bool {
    matches!(
        kind,
        "workspace" | "environment" | "github" | "connection" | "authentication"
    )
}
pub(super) fn spawn(cx: &mut Context<AppState>, handle: EngineHandle) -> Task<()> {
    // Cloud computers are a beta; without it there is nothing to list.
    if !crate::cloud_beta() {
        return Task::ready(());
    }
    // A replacement controller must not inherit another API's visible catalog.
    cx.spawn(async move |this,cx| {
        if this.update(cx,|state,_|state.clear_cloud_sidebar()).is_err(){return;}
        'reconnect: loop {
            let mut auth=match handle.client().subscribe_scoped(methods::CLOUD_ACCOUNT_STATUS,json!({})).await{Ok(v)=>v,Err(_)=>{cx.background_executor().timer(std::time::Duration::from_secs(2)).await;continue;}};
            let mut pending=None;
            'account: loop {
                let value=match pending.take(){Some(v)=>v,None=>match auth.recv().await{Some(v)=>v,None=>continue 'reconnect}};
                let scope=Scope::from_value(&value);
                if this.update(cx,|state,cx|{state.apply_cloud_account(value);if state.cloud_sidebar.scope!=scope{state.cloud_sidebar=Catalog{scope:scope.clone(),..Default::default()};}cx.notify();}).is_err(){return;}
                let Some(scope)=scope else{continue;};
                let mut events:Option<zuse_rpc::RpcSubscription>=None;
                let mut initial=true;let mut load_projects=true;let mut retry_seconds=2;
                loop {
                    if this.update(cx,|state,cx|{if scope.matches(state){state.cloud_sidebar.loading=true;state.cloud_sidebar.denied=false;cx.notify();}}).is_err(){return;}
                    let mut reads=futures::stream::FuturesUnordered::new();
                    for part in [Part::Projects,Part::Workspaces]{
                        if part==Part::Projects&&!load_projects{continue;}
                        reads.push(request(handle.clone(),scope.clone(),part,false,cx.background_executor().clone()));
                        if initial{reads.push(request(handle.clone(),scope.clone(),part,true,cx.background_executor().clone()));}
                    }
                    initial=false;let mut retry=false;let mut retry_projects=false;
                    while !reads.is_empty(){
                        tokio::select!{biased;
                            value=auth.recv()=>{let Some(value)=value else{continue 'reconnect;};if Scope::from_value(&value).as_ref()!=Some(&scope){pending=Some(value);continue 'account;}},
                            value=reads.next()=>{if let Some((part,provisional,result))=value{
                                if let Err(ref error)=result{if !provisional&&!access_denied(error){retry=true;retry_projects|=part==Part::Projects;}}
                                if this.update(cx,|state,cx|{if !scope.matches(state){return;}match result{Ok(value)=>{if state.apply_sidebar_part(part,&value){cx.notify();}},Err(error)if !provisional=>{state.sidebar_error(&scope,part,&error);cx.notify();},_=>{}}}).is_err(){return;}
                            }}
                        }
                    }
                    if this.update(cx,|state,cx|{if scope.matches(state){state.cloud_sidebar.loading=false;cx.notify();}}).is_err(){return;}
                    let delay=retry_seconds;retry_seconds=if retry{(retry_seconds*2).min(30)}else{2};
                    // The interval read's deadline is set once per read: a
                    // failed or timed-out event connection, or an event that
                    // needs no read, never postpones it.
                    let revalidate_at=std::time::Instant::now()+SIDEBAR_REVALIDATE;
                    loop {
                        let manual=match this.update(cx,|state,_|state.cloud_sidebar.refresh.clone()){Ok(value)=>value,Err(_)=>return};
                        let executor=cx.background_executor().clone();
                        let needs_connect=events.is_none();
                        let connect_executor=executor.clone();
                        let connect=async{
                            if !needs_connect{futures::future::pending::<()>().await;}
                            connect_executor.timer(std::time::Duration::from_secs(2)).await;
                            let open=handle.client().subscribe_scoped(methods::CLOUD_EVENTS,scope.params());futures::pin_mut!(open);
                            tokio::select!{value=&mut open=>value.ok(),_=connect_executor.timer(std::time::Duration::from_secs(30))=>None}
                        };
                        let event=async{match &mut events{Some(v)=>v.recv().await,None=>futures::future::pending::<Option<Value>>().await}};
                        let retry_executor=cx.background_executor().clone();
                        let retry_wait=async{if retry{retry_executor.timer(std::time::Duration::from_secs(delay)).await;}else{futures::future::pending::<()>().await;}};
                        // A missed account event must not leave rows stale
                        // (a sleeping computer shown running, a new chat
                        // missing): the list is read again at an interval too.
                        let revalidate_executor=cx.background_executor().clone();
                        let revalidate=async{revalidate_executor.timer(revalidate_wait(revalidate_at,std::time::Instant::now())).await;};
                        tokio::select!{biased;
                            value=auth.recv()=>{let Some(value)=value else{continue 'reconnect;};if Scope::from_value(&value).as_ref()!=Some(&scope){pending=Some(value);continue 'account;}},
                            _=manual.notified()=>{load_projects=true;retry_seconds=2;break;},
                            _=retry_wait=>{load_projects=retry_projects;break;},
                            _=revalidate=>{load_projects=false;break;},
                            connection=connect=>{if let Some(connection)=connection{events=Some(connection);}continue;},
                            value=event=>{
                                let Some(value)=value else{events=None;continue;};
                                if !scope.accepts(&value){continue;}
                                let data=&value["data"];
                                let authority_denied=data["code"]=="cloud_event_access_denied"||(data["cacheRevoked"]==true && data["type"]!="reset");
                                let disconnected=matches!(data["type"].as_str(),Some("error"|"closed"));
                                if this.update(cx,|state,cx|{if !scope.matches(state){return;}if let Some(revoked)=data["revokedWorkspaceIds"].as_array(){state.cloud_sidebar.workspaces.retain(|row|!revoked.iter().any(|id|row["workspaceId"]==*id));}if authority_denied{state.sidebar_error(&scope,Part::Projects,&RpcError::Capability("workspace_access_denied".into()));}else if disconnected{state.sidebar_error(&scope,Part::Projects,&RpcError::Transport("Cloud events disconnected".into()));}cx.notify();}).is_err(){return;}
                                if disconnected&&!authority_denied{events=None;retry=true;retry_projects=true;continue;}
                                let refresh=matches!(data["type"].as_str(),Some("connected"|"reset"|"revalidate"))||data["events"].as_array().into_iter().flatten().any(|e|e["kind"].as_str().is_some_and(refreshes_catalog));
                                if refresh{load_projects=true;break;}
                            }
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Each pass of the event loop waits only for what is left until the
    /// interval read: passes that end without a read (a failed connection,
    /// an event needing none) never restart the 60 seconds.
    #[test]
    fn sidebar_interval_read_keeps_its_deadline_across_passes() {
        let start = std::time::Instant::now();
        let due = start + SIDEBAR_REVALIDATE;
        let mut now = start;
        let mut waited = std::time::Duration::ZERO;
        // An unrelated event every 5 s, or a failed connection every 2 s.
        for step in [5, 2, 5, 2, 5, 45] {
            let wait = revalidate_wait(due, now);
            assert!(wait <= SIDEBAR_REVALIDATE - waited);
            let step = std::time::Duration::from_secs(step);
            now += step;
            waited += step;
        }
        assert_eq!(
            revalidate_wait(due, start + SIDEBAR_REVALIDATE),
            std::time::Duration::ZERO
        );
        assert_eq!(
            revalidate_wait(due, now),
            std::time::Duration::ZERO,
            "past due: read now"
        );
    }
    #[test]
    fn sidebar_refreshes_only_on_kinds_the_server_sends() {
        for kind in [
            "workspace",
            "environment",
            "github",
            "connection",
            "authentication",
        ] {
            assert!(refreshes_catalog(kind), "{kind}");
        }
        // Hints for other views, and kinds no server has ever sent.
        for kind in [
            "billing",
            "machine-image",
            "conversation",
            "task-command-progress",
            "project",
            "image-catalog",
            "project-catalog",
        ] {
            assert!(!refreshes_catalog(kind), "{kind}");
        }
    }
    #[test]
    fn cloud_only_catalog_needs_no_local_chat_and_scope_changes_clear_it() {
        let mut state = AppState::new();
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        let context = json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[{"projectId":"cloud-project","repositoryIdentity":"github.com/o/app"}]});
        let list = json!({"accountId":"owner","organizationId":null,"workspaces":[{"workspaceId":"task","projectId":"cloud-project","state":"paused"},{"workspaceId":"deleted","state":"deleted"}]});
        assert!(state.apply_cloud_sidebar_catalog(&context, &list));
        assert!(state.chats.is_empty() && state.spaces.is_empty());
        assert_eq!(state.cloud_sidebar_projects().len(), 1);
        assert_eq!(state.cloud_sidebar_workspaces().len(), 1);
        assert_eq!(state.cloud_sidebar_projects()[0]["accountId"], "owner");
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":"another-org"}));
        assert!(state.cloud_sidebar_projects().is_empty());
        assert!(state.cloud_sidebar_workspaces().is_empty());
        assert!(!state.apply_cloud_sidebar_catalog(&context, &list));
    }
    /// A chat created here keeps its created-row retention through a failed
    /// archive, and leaves with everything else when access is denied.
    #[gpui::test]
    fn created_rows_survive_a_failed_archive_and_go_with_denied_access(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext as _;
        let context = json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[{"projectId":"cloud-project","repositoryIdentity":"github.com/o/app"}]});
        let empty = json!({"accountId":"owner","organizationId":null,"workspaces":[]});
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            assert!(state.apply_cloud_sidebar_catalog(&context, &empty));
            state
        });
        state.update(cx, |state, cx| {
            state.note_created_cloud_workspace(
                &json!({"workspaceId":"fresh","initialSessionId":"fresh-session"}),
                &json!({"projectId":"cloud-project","firstMessage":"hi"}),
                cx,
            );
            assert!(state.cloud_sidebar_workspace("fresh").is_some());
            // Archive, then it fails: the row is back as a created row...
            assert!(state.begin_cloud_sidebar_archive("fresh"));
            assert!(state.cloud_sidebar_workspace("fresh").is_none());
            assert!(state.fail_cloud_sidebar_archive("fresh"));
            assert_eq!(state.cloud_sidebar_workspaces().len(), 1);
            // ...so a list that doesn't have it yet still keeps it.
            assert!(state.apply_cloud_sidebar_catalog(&context, &empty));
            assert!(
                state.cloud_sidebar_workspace("fresh").is_some(),
                "a failed archive keeps the created row's retention"
            );
            // A successful archive does not bring it back.
            assert!(state.begin_cloud_sidebar_archive("fresh"));
            state.acknowledge_cloud_sidebar_archive("fresh", cx);
            assert!(state.apply_cloud_sidebar_catalog(&context, &empty));
            assert!(state.cloud_sidebar_workspace("fresh").is_none());
            // Access denied: a created row goes with the rest.
            state.note_created_cloud_workspace(
                &json!({"workspaceId":"other","initialSessionId":"other-session"}),
                &json!({"projectId":"cloud-project","firstMessage":"hi"}),
                cx,
            );
            assert!(state.cloud_sidebar_workspace("other").is_some());
            let scope = Scope::from_value(&json!({"user":{"id":"owner"},"orgId":null})).unwrap();
            state.sidebar_error(
                &scope,
                Part::Workspaces,
                &RpcError::Capability("workspace_access_denied".into()),
            );
            assert!(state.cloud_sidebar_workspace("other").is_none());
            assert!(state.cloud_sidebar_workspaces().is_empty());
        });
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use gpui::AppContext;
    use std::sync::{Arc, Mutex};
    fn projects(provisional: bool, name: &str) -> Value {
        json!({"accountId":"owner","organizationId":null,"apiOrigin":"https://cloud.test","provisional":provisional,"data":{"projects":[{"projectId":name,"repositoryIdentity":"github.com/o/p","canCreate":true}]}})
    }
    fn workspaces(provisional: bool) -> Value {
        json!({"accountId":"owner","organizationId":null,"apiOrigin":"https://cloud.test","provisional":provisional,"workspaces":[{"workspaceId":"saved","state":"paused","initialSessionId":"session"}]})
    }
    #[test]
    fn independent_projects_survive_workspace_failure_and_late_cache_cannot_replace_fresh() {
        let mut state = AppState::new();
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        assert!(state.apply_sidebar_part(Part::Projects, &projects(true, "cached")));
        assert_eq!(state.cloud_sidebar_projects()[0]["canCreate"], false);
        let scope = Scope::from_value(state.cloud_account.as_ref().unwrap()).unwrap();
        state.sidebar_error(
            &scope,
            Part::Workspaces,
            &RpcError::Transport("offline".into()),
        );
        assert_eq!(state.cloud_sidebar_projects()[0]["projectId"], "cached");
        assert!(state.apply_sidebar_part(Part::Projects, &projects(false, "fresh")));
        assert!(!state.apply_sidebar_part(Part::Projects, &projects(true, "old")));
        assert_eq!(state.cloud_sidebar_projects()[0]["projectId"], "fresh");
        assert!(state.cloud_sidebar_stale());
    }
    #[test]
    fn archived_tasks_leave_the_list_and_fresh_lists_bump_the_revision() {
        let mut state = AppState::new();
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        let start = state.cloud_sidebar_workspaces_revision();
        // Saved (provisional) rows paint but do not count as a refresh.
        assert!(state.apply_sidebar_part(Part::Workspaces, &workspaces(true)));
        assert_eq!(state.cloud_sidebar_workspaces_revision(), start);
        let fresh = json!({"accountId":"owner","organizationId":null,"apiOrigin":"https://cloud.test","provisional":false,"workspaces":[
            {"workspaceId":"saved","state":"paused","initialSessionId":"session"},
            {"workspaceId":"archiving","state":"archiving","desiredState":"archived","initialSessionId":"a"},
            {"workspaceId":"archived","state":"archived","desiredState":"archived","initialSessionId":"b"}]});
        assert!(state.apply_sidebar_part(Part::Workspaces, &fresh));
        let ids: Vec<_> = state
            .cloud_sidebar_workspaces()
            .iter()
            .map(|row| row["workspaceId"].clone())
            .collect();
        assert_eq!(
            ids,
            vec![json!("saved")],
            "archived tasks stay out of Threads"
        );
        let refreshed = state.cloud_sidebar_workspaces_revision();
        assert_ne!(refreshed, start);
        // Archiving from the sidebar hides the row before the event arrives.
        assert!(state.begin_cloud_sidebar_archive("saved"));
        assert!(state.cloud_sidebar_workspaces().is_empty());
        assert_ne!(state.cloud_sidebar_workspaces_revision(), refreshed);
        assert!(!state.begin_cloud_sidebar_archive("saved"));
    }
    fn listed(provisional: bool, rows: Value) -> Value {
        json!({"accountId":"owner","organizationId":null,"apiOrigin":"https://cloud.test","provisional":provisional,"workspaces":rows})
    }
    fn ids(state: &AppState) -> Vec<String> {
        state
            .cloud_sidebar_workspaces()
            .iter()
            .filter_map(|row| row["workspaceId"].as_str().map(str::to_owned))
            .collect()
    }
    #[gpui::test]
    fn archived_rows_stay_hidden_through_stale_lists_until_the_server_confirms(
        cx: &mut gpui::TestAppContext,
    ) {
        let state = cx.new(|_| AppState::new());
        let active = json!([
            {"workspaceId":"kept","state":"paused","initialSessionId":"k"},
            {"workspaceId":"gone","state":"paused","initialSessionId":"g"}]);
        state.update(cx, |state, cx| {
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            assert!(state.apply_sidebar_part(Part::Workspaces, &listed(false, active.clone())));
            assert!(state.begin_cloud_sidebar_archive("gone"));
            assert_eq!(ids(state), ["kept"]);
            // While the archive is in flight, a list read before the server
            // recorded it still lists the task as active; it stays hidden.
            assert!(state.apply_sidebar_part(Part::Workspaces, &listed(false, active.clone())));
            assert_eq!(ids(state), ["kept"]);
            // Acknowledged, the next stale list (read replica, or a read
            // that raced the archive) still cannot bring it back.
            state.acknowledge_cloud_sidebar_archive("gone", cx);
            assert!(state.apply_sidebar_part(Part::Workspaces, &listed(false, active.clone())));
            assert_eq!(ids(state), ["kept"]);
            assert!(state.cloud_sidebar_archive_pending("gone"));
            // Nor can a saved list painted again.
            state.cloud_sidebar.workspaces_fresh = false;
            assert!(state.apply_sidebar_part(Part::Workspaces, &listed(true, active.clone())));
            assert_eq!(ids(state), ["kept"]);
            // The server confirms: the task is listed as archiving.
            let confirmed = json!([
                {"workspaceId":"kept","state":"paused","initialSessionId":"k"},
                {"workspaceId":"gone","state":"archiving","desiredState":"archived","initialSessionId":"g"}]);
            assert!(state.apply_sidebar_part(Part::Workspaces, &listed(false, confirmed)));
            assert_eq!(ids(state), ["kept"]);
            assert!(!state.cloud_sidebar_archive_pending("gone"));
        });
    }
    #[gpui::test]
    fn a_task_absent_from_a_fresh_list_confirms_its_archive(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, cx| {
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            state.apply_sidebar_part(
                Part::Workspaces,
                &listed(false, json!([{"workspaceId":"gone","state":"paused"}])),
            );
            assert!(state.begin_cloud_sidebar_archive("gone"));
            // A list before the acknowledgement never clears the hold.
            state.apply_sidebar_part(Part::Workspaces, &listed(false, json!([])));
            assert!(state.cloud_sidebar_archive_pending("gone"));
            state.acknowledge_cloud_sidebar_archive("gone", cx);
            state.apply_sidebar_part(Part::Workspaces, &listed(false, json!([])));
            assert!(!state.cloud_sidebar_archive_pending("gone"));
        });
    }
    #[gpui::test]
    fn an_unconfirmed_archive_yields_to_the_server_after_its_window(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let active = json!([{"workspaceId":"task","state":"paused","initialSessionId":"s"}]);
        state.update(cx, |state, cx| {
            state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
            state.apply_sidebar_part(Part::Workspaces, &listed(false, active.clone()));
            assert!(state.begin_cloud_sidebar_archive("task"));
            state.acknowledge_cloud_sidebar_archive("task", cx);
            let pending = state.cloud_sidebar.pending_archive.get_mut("task").unwrap();
            pending.acknowledged = Some(std::time::Instant::now() - ARCHIVE_CONFIRM_WINDOW);
            state.apply_sidebar_part(Part::Workspaces, &listed(false, active));
            assert_eq!(ids(state), ["task"]);
            assert!(!state.cloud_sidebar_archive_pending("task"));
        });
    }
    #[test]
    fn a_failed_archive_restores_the_row() {
        let mut state = AppState::new();
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        let active = json!([
            {"workspaceId":"kept","state":"paused","initialSessionId":"k"},
            {"workspaceId":"task","state":"paused","initialSessionId":"s","title":"Fix login"}]);
        state.apply_sidebar_part(Part::Workspaces, &listed(false, active));
        assert!(state.begin_cloud_sidebar_archive("task"));
        assert_eq!(ids(&state), ["kept"]);
        let revision = state.cloud_sidebar_workspaces_revision();
        assert!(state.fail_cloud_sidebar_archive("task"));
        assert_eq!(ids(&state), ["kept", "task"]);
        assert_eq!(
            state.cloud_sidebar_workspace("task").unwrap()["title"],
            "Fix login"
        );
        assert_ne!(state.cloud_sidebar_workspaces_revision(), revision);
        assert!(!state.cloud_sidebar_archive_pending("task"));
        assert!(!state.fail_cloud_sidebar_archive("task"));
    }
    #[test]
    fn denial_and_account_org_or_api_changes_do_not_resurrect_saved_catalog() {
        let mut state = AppState::new();
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        state.apply_sidebar_part(Part::Projects, &projects(true, "cached"));
        state.apply_sidebar_part(Part::Workspaces, &workspaces(true));
        let scope = Scope::from_value(state.cloud_account.as_ref().unwrap()).unwrap();
        state.sidebar_error(
            &scope,
            Part::Projects,
            &RpcError::Failed("You do not have access to this workspace.".into()),
        );
        assert!(
            state.cloud_sidebar_projects().is_empty()
                && state.cloud_sidebar_workspaces().is_empty()
        );
        assert!(!state.apply_sidebar_part(Part::Projects, &projects(true, "late")));
        state.apply_cloud_account(json!({"user":{"id":"another"},"orgId":null}));
        assert!(!state.apply_sidebar_part(Part::Projects, &projects(true, "wrong-account")));
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
        state.apply_sidebar_part(Part::Projects, &projects(false, "current"));
        let mut wrong = projects(true, "other-api");
        wrong["apiOrigin"] = json!("https://other.test");
        assert!(!state.apply_sidebar_part(Part::Projects, &wrong));
        state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":"organization"}));
        assert!(state.cloud_sidebar_projects().is_empty());
        assert!(!state.apply_sidebar_part(Part::Workspaces, &workspaces(true)));
    }
    struct Controller {
        calls: Mutex<Vec<(String, bool)>>,
        fresh: tokio::sync::Semaphore,
    }
    #[async_trait::async_trait]
    impl RpcService for Controller {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            self.calls
                .lock()
                .unwrap()
                .push((method.into(), params["cacheOnly"] == true));
            match method {
                methods::CLOUD_ACCOUNT_STATUS => Ok(RpcReply::Stream(Box::pin(
                    futures::stream::once(async { json!({"user":{"id":"owner"},"orgId":null}) })
                        .chain(futures::stream::pending()),
                ))),
                methods::CLOUD_EVENTS => futures::future::pending().await,
                methods::CLOUD_PROJECTS | methods::CLOUD_WORKSPACES => {
                    let cached = params["cacheOnly"] == true;
                    if !cached {
                        self.fresh.acquire().await.unwrap().forget();
                    }
                    Ok(RpcReply::Value(if method == methods::CLOUD_PROJECTS {
                        projects(
                            cached,
                            if cached {
                                "cached-project"
                            } else {
                                "fresh-project"
                            },
                        )
                    } else {
                        workspaces(cached)
                    }))
                }
                // A context failure must never block sidebar metadata.
                methods::CLOUD_CONTEXT => Err(RpcError::Failed("managed base unavailable".into())),
                _ => Err(RpcError::UnknownMethod(method.into())),
            }
        }
    }
    #[gpui::test]
    fn sidebar_paints_scoped_saved_catalog_while_fresh_reads_are_held(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let service = Arc::new(Controller {
            calls: Mutex::new(vec![]),
            fresh: tokio::sync::Semaphore::new(0),
        });
        let handle = EngineHandle::from_test_client(memory_client(service.clone()));
        let state = cx.new(|_| AppState::new());
        let task = state.update(cx, |_, cx| spawn(cx, handle));
        for _ in 0..24 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        state.read_with(cx, |s, _| {
            assert_eq!(s.cloud_sidebar_projects()[0]["projectId"], "cached-project");
            assert_eq!(s.cloud_sidebar_workspaces()[0]["state"], "paused");
            assert!(s.cloud_sidebar_loading() && s.cloud_sidebar_stale());
            assert!(s.chats.is_empty() && s.spaces.is_empty());
        });
        assert!(
            !service
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|(m, _)| m == methods::CLOUD_CONTEXT
                    || m == methods::CLOUD_ENVIRONMENT
                    || m == methods::CLOUD_PAUSE
                    || m == "CloudResume"
                    || m == methods::CLOUD_CREATE)
        );
        service.fresh.add_permits(2);
        for _ in 0..24 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
        state.read_with(cx, |s, _| {
            assert_eq!(s.cloud_sidebar_projects()[0]["projectId"], "fresh-project");
            assert!(!s.cloud_sidebar_loading());
            assert!(!s.cloud_sidebar_stale());
        });
        drop(task);
        drop(state);
        for _ in 0..4 {
            cx.run_until_parked();
            runtime.block_on(async {
                tokio::task::yield_now().await;
            });
        }
    }
}
