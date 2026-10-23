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
    pub(super) fn leave_cloud(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _| state.cloud_project = None);
        if let Some(config) = self.cloud.local.take() {
            self.config = config;
        }
    }
    fn select_cloud(&mut self, row: Value, cx: &mut Context<Self>) {
        if row["prepared"] != true || row["state"] != "ready" {
            self.cloud.error =
                Some("Prepare a Rust cloud image for this repository before starting.".into());
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
        let owner = state.auth_user().map(|user| user.id.clone());
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
            let _ = this.update(cx, |this, cx| {
                if this.state.read(cx).auth_user().map(|u| u.id.as_str()) != Some(owner.as_str()) {
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
                    _ => {
                        this.cloud.error = Some(
                            "Could not load cloud destinations. Open the picker again to retry."
                                .into(),
                        )
                    }
                }
                cx.notify();
            });
        }));
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
            .gap(px(2.))
            .children(
                self.cloud
                    .rows
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, row)| {
                        let ready = row["prepared"] == true && row["state"] == "ready";
                        let selected = selected.as_ref() == Some(&row["projectId"]);
                        popover::picker_row(
                            &theme,
                            selected,
                            false,
                            format!("cloud-device-{index}"),
                        )
                        .id(("cloud-device", index))
                        .child(
                            crate::icons::icon(crate::icons::CLOUD)
                                .size(px(14.))
                                .text_color(theme.text_muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .child(SharedString::from(format!(
                                    "Cloud · {}",
                                    row["displayName"].as_str().unwrap_or("Repository")
                                ))),
                        )
                        .child(div().text_color(theme.text_muted).child("Beta"))
                        .when(!ready, |view| {
                            view.child(
                                div()
                                    .text_color(theme.text_muted)
                                    .child("Update cloud image"),
                            )
                        })
                        .child(popover::radio_dot(&theme, selected))
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_cloud(row.clone(), cx)),
                        )
                    }),
            )
            .children(
                self.cloud
                    .workspaces
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, row)| {
                        popover::picker_row(&theme, false, false, format!("cloud-existing-{index}"))
                            .id(("cloud-existing", index))
                            .child(crate::icons::icon(crate::icons::CLOUD).size(px(14.)))
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
                                    state.auth_user().map(|u| u.id.clone()),
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
            .child(popover::menu_separator())
            .child(
                popover::picker_row(&theme, false, false, "add-computer")
                    .id("add-computer")
                    .child(crate::icons::icon(crate::icons::MONITOR).size(px(14.)))
                    .child("Add computer…")
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
