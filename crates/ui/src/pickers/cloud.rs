//! Prepared destinations extend the native device picker and keep local picks intact.
use super::*;
use serde_json::{Value, json};
#[derive(Default)]
pub(super) struct Cloud {
    owner: Option<String>,
    loaded: bool,
    task: Option<Task<()>>,
    rows: Vec<Value>,
    workspaces: Vec<Value>,
    error: Option<String>,
    local: Option<DraftConfig>,
}
impl Cloud {
    pub(super) fn refresh(&mut self) {
        self.loaded = false;
    }
}
impl Pickers {
    pub(crate) fn start_without_project(&mut self, cx: &mut Context<Self>) {
        self.pick_no_project(cx);
    }
    pub(super) fn leave_cloud(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _| state.cloud_project = None);
        if let Some(config) = self.cloud.local.take() {
            self.config = config;
        }
    }
    fn select_cloud(&mut self, row: Value, cx: &mut Context<Self>) {
        if row["prepared"] != true || row["state"] != "ready" {
            self.cloud.error =
                Some("Build a cloud image for this repository before starting.".into());
            cx.notify();
            return;
        }
        if row["accountId"].as_str() != self.state.read(cx).cloud_user_id() {
            self.cloud.error = Some("Your account changed. Open the project picker again.".into());
            cx.notify();
            return;
        }
        self.cloud.local.get_or_insert_with(|| self.config.clone());
        self.config.harness = Some(HarnessId::ClaudeCode);
        self.config.model = Some("sonnet".into());
        self.config.reasoning = None;
        self.config.sandbox = Some(SandboxLevel::WorkspaceWrite);
        self.config.branch = None;
        self.state.update(cx, |state, cx| {
            state.cloud_project = Some(row);
            cx.notify();
        });
        self.close(cx);
    }
    fn ensure_cloud(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let owner = state.cloud_user_id().map(str::to_owned);
        if self.cloud.owner != owner {
            self.cloud.owner = owner.clone();
            self.cloud.loaded = false;
            self.cloud.rows.clear();
            self.cloud.workspaces.clear();
            self.cloud.task = None;
        }
        if self.cloud.loaded || self.cloud.task.is_some() {
            return;
        }
        let Some(owner) = owner else {
            return;
        };
        let Some(engine) = state.engine().map(|engine| engine.controller()) else {
            return;
        };
        self.cloud.loaded = true;
        let job = gpui_tokio::Tokio::spawn(cx, async move {
            let (context, list) = futures::try_join!(
                engine
                    .client()
                    .call_scoped(methods::CLOUD_CONTEXT, json!({})),
                engine
                    .client()
                    .call_scoped(methods::CLOUD_WORKSPACES, json!({}))
            )?;
            Ok::<_, zuse_rpc::RpcError>((context, list))
        });
        self.cloud.task = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ =
                this.update(cx, |this, cx| {
                    if this.state.read(cx).cloud_user_id() != Some(owner.as_str()) {
                        return;
                    }
                    this.cloud.task = None;
                    match result {
                        Ok(Ok((value, list)))
                            if value["accountId"].as_str() == Some(owner.as_str()) =>
                        {
                            this.cloud.workspaces = list["workspaces"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default()
                                .into_iter()
                                .filter(|w| {
                                    w["desiredState"] != "deleted"
                                        && w["desiredState"] != "archived"
                                        && w["state"] != "deleted"
                                })
                                .collect();
                            this.cloud.rows = value["projects"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default()
                                .into_iter()
                                .map(|mut row| {
                                    row["accountId"] = json!(owner);
                                    row
                                })
                                .collect();
                            this.cloud.error = None;
                        }
                        Ok(Err(error)) => this.cloud.error = Some(error.to_string()),
                        _ => this.cloud.error = Some(
                            "Could not load cloud destinations. Open the picker again to retry."
                                .into(),
                        ),
                    }
                    cx.notify();
                });
        }));
    }
    pub(super) fn filtered_cloud_projects(&self, cx: &App) -> Vec<Value> {
        let query = self.search.read(cx).text().to_lowercase();
        self.cloud
            .rows
            .iter()
            .filter(|p| {
                !self.state.read(cx).spaces.iter().any(|space| {
                    space.remote_repository.as_deref().is_some_and(|identity| {
                        p["repositoryIdentity"]
                            .as_str()
                            .is_some_and(|v| v.eq_ignore_ascii_case(identity))
                    })
                }) && format!(
                    "{} {}",
                    p["displayName"].as_str().unwrap_or(""),
                    p["repositoryIdentity"].as_str().unwrap_or("")
                )
                .to_lowercase()
                .contains(&query)
            })
            .cloned()
            .collect()
    }
    pub(super) fn cloud_destination(&self, cx: &App) -> Option<Value> {
        let state = self.state.read(cx);
        if let Some(project) = &state.cloud_project {
            return Some(project.clone());
        }
        let identity = state.selected_space_row()?.remote_repository.as_ref()?;
        self.cloud
            .rows
            .iter()
            .find(|p| {
                p["repositoryIdentity"]
                    .as_str()
                    .is_some_and(|v| v.eq_ignore_ascii_case(identity))
            })
            .cloned()
    }
    pub(super) fn cloud_project_rows(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_cloud(cx);
        let selected = self
            .state
            .read(cx)
            .cloud_project
            .as_ref()
            .map(|p| p["projectId"].clone());
        let base = self.filtered_project_rows(cx).len() + 1;
        let mut view = div().flex().flex_col();
        for (i, project) in self.filtered_cloud_projects(cx).into_iter().enumerate() {
            let chosen = selected.as_ref() == Some(&project["projectId"]);
            view = view.child(
                zuse_viewport::creation_style::option(
                    ("cloud-project", i),
                    project["displayName"]
                        .as_str()
                        .unwrap_or("Repository")
                        .to_owned(),
                    "folder",
                    chosen,
                )
                .h(px(32.))
                .when(self.active == base + i, |v| {
                    v.bg(Theme::of(cx).element_hover)
                })
                .child(zuse_viewport::creation_style::selection(chosen))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if project["prepared"] == true && project["state"] == "ready" {
                        this.select_cloud(project.clone(), cx);
                    } else {
                        this.close(cx);
                        window.dispatch_action(Box::new(crate::shell::OpenCloudSettings), cx);
                    }
                })),
            );
        }
        view.when_some(self.cloud.error.clone(), |v, error| {
            v.child(
                div()
                    .p_2()
                    .text_size(px(11.))
                    .text_color(Theme::of(cx).warning)
                    .child(error),
            )
        })
        .into_any_element()
    }
    pub(super) fn select_cloud_index(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.filtered_cloud_projects(cx).get(i).cloned() {
            self.select_cloud(row, cx);
        }
    }
    pub(super) fn select_cloud_destination(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self.cloud_destination(cx) {
            self.select_cloud(row, cx);
        }
    }
    pub(super) fn cloud_rows(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_cloud(cx);
        let theme = Theme::of(cx).for_popup();
        let selected = self
            .state
            .read(cx)
            .cloud_project
            .as_ref()
            .map(|p| p["projectId"].clone());
        div()
            .flex()
            .flex_col()
            .when_some(self.cloud_destination(cx), |view, row| {
                let ready = row["prepared"] == true && row["state"] == "ready";
                let selected = selected.as_ref() == Some(&row["projectId"]);
                view.child(zuse_viewport::creation_style::separator())
                    .child(
                        zuse_viewport::creation_style::option(
                            "cloud-destination",
                            "Cloud sandbox · boxd",
                            "cloud-dither",
                            selected,
                        )
                        .when(self.active == self.filtered_device_rows(cx).len(), |v| {
                            v.bg(theme.element_hover)
                        })
                        .child(zuse_viewport::creation_style::meta("Beta"))
                        .when(!ready, |v| {
                            v.child(zuse_viewport::creation_style::meta("Set up cloud"))
                        })
                        .child(zuse_viewport::creation_style::selection(selected))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_cloud(row.clone(), cx)),
                        ),
                    )
            })
            .children(
                self.cloud
                    .workspaces
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, row)| {
                        popover::picker_row(&theme, false, false, format!("cloud-existing-{index}"))
                            .id(("cloud-existing", index))
                            .child(crate::icons::icon(crate::icons::DITHER_CLOUD).size(px(14.)))
                            .child(SharedString::from(format!(
                                "Open · {}",
                                row["title"]
                                    .as_str()
                                    .or(row["branch"].as_str())
                                    .unwrap_or("Cloud thread")
                            )))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let state = this.state.read(cx);
                                let (Some(controller), Some(subject)) = (
                                    state.engine().map(|e| e.controller()),
                                    state.cloud_user_id().map(str::to_owned),
                                ) else {
                                    return;
                                };
                                let target = crate::state::CloudTarget {
                                    controller,
                                    subject,
                                    workspace: row["workspaceId"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .into(),
                                    initial_session: row["initialSessionId"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .into(),
                                };
                                this.close(cx);
                                crate::cloud_history::saved::open(target, cx);
                            }))
                    }),
            )
            .child(zuse_viewport::creation_style::separator())
            .child(
                zuse_viewport::creation_style::option(
                    "add-computer",
                    "Add computer…",
                    "monitor",
                    false,
                )
                .role(gpui::Role::Button)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.close(cx);
                    window.dispatch_action(Box::new(crate::shell::OpenDevices), cx);
                })),
            )
            .when_some(self.cloud.error.clone(), |view, error| {
                view.child(
                    div()
                        .p_2()
                        .text_size(crate::typography::text_xs())
                        .text_color(theme.text_muted)
                        .child(SharedString::from(error)),
                )
            })
            .into_any_element()
    }
    pub(super) fn cloud_git_summary(&self, cx: &App) -> Option<AnyElement> {
        let row = self.state.read(cx).cloud_project.as_ref()?;
        let theme = Theme::of(cx);
        Some(
            workspace_footer_row()
                .w_auto()
                .flex_none()
                .gap(px(6.))
                .child(
                    crate::icons::icon(crate::icons::GIT_BRANCH)
                        .size(px(14.))
                        .text_color(theme.text_muted),
                )
                .child(SharedString::from(
                    row["defaultBranch"].as_str().unwrap_or("main").to_owned(),
                ))
                .into_any_element(),
        )
    }
}
