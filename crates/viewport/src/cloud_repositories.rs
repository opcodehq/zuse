//! One repository settings component shared by desktop and browser.
use crate::{
    input::TextInput,
    settings_style as s,
    shell::{column, label, row},
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use zuse_protocol::{
    cloud_projects::{CloudProject, GithubCatalog, GithubRepository, ProjectList},
    methods,
};
mod images;
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
    image: images::ImageView,
    owner: Option<Owner>,
    next: u64,
    pending: BTreeMap<u64, Pending>,
    loaded: bool,
    projects: Vec<CloudProject>,
    github: Option<GithubCatalog>,
    error: Option<String>,
    notice: Option<String>,
    picker: bool,
    search: Entity<TextInput>,
    selected: BTreeSet<String>,
    queue: Vec<GithubRepository>,
    keys: BTreeMap<String, String>,
}
impl EventEmitter<Request> for RepositoryPanel {}
impl RepositoryPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new("Search repositories…", window, cx));
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        Self {
            image: Default::default(),
            owner: None,
            next: 0,
            pending: BTreeMap::new(),
            loaded: false,
            projects: vec![],
            github: None,
            error: None,
            notice: None,
            picker: false,
            search,
            selected: BTreeSet::new(),
            queue: vec![],
            keys: BTreeMap::new(),
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
        self.owner = owner;
        self.loaded = false;
        self.projects.clear();
        self.github = None;
        self.error = None;
        self.notice = None;
        self.picker = false;
        self.selected.clear();
        self.queue.clear();
        self.keys.clear();
        self.search
            .update(cx, |s, cx| s.set_value(String::new(), cx));
        cx.notify();
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
        if matches!(pending.kind, "image" | "image-build") {
            self.receive_image(pending, result, cx);
            return;
        }
        let value = match result {
            Ok(v) => v,
            Err(e) => {
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
        match pending.kind {
            "list" => match serde_json::from_value::<ProjectList>(data) {
                Ok(list) => self.projects = list.projects,
                Err(_) => {
                    self.error =
                        Some("Repository list is unavailable. Refresh and try again.".into())
                }
            },
            "github" => match serde_json::from_value::<GithubCatalog>(data) {
                Ok(github) => self.github = Some(github),
                Err(_) => {
                    self.error =
                        Some("GitHub repository list is unavailable. Refresh and try again.".into())
                }
            },
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
        self.pending
            .values()
            .any(|p| !matches!(p.kind, "image" | "image-build"))
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
impl Render for RepositoryPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.loaded && self.owner.is_some() {
            self.refresh(cx);
        }
        let busy = self.repository_busy();
        let can_manage = self.can_manage();
        let mut card = s::card();
        if self.projects.is_empty() {
            card = card.child(
                div()
                    .p_4()
                    .text_size(px(12.))
                    .child(if self.owner.is_none() {
                        "Sign in to manage cloud repositories."
                    } else if busy {
                        "Loading repositories…"
                    } else {
                        "No repositories included"
                    }),
            );
        }
        for project in &self.projects {
            let id = project.project_id.clone();
            card = card.child(
                row()
                    .p_3()
                    .justify_between()
                    .gap_3()
                    .child(
                        column()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(s::title_text(
                                project.repository_identity.replace("github.com/", ""),
                            ))
                            .child(s::description_text(format!(
                                "{} · {}",
                                project.default_branch,
                                if project.visibility
                                    == zuse_protocol::cloud_projects::ProjectVisibility::Private
                                {
                                    "Private"
                                } else {
                                    "Public"
                                }
                            ))),
                    )
                    .child(
                        s::button(
                            SharedString::from(format!("remove-cloud-repository-{id}")),
                            "Remove",
                        )
                        .when(busy || !can_manage, |b| b.opacity(0.5))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.repository_busy() || !this.can_manage() {
                                return;
                            }
                            this.error = None;
                            this.notice = None;
                            this.call(
                                "remove",
                                methods::CLOUD_PROJECT_REMOVE,
                                json!({"projectId":id}),
                                cx,
                            );
                            cx.notify();
                        })),
                    ),
            );
        }
        let available = self
            .github
            .as_ref()
            .is_some_and(|g| g.installations.iter().any(|i| !i.suspended));
        let mut trigger = div().relative().child(
            s::button("add-cloud-repositories", "+ Repository")
                .when(busy || !available || !can_manage, |b| b.opacity(0.5))
                .on_click(cx.listener(|this, _, _, cx| {
                    if this.repository_busy()
                        || !this.can_manage()
                        || !this
                            .github
                            .as_ref()
                            .is_some_and(|g| g.installations.iter().any(|i| !i.suspended))
                    {
                        return;
                    }
                    this.picker = !this.picker;
                    this.selected.clear();
                    this.error = None;
                    if this.picker {
                        this.refresh(cx);
                    }
                    cx.notify();
                })),
        );
        if self.picker {
            let search = self.search.read(cx).value(cx).trim().to_lowercase();
            let included = self
                .projects
                .iter()
                .map(|p| {
                    p.repository_identity
                        .replace("github.com/", "")
                        .to_lowercase()
                })
                .collect::<BTreeSet<_>>();
            let repos = self
                .github
                .as_ref()
                .map(|g| {
                    g.repositories
                        .iter()
                        .filter(|r| {
                            !included.contains(&r.name_with_owner.to_lowercase())
                                && (search.is_empty()
                                    || r.name_with_owner.to_lowercase().contains(&search)
                                    || r.description
                                        .as_deref()
                                        .unwrap_or("")
                                        .to_lowercase()
                                        .contains(&search))
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mut choices = column()
                .id("cloud-repository-choices")
                .max_h(px(260.))
                .overflow_y_scroll();
            for repo in &repos {
                let name = repo.name_with_owner.clone();
                let selected = self.selected.contains(&name);
                choices = choices.child(
                    s::button(
                        SharedString::from(format!("cloud-repository-choice-{name}")),
                        format!(
                            "{}{}",
                            name,
                            if repo.is_private { " · Private" } else { "" }
                        ),
                    )
                    .aria_selected(selected)
                    .when(selected, |button| {
                        button.child(crate::assets::icon("check"))
                    })
                    .w_full()
                    .justify_start()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.selected.remove(&name) {
                            this.selected.insert(name.clone());
                        }
                        cx.notify();
                    })),
                );
            }
            if repos.is_empty() {
                choices = choices.child(
                    div()
                        .p_3()
                        .text_size(px(11.))
                        .child("No matching repositories"),
                );
            }
            let menu = s::frame()
                .w(px(420.))
                .gap_2()
                .p_2()
                .border_1()
                .border_color(s::card_border())
                .shadow_lg()
                .child(
                    row()
                        .gap_2()
                        .child(s::field().flex_1().child(self.search.clone()))
                        .child(s::button("close-repository-picker", "Close").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.picker = false;
                                cx.notify();
                            }),
                        )),
                )
                .child(choices)
                .child(
                    row()
                        .justify_between()
                        .child(label(format!("{} selected", self.selected.len())))
                        .child(
                            s::button(
                                "connect-selected-repositories",
                                format!("Add {}", self.selected.len()),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.repository_busy() || this.selected.is_empty() {
                                    return;
                                }
                                this.queue = this
                                    .github
                                    .as_ref()
                                    .map(|g| {
                                        g.repositories
                                            .iter()
                                            .filter(|r| this.selected.contains(&r.name_with_owner))
                                            .cloned()
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                this.picker = false;
                                this.selected.clear();
                                this.error = None;
                                this.connect_next(cx);
                                cx.notify();
                            })),
                        ),
                );
            trigger = trigger.child(deferred(
                anchored()
                    .anchor(Anchor::TopRight)
                    .snap_to_window_with_margin(px(8.))
                    .child(
                        menu.occlude()
                            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                                this.picker = false;
                                cx.notify();
                            })),
                    ),
            ));
        }
        let mut view = s::frame()
            .child(
                row()
                    .justify_between()
                    .child(s::header(
                        "Repositories",
                        "Choose the repositories to include in your cloud image.",
                    ))
                    .child(trigger),
            )
            .child(card);
        if let Some(error) = &self.error {
            view = view.child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(11.))
                    .child(error.clone())
                    .child(
                        s::button("retry-cloud-repositories", "Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            );
        }
        if let Some(notice) = &self.notice {
            view = view.child(div().px_3().py_2().text_size(px(11.)).child(notice.clone()));
        }
        column().gap_4().child(view).child(self.render_image(cx))
    }
}
