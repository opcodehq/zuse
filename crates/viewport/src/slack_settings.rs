//! Shared Slack settings surface; the host owns authentication and OAuth.
use crate::{
    cloud_repositories::{Owner, Request},
    palette, settings_style as s,
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use std::collections::HashMap;
use zuse_protocol::{
    cloud_catalog::CloudAgentCatalog,
    methods,
    slack::{Connection, PendingConnection},
};

/// Shown while settings re-check the connection after a failed action.
const CHECKING: &str = "Checking the current connection…";

pub struct SlackPanel {
    owner: Option<Owner>,
    workspace: Option<String>,
    workspaces: Vec<Value>,
    workspaces_open: bool,
    next: u64,
    pending: HashMap<u64, (&'static str, Owner)>,
    connections: Vec<Connection>,
    pending_links: Vec<PendingConnection>,
    projects: Vec<Value>,
    catalog: Option<CloudAgentCatalog>,
    project: Option<String>,
    model: usize,
    models_open: bool,
    repositories_open: bool,
    error: Option<String>,
    notice: Option<String>,
    loaded: bool,
    execution_available: bool,
    confirm: Option<(String, String)>,
}
impl EventEmitter<Request> for SlackPanel {}
impl SlackPanel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            owner: None,
            workspace: None,
            workspaces: vec![],
            workspaces_open: false,
            next: 0,
            pending: HashMap::new(),
            connections: vec![],
            pending_links: vec![],
            projects: vec![],
            catalog: None,
            project: None,
            model: 0,
            models_open: false,
            repositories_open: false,
            error: None,
            notice: None,
            loaded: false,
            execution_available: false,
            confirm: None,
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
        self.workspace = owner.as_ref().map(|(_, organization)| {
            organization
                .as_ref()
                .map(|id| format!("organization:{id}"))
                .unwrap_or_else(|| "personal".into())
        });
        self.workspaces = if owner.is_some() {
            vec![json!({"id":"personal","name":"Personal"})]
        } else {
            vec![]
        };
        if let Some((_, Some(id))) = &owner {
            self.workspaces
                .push(json!({"id":format!("organization:{id}"),"name":"Current organization"}));
        }
        self.workspaces_open = false;
        self.connections.clear();
        self.pending_links.clear();
        self.projects.clear();
        self.catalog = None;
        self.project = None;
        self.model = 0;
        self.confirm = None;
        self.error = None;
        self.notice = None;
        self.loaded = false;
        self.execution_available = false;
        self.owner = owner;
        cx.notify();
    }
    pub fn interrupted(&mut self, cx: &mut Context<Self>) {
        self.pending.clear();
        // Stay "loaded" so render does not auto-refresh and wipe this message;
        // the user refreshes explicitly.
        self.loaded = true;
        self.error = Some("Connection interrupted. Refresh Slack settings.".into());
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
        if !self.pending.is_empty() {
            return;
        }
        if let Some(workspace) = &self.workspace {
            params["workspaceId"] = json!(workspace);
        }
        params["expectedAccountId"] = json!(owner.0);
        params["expectedOrganizationId"] = json!(owner.1);
        self.next += 1;
        self.pending.insert(self.next, (kind, owner));
        cx.emit(Request::Call {
            id: self.next,
            method,
            params,
        });
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.loaded = true;
        self.call("list", methods::SLACK_CONNECTIONS, json!({}), cx);
    }
    pub fn receive(&mut self, id: u64, result: Result<Value, String>, cx: &mut Context<Self>) {
        let Some((kind, owner)) = self.pending.remove(&id) else {
            return;
        };
        if self.owner.as_ref() != Some(&owner) {
            return;
        }
        let result = result.and_then(|value| {
            if value["accountId"].as_str() != Some(&owner.0)
                || value["organizationId"].as_str() != owner.1.as_deref()
            {
                return Err("Your workspace changed. Refresh Slack settings.".into());
            }
            Ok(value["data"].clone())
        });
        match result {
            Err(error) => {
                if kind != "list" {
                    // Refresh first: it clears `error`, and the failed action's
                    // message must survive it.
                    self.refresh(cx);
                    self.notice = Some(CHECKING.into());
                }
                self.error = Some(error);
            }
            Ok(data) if kind == "list" => {
                let expected = self.workspace.clone().unwrap_or_else(|| {
                    owner
                        .1
                        .as_ref()
                        .map(|id| format!("organization:{id}"))
                        .unwrap_or_else(|| "personal".into())
                });
                if !zuse_protocol::slack::settings_scope_matches(&data, &expected) {
                    self.error =
                        Some("Slack returned an invalid workspace. Refresh to retry.".into());
                    cx.notify();
                    return;
                }
                let parsed: Result<Vec<Connection>, _> =
                    serde_json::from_value(data["connections"].clone());
                let catalog: Result<CloudAgentCatalog, _> =
                    serde_json::from_value(data["catalog"].clone());
                match (parsed, catalog, data["projects"].as_array()) {
                    (Ok(connections), Ok(catalog), Some(projects)) => {
                        // The check after a failed action is done; its result is
                        // now on screen.
                        if self.notice.as_deref() == Some(CHECKING) {
                            self.notice = None;
                        }
                        self.workspace = data["workspaceId"].as_str().map(str::to_owned);
                        self.workspaces =
                            data["workspaces"].as_array().cloned().unwrap_or_default();
                        self.execution_available =
                            data["executionAvailable"].as_bool() == Some(true);
                        self.connections = connections;
                        self.pending_links =
                            serde_json::from_value(data["pending"].clone()).unwrap_or_default();
                        if self.catalog.is_none() {
                            self.model = catalog
                                .providers
                                .first()
                                .and_then(|p| p.models.iter().position(|m| m.id == p.default_model))
                                .unwrap_or(0);
                        }
                        self.catalog = Some(catalog);
                        self.projects = projects
                            .iter()
                            .filter(|p| {
                                p["prepared"] == true
                                    && p["state"] == "ready"
                                    && p["supportsSlack"] == true
                            })
                            .cloned()
                            .collect();
                        if self
                            .project
                            .as_ref()
                            .is_none_or(|id| !self.projects.iter().any(|p| p["projectId"] == *id))
                        {
                            self.project = self
                                .projects
                                .first()
                                .and_then(|p| p["projectId"].as_str().map(str::to_owned));
                        }
                    }
                    _ => {
                        self.error = Some(
                            "Slack settings returned an invalid response. Refresh to retry.".into(),
                        )
                    }
                }
            }
            Ok(data) if kind == "connect" => {
                if let Some(url) = data["url"].as_str().filter(|u| safe_oauth_url(u)) {
                    cx.open_url(url);
                    self.notice = Some(
                        "Choose your Slack workspace in the browser, then return here, refresh and confirm the connection."
                            .into(),
                    );
                } else {
                    self.error = Some("Slack returned an invalid sign-in link.".into());
                }
            }
            Ok(data) if kind == "confirm" => {
                self.notice = Some(if data["confirmed"] == true {
                    "Slack connected. Mention Zuse in a channel or send it a direct message to start a task.".into()
                } else {
                    "Slack sign-in declined. Nothing was connected.".into()
                });
                self.refresh(cx);
            }
            Ok(_) => {
                self.confirm = None;
                self.notice = Some("Slack account disconnected.".into());
                self.refresh(cx);
            }
        }
        cx.notify();
    }
}
fn safe_oauth_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/slack/oauth/start"
            && url.fragment().is_none()
    })
}
impl Render for SlackPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.loaded && self.owner.is_some() {
            self.refresh(cx);
        }
        let busy = !self.pending.is_empty();
        let workspace = self
            .workspaces
            .iter()
            .find(|w| w["id"].as_str() == self.workspace.as_deref())
            .and_then(|w| w["name"].as_str())
            .unwrap_or("Personal")
            .to_owned();
        let project = self
            .projects
            .iter()
            .find(|p| p["projectId"].as_str() == self.project.as_deref());
        let project_name = project
            .and_then(|p| p["displayName"].as_str())
            .unwrap_or("Choose repository")
            .to_owned();
        let provider = self.catalog.as_ref().and_then(|c| c.providers.first());
        let model = provider.and_then(|p| p.models.get(self.model));
        let model_label = model
            .map(|m| m.label.clone())
            .unwrap_or_else(|| "Choose model".into());
        let mut body = s::frame().child(s::header_with_action(
            "Slack",
            "Start cloud tasks from a mention or direct message and follow results in the thread.",
            Some(
                s::button("slack-refresh", "Refresh")
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                    .into_any_element(),
            ),
        ));
        let mut card = s::card().child(
            s::setting_row(true)
                .child(s::details(
                    "Zuse workspace",
                    "Choose whose repositories and cloud usage Slack tasks use.",
                ))
                .child(
                    s::button("slack-workspace", workspace).on_click(cx.listener(
                        |this, _, _, cx| {
                            if this.pending.is_empty() {
                                this.workspaces_open = !this.workspaces_open;
                                cx.notify();
                            }
                        },
                    )),
                ),
        );
        if self.workspaces_open {
            for (index, workspace) in self.workspaces.clone().into_iter().enumerate() {
                let id = workspace["id"].as_str().unwrap_or("personal").to_owned();
                let name = workspace["name"].as_str().unwrap_or("Personal").to_owned();
                card = card.child(s::button(("slack-workspace-choice", index), name).on_click(
                    cx.listener(move |this, _, _, cx| {
                        if this.pending.is_empty() {
                            this.workspace = Some(id.clone());
                            this.connections.clear();
                            this.projects.clear();
                            this.project = None;
                            this.workspaces_open = false;
                            this.refresh(cx);
                        }
                    }),
                ));
            }
        }

        // A finished Slack sign-in only links after it is confirmed here, so a
        // leaked sign-in link cannot attach someone else's Slack user.
        for (index, link) in self.pending_links.clone().into_iter().enumerate() {
            let who = link
                .slack_user_name
                .clone()
                .unwrap_or_else(|| link.slack_user_id.clone());
            let (accept, decline) = (link.pending_id.clone(), link.pending_id.clone());
            card = card.child(
                s::setting_row(false)
                    .child(s::details(
                        format!("Confirm {who} in {}", link.team_name),
                        "Tasks from this Slack user will use this account's repositories and cloud usage.",
                    ))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(s::button(("slack-decline", index), "Decline").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.call(
                                        "confirm",
                                        methods::SLACK_CONFIRM,
                                        json!({"pendingId":decline,"accept":false}),
                                        cx,
                                    )
                                }),
                            ))
                            .child(s::button(("slack-accept", index), "Confirm").on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.call(
                                        "confirm",
                                        methods::SLACK_CONFIRM,
                                        json!({"pendingId":accept,"accept":true}),
                                        cx,
                                    )
                                }),
                            )),
                    ),
            );
        }
        for (index, connection) in self.connections.clone().into_iter().enumerate() {
            let confirm = self.confirm.as_ref()
                == Some(&(connection.team_id.clone(), connection.slack_user_id.clone()));
            let team = connection.team_id.clone();
            let user = connection.slack_user_id.clone();
            card = card.child(
                s::setting_row(false)
                    .child(s::details(
                        connection.team_name,
                        format!(
                            "{} · {}",
                            self.projects
                                .iter()
                                .find(|p| p["projectId"] == connection.project_id)
                                .and_then(|p| p["displayName"].as_str())
                                .unwrap_or("Repository unavailable"),
                            self.catalog
                                .as_ref()
                                .and_then(|c| c
                                    .providers
                                    .iter()
                                    .find(|p| String::from(p.id) == connection.agent))
                                .map(|p| p.name.as_str())
                                .unwrap_or("Cloud agent")
                        ),
                    ))
                    .child(
                        s::button(
                            ("slack-disconnect", index),
                            if confirm {
                                "Confirm disconnect"
                            } else {
                                "Disconnect"
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if confirm {
                                this.call(
                                    "disconnect",
                                    methods::SLACK_DISCONNECT,
                                    json!({"teamId":team,"slackUserId":user}),
                                    cx,
                                );
                            } else {
                                this.confirm = Some((team.clone(), user.clone()));
                                cx.notify();
                            }
                        })),
                    ),
            );
        }
        card = card.child(
            s::setting_row(false)
                .child(s::details(
                    "Repository",
                    "New Slack threads use this repository.",
                ))
                .child(
                    s::button("slack-repository", project_name).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.repositories_open = !this.repositories_open;
                            cx.notify();
                        },
                    )),
                ),
        );
        if self.repositories_open {
            for (index, project) in self.projects.clone().into_iter().enumerate() {
                let id = project["projectId"].as_str().unwrap_or_default().to_owned();
                let label = project["displayName"].as_str().unwrap_or(&id).to_owned();
                card = card.child(s::button(("slack-project", index), label).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.project = Some(id.clone());
                        this.repositories_open = false;
                        cx.notify();
                    }),
                ));
            }
        }
        card = card.child(
            s::setting_row(false)
                .child(s::details(
                    "Agent and model",
                    provider.map(|p| p.name.clone()).unwrap_or_default(),
                ))
                .child(s::button("slack-model", model_label).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.models_open = !this.models_open;
                        cx.notify();
                    },
                ))),
        );
        if self.models_open {
            if let Some(provider) = provider {
                for (index, model) in provider.models.iter().enumerate() {
                    card = card.child(
                        s::button(("slack-model-choice", index), model.label.clone()).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.model = index;
                                this.models_open = false;
                                cx.notify();
                            }),
                        ),
                    );
                }
            }
        }
        let enabled =
            !busy && self.execution_available && self.project.is_some() && model.is_some();
        card=card.child(s::setting_row(false).child(s::details("Connect Slack workspace","Each Slack member connects their own account. Existing threads keep their original account.")).child(s::button("slack-connect",if busy{"Working…"}else{"Connect Slack"}).opacity(if enabled{1.}else{0.5}).on_click(cx.listener(move|this,_,_,cx|{
            if !enabled{return;}
            let Some(provider)=this.catalog.as_ref().and_then(|c|c.providers.first())else{return;};let Some(model)=provider.models.get(this.model)else{return;};
            this.call("connect",methods::SLACK_CONNECT,json!({"projectId":this.project,"agent":provider.id,"model":model.id}),cx);
        }))));
        body = body.child(card);
        if self.loaded && !busy && !self.execution_available {
            body=body.child(s::description_text("Organization cloud runtimes are not available on this deployment yet. Choose Personal to start Slack tasks."));
        }

        if self.projects.is_empty() && self.loaded && !busy {
            body = body.child(s::description_text(
                "Prepare or rebuild a cloud repository with Slack support above before connecting.",
            ));
        }
        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .text_size(px(12.))
                    .text_color(s::destructive_text())
                    .child(error.clone()),
            );
        }
        if let Some(notice) = &self.notice {
            body = body.child(s::description_text(notice.clone()));
        }
        body.text_color(palette::rgb(crate::shell::TEXT))
    }
}
