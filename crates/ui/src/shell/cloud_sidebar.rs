//! Cloud tasks share project headings with local checkouts. These are real
//! account task rows, never synthetic local chats or guest connections; the
//! one exception is a new cloud chat still being created, shown from its
//! tab's pending launch until the account's list has it.
use super::*;
use serde_json::Value;

pub(super) struct CloudSidebarGroup {
    pub group: Option<(String, String)>,
    pub project: Value,
    pub rows: Vec<Value>,
}
impl Shell {
    pub(super) fn has_offline_cloud_navigation(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        matches!(state.connection, ConnectionStatus::Failed(_))
            && state.engine().is_some()
            && !state.cloud_sidebar_projects().is_empty()
    }
    /// Account navigation stays available while the local execution lane fails.
    /// This never changes its connection state or installs a local composer.
    pub(super) fn render_offline_cloud_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let groups = self.cloud_sidebar_groups(cx);
        let mut elements = Vec::new();
        for group in groups {
            let project = group.project;
            let title = group
                .group
                .as_ref()
                .map(|(_, name)| name.as_str())
                .or_else(|| project["displayName"].as_str())
                .unwrap_or("Cloud project")
                .to_owned();
            let id = project["projectId"].as_str().unwrap_or_default().to_owned();
            let header = div()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(8.))
                .py(px(8.))
                .child(
                    icon(icons::DITHER_CLOUD)
                        .size(px(14.))
                        .text_color(theme.text_muted),
                )
                .child(div().flex_1().text_size(px(12.)).child(title))
                .child(
                    div()
                        .id(SharedString::from(format!("offline-cloud-new-{id}")))
                        .cursor_pointer()
                        .px(px(5.))
                        .py(px(3.))
                        .rounded_md()
                        .text_size(px(11.))
                        .child("New thread")
                        .hover(|v| v.bg(theme.element_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_new_cloud_session(project.clone(), cx)
                        })),
                );
            let mut rows = Vec::new();
            for row in group.rows {
                if let Some((_, _, row)) = self.render_cloud_sidebar_row(&row, &theme, cx) {
                    rows.push(row)
                }
            }
            elements.push(div().child(header).children(rows).into_any_element());
        }
        div()
            .id("offline-cloud-sidebar")
            .w(px(self.settings.sidebar_width.max(SIDEBAR_MIN)))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme.bg)
            .text_color(theme.text)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .px(px(16.))
                    .py(px(16.))
                    .text_size(px(14.))
                    .child("Cloud conversations"),
            )
            .child(
                div()
                    .id("offline-cloud-projects")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(6.))
                    .children(elements),
            )
            .child(
                div()
                    .px(px(16.))
                    .py(px(12.))
                    .text_size(px(11.))
                    .text_color(theme.text_muted)
                    .child("Local computer offline · Cloud access is separate"),
            )
            .into_any_element()
    }
    pub(super) fn cloud_sidebar_groups(&self, cx: &App) -> Vec<CloudSidebarGroup> {
        let state = self.state.read(cx);
        let mut workspaces = state.cloud_sidebar_workspaces();
        // New chats still being created show at once, like a local chat
        // does, until the account's list has them.
        let now = Utc::now().timestamp_millis();
        for launch in self.composer.read(cx).cloud_launch_summaries() {
            if launch.workspace.as_ref().is_some_and(|id| {
                workspaces
                    .iter()
                    .any(|row| row["workspaceId"].as_str() == Some(id.as_str()))
            }) {
                continue;
            }
            workspaces.push(pending_launch_row(&launch, now));
        }
        let filter_key = self
            .settings
            .space_filter
            .as_deref()
            .and_then(|id| state.space_row(id))
            .map(zuse_protocol::view::project_key);
        let projects = state.cloud_sidebar_projects();
        let labels = state.project_labels(&projects);
        projects
            .into_iter()
            .filter_map(|project| {
                let mut identity =
                    zuse_protocol::presentation::cloud_project_group(&project, &state.spaces)?;
                if let Some(label) = labels.cloud.get(&identity.project_id) {
                    identity.name = label.clone();
                }
                if filter_key.as_ref().is_some_and(|key| key != &identity.key) {
                    return None;
                }
                let group = match self.settings.sidebar_organization {
                    SidebarOrganization::ByProject => Some((identity.key, identity.name)),
                    SidebarOrganization::ByDevice => {
                        Some(("cloud-computers".into(), "Cloud computers".into()))
                    }
                    SidebarOrganization::InOneList => None,
                };
                let mut rows: Vec<_> = workspaces
                    .iter()
                    .filter(|row| row["projectId"].as_str() == Some(identity.project_id.as_str()))
                    .cloned()
                    .collect();
                rows.sort_by(|a, b| {
                    b["updatedAt"]
                        .as_i64()
                        .unwrap_or_default()
                        .cmp(&a["updatedAt"].as_i64().unwrap_or_default())
                });
                Some(CloudSidebarGroup {
                    group,
                    project,
                    rows,
                })
            })
            .collect()
    }
    /// The conversation target for a cloud task row, when it can open.
    pub(super) fn cloud_row_target(
        &self,
        row: &Value,
        cx: &App,
    ) -> Option<crate::state::CloudTarget> {
        let workspace = row["workspaceId"].as_str().filter(|id| !id.is_empty())?;
        let session = row["initialSessionId"]
            .as_str()
            .filter(|id| !id.is_empty())?;
        let state = self.state.read(cx);
        Some(crate::state::CloudTarget {
            controller: state.engine()?.controller(),
            subject: state.cloud_user_id()?.to_owned(),
            organization: state.cloud_organization_id().map(str::to_owned),
            workspace: workspace.to_owned(),
            initial_session: session.to_owned(),
        })
    }
    /// Open the cloud task behind a `cloud:<workspace>` sidebar row.
    pub(super) fn open_cloud_sidebar_row(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(workspace) = id.strip_prefix(super::CLOUD_ROW_PREFIX) else {
            return;
        };
        if let Some(key) = workspace.strip_prefix(PENDING_LAUNCH_PREFIX) {
            self.select_cloud_launch(key, cx);
            return;
        }
        let row = self
            .state
            .read(cx)
            .cloud_sidebar_workspaces()
            .into_iter()
            .find(|row| row["workspaceId"].as_str() == Some(workspace));
        let Some(row) = row else { return };
        if let Some(target) = self.cloud_row_target(&row, cx) {
            self.open_cloud_conversation(target, row, serde_json::json!({}), cx);
        }
    }
    /// Archive the cloud task behind a `cloud:<workspace>` row. Its computer
    /// sleeps now and is deleted after the archive retention; the chat stays
    /// in Settings → Archived, where restoring it brings a computer back.
    pub(super) fn archive_cloud_sidebar_row(&mut self, id: &str, cx: &mut Context<Self>) {
        self.close_chat_menu(cx);
        let Some(workspace) = id.strip_prefix(super::CLOUD_ROW_PREFIX) else {
            return;
        };
        motion::set_hover(&format!("chat-{id}-hover"), false, true);
        if self.cloud_archiving.contains(workspace) {
            return;
        }
        let row = self
            .state
            .read(cx)
            .cloud_sidebar_workspaces()
            .into_iter()
            .find(|row| row["workspaceId"].as_str() == Some(workspace));
        let Some(target) = row.and_then(|row| self.cloud_row_target(&row, cx)) else {
            return;
        };
        // Settings → Archived restores personal computers only; an archived
        // organization task would have nowhere to be restored from.
        if target.organization.is_some() {
            self.show_notice(
                crate::toast::ToastKind::Info,
                "Archiving is available for personal cloud chats.",
                cx,
            );
            return;
        }
        let workspace = workspace.to_owned();
        self.cloud_archiving.insert(workspace.clone());
        // The row leaves at once and stays out of every list, including one
        // read before the server records the archive, until it is confirmed.
        self.state.update(cx, |state, cx| {
            if state.begin_cloud_sidebar_archive(&workspace) {
                cx.notify();
            }
        });
        let command = uuid::Uuid::new_v4().to_string();
        cx.spawn(async move |this, cx| {
            let result = target.archive(&command).await;
            let _ = this.update(cx, |this, cx| {
                this.cloud_archiving.remove(&workspace);
                match result {
                    Ok(_) => {
                        this.state.update(cx, |state, cx| {
                            state.acknowledge_cloud_sidebar_archive(&workspace, cx);
                            cx.notify();
                        });
                        this.show_notice(
                            crate::toast::ToastKind::Success,
                            "Archived. Its cloud computer sleeps now and is deleted after 7 days; restore the chat from Settings → Archived.",
                            cx,
                        );
                    }
                    Err(error) => {
                        this.state.update(cx, |state, cx| {
                            if state.fail_cloud_sidebar_archive(&workspace) {
                                cx.notify();
                            }
                        });
                        this.show_notice(
                            crate::toast::ToastKind::Error,
                            format!("Couldn’t archive this cloud chat. {error}"),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    /// A cloud task drawn exactly like a local chat row: the project's
    /// avatar, its title and age, and the same activity corner. Only the
    /// click target differs.
    pub(super) fn render_cloud_sidebar_row(
        &self,
        row: &Value,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<(String, f32, AnyElement)> {
        let pending = row["pendingLaunch"].as_str().map(str::to_owned);
        if pending.is_none() {
            self.cloud_row_target(row, cx)?;
        }
        let workspace = row["workspaceId"].as_str()?.to_owned();
        let state = self.state.read(cx);
        let selected = match &pending {
            // The tab creating it is in front.
            Some(key) => {
                self.cloud_conversation.is_none()
                    && state.selected_chat.is_none()
                    && self.composer.read(cx).current_key == *key
            }
            None => {
                self.cloud_conversation
                    .as_ref()
                    .is_some_and(|view| view.read(cx).target.workspace == workspace)
                    || state
                        .engine()
                        .and_then(|engine| engine.cloud_workspace())
                        .as_deref()
                        == Some(workspace.as_str())
            }
        };
        let title = transcript::single_line(&zuse_protocol::file_mentions::mention_display_text(
            row["title"]
                .as_str()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or("New session"),
        ));
        let now = Utc::now();
        let updated = row["updatedAt"]
            .as_i64()
            .or_else(|| row["createdAt"].as_i64())
            .and_then(chrono::DateTime::from_timestamp_millis)
            .unwrap_or(now);
        let time_ago = zuse_protocol::view::format_time_ago(updated, now);
        let status = self.cloud_row_status(row, cx);
        let harness = self
            .settings
            .sidebar_show_harness
            .then(|| {
                self.cloud_conversation
                    .as_ref()
                    .map(|view| view.read(cx))
                    .filter(|view| view.target.workspace == workspace)
                    .and_then(|view| view.harness(Some(row)))
                    .or_else(|| super::cloud_conversation::cloud_harness(row))
                    .or_else(|| settings::remembered_cloud_agent(&workspace, cx))
            })
            .flatten();
        // Under a project or device heading, rows take the nested shape.
        let project_group = self.settings.sidebar_organization != SidebarOrganization::InOneList;
        let height = sidebar_row_height(
            self.sidebar_row_compact(project_group),
            self.sidebar_project_labels(),
            project_group,
        );
        let id = format!("{}{workspace}", super::CLOUD_ROW_PREFIX);
        // Under a project heading, the card names its branch, as local
        // cards do, while Branch is shown.
        let description = project_group.then(|| {
            ["branch", "branchName", "headBranch"]
                .into_iter()
                .filter(|_| self.settings.sidebar_show_branch)
                .find_map(|key| row[key].as_str().map(str::trim))
                .unwrap_or_default()
                .to_owned()
        });
        let project = description.unwrap_or_else(|| self.cloud_row_project_name(row, cx));
        let element = self.render_chat_row(
            id.clone(),
            title.into(),
            time_ago.into(),
            project.into(),
            None,
            None,
            harness,
            status,
            selected,
            false,
            false,
            None,
            None,
            self.settings.sidebar_show_project_icon && !project_group,
            project_group,
            None,
            theme,
            cx,
        );
        Some((id, height, element))
    }
    /// A cloud chat row's status. Starting and resuming read as activity,
    /// like a local run; an idle or sleeping computer shows its age, like
    /// any idle chat. The chat's live status knows more than the last list
    /// read, whichever chat is open: its agent mid-turn spins, like a local
    /// chat's row, a turn waiting on the user asks for input, a turn that
    /// finished out of sight reads done until the chat is opened, and a
    /// computer seen online is idle even while the listed row still reads
    /// starting.
    pub(super) fn cloud_row_status(&self, row: &Value, cx: &App) -> zuse_protocol::ChatIndicator {
        use zuse_protocol::ChatIndicator;
        let activity = row["workspaceId"]
            .as_str()
            .filter(|_| row["pendingLaunch"].is_null())
            .map(|workspace| self.state.read(cx).cloud_chat_activity(workspace))
            .unwrap_or_default();
        let live = activity.live.unwrap_or_default();
        let settled = if activity.unseen {
            ChatIndicator::Completed
        } else {
            ChatIndicator::Idle
        };
        match row["state"].as_str() {
            _ if live.working => ChatIndicator::Working,
            _ if live.awaiting_input => ChatIndicator::AwaitingInput,
            _ if live.online => settled,
            Some("ready" | "paused" | "pausing" | "archived") => settled,
            Some("failed" | "error" | "blocked") => ChatIndicator::Errored,
            _ => ChatIndicator::Working,
        }
    }
    #[cfg(test)]
    pub(super) fn pending_launch_rows(&self, cx: &App) -> Vec<Value> {
        self.cloud_sidebar_groups(cx)
            .into_iter()
            .flat_map(|group| group.rows)
            .filter(|row| row["pendingLaunch"].is_string())
            .collect()
    }
    pub(super) fn cloud_row_project_name(&self, row: &Value, cx: &App) -> String {
        let state = self.state.read(cx);
        let project_id = row["projectId"].as_str().unwrap_or_default();
        state
            .cloud_sidebar_projects()
            .into_iter()
            .find(|project| project["projectId"].as_str() == Some(project_id))
            .and_then(|project| {
                zuse_protocol::presentation::cloud_project_group(&project, &state.spaces)
                    .map(|group| group.name)
            })
            .unwrap_or_else(|| "Cloud".into())
    }
}

/// Sidebar ids of new chats still being created: `cloud:launch:<draft key>`.
pub(super) const PENDING_LAUNCH_PREFIX: &str = "launch:";
/// The sidebar row of a new chat before the server lists it: its first
/// message as the title, starting (or failed) like a starting computer.
fn pending_launch_row(launch: &crate::composer::cloud::CloudLaunchSummary, now: i64) -> Value {
    let started = launch
        .started_at
        .map(|at| now - at.elapsed().as_millis() as i64)
        .unwrap_or(now);
    serde_json::json!({
        "workspaceId": format!("{PENDING_LAUNCH_PREFIX}{}", launch.key),
        "pendingLaunch": launch.key,
        "projectId": launch.project_id,
        "title": launch.first_message,
        "agent": launch.harness,
        "state": if launch.failed { "failed" } else { "provisioning" },
        "createdAt": started,
        "updatedAt": started,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use serde_json::json;

    struct Pending;
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Pending {
        async fn handle(
            &self,
            _: &str,
            _: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                futures::stream::pending(),
            )))
        }
    }
    struct ProjectList {
        shell: Entity<Shell>,
        _directory: tempfile::TempDir,
    }
    impl Render for ProjectList {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.shell.update(cx, |shell, cx| {
                let rows = shell.render_active_rows(&Theme::of(cx).clone(), cx);
                div()
                    .w(px(320.))
                    .flex()
                    .flex_col()
                    .children(rows.rows.into_iter().map(|(_, _, row)| row))
            })
        }
    }
    /// The owner's case: the sidebar project "zuse" is both a local checkout
    /// and a Cloud repository. A new chat in it must be a cloud chat, with
    /// the checkout kept as the Local alternative.
    #[gpui::test]
    fn new_chat_in_a_cloud_backed_project_starts_on_cloud(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
                    state.set_cloud_subscription_for_test(true);
                    assert!(state.apply_cloud_sidebar_catalog(
                        &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[{"projectId":"project-zuse","displayName":"opcodehq/zuse","repositoryIdentity":"github.com/opcodehq/zuse","canCreate":true}]}),
                        &json!({"accountId":"owner","organizationId":null,"workspaces":[]})
                    ));
                    state.spaces.push(zuse_protocol::Space {
                        id: "space-zuse".into(),
                        device_id: "device".into(),
                        path: "/work/zuse".into(),
                        name: None,
                        git_detected: true,
                        git_checked_at: None,
                        checkout_id: None,
                        repository_id: None,
                        remote_repository: Some("github.com/OpcodeHQ/zuse".into()),
                        created_at: chrono::Utc::now(),
                    });
                    state
                });
                state.update(cx, |state, _| state.set_test_engine(controller));
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        target: None,
                        data_dir: directory.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zuse_protocol::HarnessId::Mock,
                    },
                    cx,
                );
                shell.reduced_motion = true;
                shell.settings.space_filter = Some("space-zuse".into());
                shell.open_new_session(None, cx);
                shell
            });
            ProjectList {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        shell.read_with(cx, |shell, cx| {
            let state = shell.state.read(cx);
            assert_eq!(
                state.cloud_project.as_ref().expect("a cloud chat")["projectId"],
                "project-zuse"
            );
            assert_eq!(state.selected_space.as_deref(), Some("space-zuse"));
        });
    }

    #[gpui::test]
    fn typed_cloud_workspaces_paint_under_project_and_pencil_opens_current_canvas(
        cx: &mut TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        let (host,cx)=cx.add_window_view(|_,cx| {
            let shell=cx.new(|cx| {
                let state=cx.new(|_| {
                    let mut state=AppState::new();
                    state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
                    let rows=(1..=2).map(|n| {
                        // Exactly the controller LIST decode/re-encode path that
                        // previously discarded projectId before sidebar grouping.
                        let row:zuse_model::cloud::Workspace=serde_json::from_value(json!({
                            "workspaceId":format!("task-{n}"),"projectId":"project","providerId":"boxd",
                            "chatId":format!("session-{n}"),"initialSessionId":format!("session-{n}"),
                            "title":format!("Saved cloud chat {n}"),"branch":format!("task/branch-{n}"),
                            "state":"paused","desiredState":"paused","statusCode":"paused","revision":1,
                            "rpcProtocol":"zuse-rpc-v1","createdAt":n,"updatedAt":n
                        })).unwrap();serde_json::to_value(row).unwrap()
                    }).collect::<Vec<_>>();
                    assert!(state.apply_cloud_sidebar_catalog(
                        &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[{"projectId":"project","displayName":"Cloud repo","repositoryIdentity":"github.com/owner/repo","canCreate":true}]}),
                        &json!({"accountId":"owner","organizationId":null,"workspaces":rows})));
                    state
                });
                state.update(cx,|state,_|state.set_test_engine(controller));
                let mut shell=Shell::new(state,EngineBootConfig {
                    target:None,data_dir:directory.path().into(),ipc_port:0,edge_url:"http://127.0.0.1:1".into(),
                    edge_token:None,org_id:None,workos_client_id:None,default_harness:zuse_protocol::HarnessId::Mock,
                },cx);
                shell.reduced_motion=true;
                shell.settings.sidebar_organization=SidebarOrganization::ByProject;
                shell.sidebar_collapsed_groups.clear();
                let project=shell.state.read(cx).cloud_sidebar_projects()[0].clone();
                shell.open_new_cloud_session(project,cx);
                let groups=shell.cloud_sidebar_groups(cx);
                assert_eq!(groups.len(),1);assert_eq!(groups[0].rows.len(),2);
                assert_eq!(groups[0].rows[0]["workspaceId"],"task-2");
                shell
            });
            ProjectList {shell,_directory:directory}
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("chat-cloud:task-1").is_some());
        assert!(cx.debug_bounds("chat-cloud:task-2").is_some());
        let pencil = cx
            .debug_bounds("sidebar-group-new-chat-project:cloud-project:project")
            .expect("cloud project must have the same pencil action as a local project");
        let composer = shell.read_with(cx, |shell, _| shell.composer.entity_id());
        cx.simulate_mouse_move(pencil.center(), None, gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_click(pencil.center(), gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert!(matches!(shell.route, Route::Chat));
            assert!(shell.cloud_conversation.is_none());
            assert_eq!(
                shell.composer.entity_id(),
                composer,
                "project pencil must reuse the current composer"
            );
            assert_eq!(
                shell.state.read(cx).cloud_project.as_ref().unwrap()["projectId"],
                "project"
            );
        });
    }

    /// The owner's account: two local checkouts of `opcodehq/zuse`, Cloud
    /// projects for `opcodehq/zuse` and `swarajbachu/zuse` (same short name)
    /// plus four others, and two ready cloud chats in `opcodehq/zuse`.
    fn owner_account_sidebar(
        cx: &mut TestAppContext,
    ) -> (Entity<Shell>, &mut gpui::VisualTestContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    state.workspace_scope = Some(zuse_protocol::WorkspaceScope::Local);
                    state.local_device_id = Some("mac".into());
                    state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
                    state.set_cloud_subscription_for_test(true);
                    let project = |id: &str, repo: &str| {
                        json!({
                            "projectId": id, "displayName": repo,
                            "repositoryIdentity": format!("github.com/{repo}"),
                            "repositoryUrl": format!("https://github.com/{repo}"),
                            "canCreate": true,
                        })
                    };
                    let workspace = |id: &str, title: &str, at: i64| {
                        let row: zuse_model::cloud::Workspace = serde_json::from_value(json!({
                            "workspaceId": id, "projectId": "project_3d6bff4de5a34ef5b345c71dd9686801",
                            "providerId": "boxd", "chatId": format!("{id}-thread"),
                            "initialSessionId": format!("{id}-session"), "title": title, "branch": format!("cloud/{id}"),
                            "state": "ready", "desiredState": "ready", "statusCode": "ready",
                            "revision": 1, "rpcProtocol": "zuse-rpc-v1", "createdAt": at, "updatedAt": at,
                        }))
                        .unwrap();
                        serde_json::to_value(row).unwrap()
                    };
                    assert!(state.apply_cloud_sidebar_catalog(
                        &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[
                            project("project_92e8e07c", "swarajbachu/zuse"),
                            project("project_3d6bff4de5a34ef5b345c71dd9686801", "opcodehq/zuse"),
                            project("project_review", "opcodehq/zuse-review"),
                            project("project_zkit", "opcodehq/zkit"),
                            project("project_delulu", "opcodehq/delulu"),
                            project("project_brokeboy", "opcodehq/brokeboy"),
                        ]}),
                        &json!({"accountId":"owner","organizationId":null,"workspaces":[
                            workspace("workspace_37ce54a9", "nice finally we can work on cloud", 2),
                            workspace("workspace_6d270d98", "yo", 1),
                        ]})
                    ));
                    for (id, path) in [
                        ("space-startups", "/Users/owner/Developer/startups/zuse"),
                        ("space-token-maxing", "/Users/owner/Developer/token-maxing/zuse"),
                    ] {
                        state.spaces.push(zuse_protocol::Space {
                            id: id.into(),
                            device_id: "mac".into(),
                            path: path.into(),
                            name: None,
                            git_detected: true,
                            git_checked_at: None,
                            checkout_id: None,
                            repository_id: Some("github.com/opcodehq/zuse".into()),
                            remote_repository: Some("https://github.com/opcodehq/zuse.git".into()),
                            created_at: chrono::Utc::now(),
                        });
                    }
                    state.chats = vec![
                        serde_json::from_value(json!({
                            "id":"local-chat","deviceId":"mac","spaceId":"space-startups",
                            "title":"Debug Image Issue","archived":false,
                            "createdAt":"2026-11-19T12:00:00Z"
                        }))
                        .unwrap(),
                    ];
                    state
                });
                state.update(cx, |state, _| state.set_test_engine(controller));
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        target: None,
                        data_dir: directory.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zuse_protocol::HarnessId::Mock,
                    },
                    cx,
                );
                shell.reduced_motion = true;
                shell.settings.sidebar_organization = SidebarOrganization::ByProject;
                shell.sidebar_collapsed_groups.clear();
                shell
            });
            ProjectList {
                shell,
                _directory: directory,
            }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        (shell, cx)
    }

    /// Every listed cloud chat renders under some project, and a local
    /// checkout joins its Cloud repository by GitHub identity, never by its
    /// short name: `opcodehq/zuse`'s chats sit in the local "zuse" project,
    /// `swarajbachu/zuse` stays its own project labelled with its owner.
    #[gpui::test]
    fn owner_account_cloud_chats_render_under_their_repository(cx: &mut TestAppContext) {
        let (shell, cx) = owner_account_sidebar(cx);
        cx.update(|window, cx| window.draw(cx).clear());
        for workspace in [
            "chat-cloud:workspace_37ce54a9",
            "chat-cloud:workspace_6d270d98",
        ] {
            assert!(
                cx.debug_bounds(workspace).is_some(),
                "ready cloud chat {workspace} was dropped from the sidebar"
            );
        }
        shell.read_with(cx, |shell, cx| {
            let groups = shell.cloud_sidebar_groups(cx);
            // No listed workspace is left without a group.
            let shown: usize = groups.iter().map(|group| group.rows.len()).sum();
            assert_eq!(shown, shell.state.read(cx).cloud_sidebar_workspaces().len());
            let group = |id: &str| {
                groups
                    .iter()
                    .find(|g| g.project["projectId"] == id)
                    .and_then(|g| g.group.clone())
                    .unwrap()
            };
            let local_key = zuse_protocol::view::project_key(&shell.state.read(cx).spaces[0]);
            let zuse = group("project_3d6bff4de5a34ef5b345c71dd9686801");
            assert_eq!(zuse.0, local_key, "opcodehq/zuse joins the local checkout");
            let fork = group("project_92e8e07c");
            assert_ne!(fork.0, local_key);
            assert_eq!(fork.1, "swarajbachu/zuse");
            assert_eq!(zuse.1, "opcodehq/zuse");
            assert_eq!(group("project_review").1, "zuse-review");
        });
    }

    /// Records archive requests and acknowledges them like the controller.
    #[derive(Default)]
    struct Archiver {
        calls: std::sync::Mutex<Vec<Value>>,
        /// Refuse every archive, like a controller that could not reach Cloud.
        fail: bool,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Archiver {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            if method == zuse_rpc::methods::CLOUD_ARCHIVE {
                self.calls.lock().unwrap().push(params.clone());
                if self.fail {
                    return Err(zuse_rpc::RpcError::Failed("Cloud is unreachable.".into()));
                }
                return zuse_rpc::RpcReply::value(&json!({
                    "accountId":"owner","organizationId":null,
                    "workspace":{"workspaceId":params["workspaceId"],"state":"archiving","desiredState":"archived"}
                }));
            }
            Ok(zuse_rpc::RpcReply::Stream(Box::pin(
                futures::stream::pending(),
            )))
        }
    }
    /// The memory controller runs on tokio: drive it and gpui until `calls`
    /// archive requests were answered.
    fn settle(
        runtime: &tokio::runtime::Runtime,
        archiver: &Archiver,
        calls: usize,
        cx: &mut gpui::VisualTestContext,
    ) {
        for _ in 0..200 {
            cx.run_until_parked();
            runtime.block_on(tokio::time::sleep(std::time::Duration::from_millis(5)));
            cx.run_until_parked();
            if archiver.calls.lock().unwrap().len() >= calls {
                break;
            }
        }
        for _ in 0..5 {
            runtime.block_on(tokio::task::yield_now());
            cx.run_until_parked();
        }
    }
    #[gpui::test]
    fn organization_cloud_rows_are_not_offered_archive(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let archiver = std::sync::Arc::new(Archiver::default());
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(archiver.clone()));
        let (host,cx)=cx.add_window_view(|_,cx| {
            let shell=cx.new(|cx| {
                let state=cx.new(|_| {
                    let mut state=AppState::new();
                    state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":"org-team"}));
                    let rows=(1..=2).map(|n| json!({
                        "workspaceId":format!("task-{n}"),"projectId":"project","providerId":"boxd",
                        "chatId":format!("session-{n}"),"initialSessionId":format!("session-{n}"),
                        "title":format!("Cloud chat {n}"),"state":"paused","desiredState":"paused",
                        "createdAt":n,"updatedAt":n
                    })).collect::<Vec<_>>();
                    assert!(state.apply_cloud_sidebar_catalog(
                        &json!({"accountId":"owner","organizationId":"org-team","ownerId":"organization:org-team","projects":[{"projectId":"project","displayName":"Cloud repo","repositoryIdentity":"github.com/owner/repo","canCreate":true}]}),
                        &json!({"accountId":"owner","organizationId":"org-team","workspaces":rows})));
                    state
                });
                state.update(cx,|state,_|state.set_test_engine(controller));
                let mut shell=Shell::new(state,EngineBootConfig {
                    target:None,data_dir:directory.path().into(),ipc_port:0,edge_url:"http://127.0.0.1:1".into(),
                    edge_token:None,org_id:None,workos_client_id:None,default_harness:zuse_protocol::HarnessId::Mock,
                },cx);
                shell.reduced_motion=true;
                shell.settings.sidebar_organization=SidebarOrganization::ByProject;
                shell.sidebar_collapsed_groups.clear();
                shell
            });
            ProjectList {shell,_directory:directory}
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        // Settings → Archived lists personal computers only, so an
        // organization task offers no Archive menu and refuses to archive.
        let row = cx.debug_bounds("chat-cloud:task-1").expect("cloud row");
        cx.simulate_event(gpui::MouseDownEvent {
            position: row.center(),
            button: gpui::MouseButton::Right,
            modifiers: gpui::Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        shell.read_with(cx, |shell, _| {
            assert!(!shell.chat_menu_targets("cloud:task-1"));
        });
        shell.update(cx, |shell, cx| {
            shell.archive_cloud_sidebar_row("cloud:task-1", cx)
        });
        settle(&runtime, &archiver, 1, cx);
        assert!(archiver.calls.lock().unwrap().is_empty());
        shell.read_with(cx, |shell, cx| {
            assert_eq!(shell.state.read(cx).cloud_sidebar_workspaces().len(), 2);
        });
    }
    #[gpui::test]
    fn cloud_rows_archive_from_their_menu_and_hover_pill(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let archiver = std::sync::Arc::new(Archiver::default());
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(archiver.clone()));
        let (host,cx)=cx.add_window_view(|_,cx| {
            let shell=cx.new(|cx| {
                let state=cx.new(|_| {
                    let mut state=AppState::new();
                    state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
                    let rows=(1..=2).map(|n| json!({
                        "workspaceId":format!("task-{n}"),"projectId":"project","providerId":"boxd",
                        "chatId":format!("session-{n}"),"initialSessionId":format!("session-{n}"),
                        "title":format!("Cloud chat {n}"),"state":"paused","desiredState":"paused",
                        "createdAt":n,"updatedAt":n
                    })).collect::<Vec<_>>();
                    assert!(state.apply_cloud_sidebar_catalog(
                        &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[{"projectId":"project","displayName":"Cloud repo","repositoryIdentity":"github.com/owner/repo","canCreate":true}]}),
                        &json!({"accountId":"owner","organizationId":null,"workspaces":rows})));
                    state
                });
                state.update(cx,|state,_|state.set_test_engine(controller));
                let mut shell=Shell::new(state,EngineBootConfig {
                    target:None,data_dir:directory.path().into(),ipc_port:0,edge_url:"http://127.0.0.1:1".into(),
                    edge_token:None,org_id:None,workos_client_id:None,default_harness:zuse_protocol::HarnessId::Mock,
                },cx);
                shell.reduced_motion=true;
                shell.settings.sidebar_organization=SidebarOrganization::ByProject;
                shell.sidebar_collapsed_groups.clear();
                shell
            });
            ProjectList {shell,_directory:directory}
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        // Right-click opens the row menu for the cloud task (Archive only).
        let row = cx.debug_bounds("chat-cloud:task-1").expect("cloud row");
        cx.simulate_event(gpui::MouseDownEvent {
            position: row.center(),
            button: gpui::MouseButton::Right,
            modifiers: gpui::Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        shell.read_with(cx, |shell, _| {
            assert!(shell.chat_menu_targets("cloud:task-1"));
        });
        shell.update(cx, |shell, cx| {
            shell.archive_cloud_sidebar_row("cloud:task-1", cx);
            assert!(!shell.chat_menu.is_open(), "archiving closes the menu");
            // A second request while the first is in flight is ignored.
            shell.archive_cloud_sidebar_row("cloud:task-1", cx);
        });
        settle(&runtime, &archiver, 1, cx);
        {
            let calls = archiver.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0]["workspaceId"], "task-1");
            assert_eq!(calls[0]["expectedAccountId"], "owner");
            assert!(
                calls[0]["commandId"]
                    .as_str()
                    .is_some_and(|id| !id.is_empty())
            );
        }
        shell.read_with(cx, |shell, cx| {
            let rows = shell.state.read(cx).cloud_sidebar_workspaces();
            assert_eq!(rows.len(), 1, "the archived task leaves Threads");
            assert_eq!(rows[0]["workspaceId"], "task-2");
        });
        // Hovering the remaining row offers the same Archive pill as a local chat.
        cx.update(|window, cx| window.draw(cx).clear());
        let row = cx.debug_bounds("chat-cloud:task-2").expect("cloud row");
        cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        shell.read_with(cx, |shell, _| {
            assert_eq!(
                shell.chat_status_hover.as_deref(),
                Some("chat-cloud:task-2"),
                "cloud rows swap their corner for the archive pill on hover"
            );
        });
        let pill = cx
            .debug_bounds("chat-archive-cloud:task-2")
            .expect("cloud rows show Archive on hover");
        cx.simulate_click(pill.center(), gpui::Modifiers::default());
        settle(&runtime, &archiver, 2, cx);
        assert_eq!(archiver.calls.lock().unwrap().len(), 2);
        assert_eq!(archiver.calls.lock().unwrap()[1]["workspaceId"], "task-2");
        shell.read_with(cx, |shell, cx| {
            assert!(shell.state.read(cx).cloud_sidebar_workspaces().is_empty());
            assert!(
                shell.cloud_conversation.is_none(),
                "archiving does not open the task"
            );
        });
    }
    #[gpui::test]
    fn a_failed_cloud_archive_brings_the_row_back_with_an_error(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(UiSettings::default(), directory.path(), cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let archiver = std::sync::Arc::new(Archiver {
            fail: true,
            ..Default::default()
        });
        let controller =
            crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(archiver.clone()));
        let (host,cx)=cx.add_window_view(|_,cx| {
            let shell=cx.new(|cx| {
                let state=cx.new(|_| {
                    let mut state=AppState::new();
                    state.apply_cloud_account(json!({"user":{"id":"owner"},"orgId":null}));
                    assert!(state.apply_cloud_sidebar_catalog(
                        &json!({"accountId":"owner","organizationId":null,"ownerId":"owner","projects":[{"projectId":"project","displayName":"Cloud repo","repositoryIdentity":"github.com/owner/repo","canCreate":true}]}),
                        &json!({"accountId":"owner","organizationId":null,"workspaces":[{
                            "workspaceId":"task-1","projectId":"project","providerId":"boxd",
                            "chatId":"session-1","initialSessionId":"session-1",
                            "title":"Cloud chat 1","state":"paused","desiredState":"paused",
                            "createdAt":1,"updatedAt":1
                        }]})));
                    state
                });
                state.update(cx,|state,_|state.set_test_engine(controller));
                let mut shell=Shell::new(state,EngineBootConfig {
                    target:None,data_dir:directory.path().into(),ipc_port:0,edge_url:"http://127.0.0.1:1".into(),
                    edge_token:None,org_id:None,workos_client_id:None,default_harness:zuse_protocol::HarnessId::Mock,
                },cx);
                shell.reduced_motion=true;
                shell.settings.sidebar_organization=SidebarOrganization::ByProject;
                shell.sidebar_collapsed_groups.clear();
                shell
            });
            ProjectList {shell,_directory:directory}
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        cx.update(|window, cx| window.draw(cx).clear());
        // The row leaves the moment Archive is clicked, before any reply.
        shell.update(cx, |shell, cx| {
            shell.archive_cloud_sidebar_row("cloud:task-1", cx);
            assert!(shell.state.read(cx).cloud_sidebar_workspaces().is_empty());
        });
        settle(&runtime, &archiver, 1, cx);
        assert_eq!(archiver.calls.lock().unwrap().len(), 1);
        shell.read_with(cx, |shell, cx| {
            let rows = shell.state.read(cx).cloud_sidebar_workspaces();
            assert_eq!(rows.len(), 1, "a failed archive puts the row back");
            assert_eq!(rows[0]["workspaceId"], "task-1");
            assert!(
                shell
                    .sidebar_notice
                    .as_deref()
                    .is_some_and(|notice| notice.starts_with("Couldn’t archive this cloud chat.")),
                "the failure is reported: {:?}",
                shell.sidebar_notice
            );
            assert!(shell.cloud_archiving.is_empty());
        });
    }

    /// The whole chat sidebar, as the app draws it (including the per-render
    /// hover bookkeeping the row lists alone skip).
    struct FullSidebar(Entity<Shell>);
    impl Render for FullSidebar {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0.update(cx, |shell, cx| {
                let sidebar = shell.render_chat_sidebar(&Theme::default(), cx);
                let overlays = if shell.chat_menu.get().is_some() {
                    shell.render_overlays(window.viewport_size(), window, cx)
                } else {
                    Vec::new()
                };
                div()
                    .w(px(300.0))
                    .h(px(900.0))
                    .child(sidebar)
                    .children(overlays)
            })
        }
    }

    /// A sidebar holding a pinned local chat, a local chat under a project
    /// and a cloud task, for a personal account or an organization.
    fn mixed_sidebar<'a>(
        cx: &'a mut TestAppContext,
        runtime: &tokio::runtime::Runtime,
        directory: &std::path::Path,
        organization: Option<&'static str>,
    ) -> (Entity<Shell>, &'a mut gpui::VisualTestContext) {
        let data_dir = directory.to_path_buf();
        cx.update(|cx| {
            gpui_base::init(cx);
            gpui_tokio::init_from_handle(cx, runtime.handle().clone());
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            settings::init(UiSettings::default(), &data_dir, cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let controller = crate::state::EngineHandle::from_test_client(zuse_rpc::memory_client(
            std::sync::Arc::new(Pending),
        ));
        let (host, cx) = cx.add_window_view(|_, cx| {
            FullSidebar(cx.new(|cx| {
                let state = cx.new(|_| {
                    let mut state = AppState::new();
                    state.workspace_scope = Some(zuse_protocol::WorkspaceScope::Local);
                    state.local_device_id = Some("local".into());
                    state.apply_cloud_account(
                        json!({"user":{"id":"owner"},"orgId":organization}),
                    );
                    let owner = organization
                        .map(|org| format!("organization:{org}"))
                        .unwrap_or_else(|| "owner".into());
                    assert!(state.apply_cloud_sidebar_catalog(
                        &json!({"accountId":"owner","organizationId":organization,"ownerId":owner,
                            "projects":[{"projectId":"project","displayName":"Hello-World","repositoryIdentity":"github.com/owner/hello-world","canCreate":true}]}),
                        &json!({"accountId":"owner","organizationId":organization,"workspaces":[{
                            "workspaceId":"task-1","projectId":"project","providerId":"boxd",
                            "chatId":"session-1","initialSessionId":"session-1",
                            "title":"Cloud chat","agent":"codex","branch":"task/cloud","state":"ready","desiredState":"ready",
                            "createdAt":1,"updatedAt":1
                        }]})
                    ));
                    state.spaces = vec![
                        serde_json::from_value(json!({
                            "id":"space-1","deviceId":"local","path":"/projects/zuse",
                            "createdAt":Utc::now()
                        }))
                        .unwrap(),
                    ];
                    state.chats = ["local-pin", "local-1"]
                        .into_iter()
                        .map(|id| {
                            serde_json::from_value(json!({
                                "id":id,"title":id,"deviceId":"local","archived":false,
                                "spaceId":"space-1","createdAt":Utc::now()
                            }))
                            .unwrap()
                        })
                        .collect();
                    state
                });
                state.update(cx, |state, _| state.set_test_engine(controller));
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        target: None,
                        data_dir: data_dir.clone(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zuse_protocol::HarnessId::Mock,
                    },
                    cx,
                );
                shell.reduced_motion = true;
                shell
                    .settings
                    .sidebar_pins_mut("local".into())
                    .push("local-pin".into());
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());
        (shell, cx)
    }

    /// Hover each row variant in each sidebar layout and read what its
    /// trailing slot and cloud mark show.
    fn assert_rows_offer_archive(
        shell: &Entity<Shell>,
        cx: &mut gpui::VisualTestContext,
        organization: bool,
    ) {
        for layout in [
            SidebarOrganization::ByProject,
            SidebarOrganization::InOneList,
            SidebarOrganization::ByDevice,
        ] {
            shell.update(cx, |shell, cx| {
                shell.settings.sidebar_organization = layout;
                shell.sidebar_collapsed_groups.clear();
                shell.pinned_open = true;
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
            // `debug_bounds` takes static selectors.
            for (id, row_selector, archive, time, mark) in [
                (
                    "local-pin",
                    "chat-local-pin",
                    "chat-archive-local-pin",
                    "chat-time-local-pin",
                    "chat-cloud-mark-local-pin",
                ),
                (
                    "local-1",
                    "chat-local-1",
                    "chat-archive-local-1",
                    "chat-time-local-1",
                    "chat-cloud-mark-local-1",
                ),
                (
                    "cloud:task-1",
                    "chat-cloud:task-1",
                    "chat-archive-cloud:task-1",
                    "chat-time-cloud:task-1",
                    "chat-cloud-mark-cloud:task-1",
                ),
            ] {
                let cloud = id.starts_with(CLOUD_ROW_PREFIX);
                let row = cx
                    .debug_bounds(row_selector)
                    .unwrap_or_else(|| panic!("{id} paints in {layout:?}"));
                // Checked at rest: a hovered card swaps its marks for actions.
                // Under a machine heading the heading names the cloud instead.
                let marked = cloud && layout != SidebarOrganization::ByDevice;
                assert_eq!(
                    cx.debug_bounds(mark).is_some(),
                    marked,
                    "{id} in {layout:?}: cloud mark only on cloud rows"
                );
                cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
                cx.run_until_parked();
                cx.update(|window, cx| window.draw(cx).clear());
                cx.update(|window, cx| window.draw(cx).clear());
                let offered = !(organization && cloud);
                assert_eq!(
                    cx.debug_bounds(archive).is_some(),
                    offered,
                    "{id} in {layout:?}: hover Archive shown = {offered}"
                );
                if !offered {
                    assert!(
                        cx.debug_bounds(time).is_some(),
                        "a row without an archive action keeps its age"
                    );
                }
            }
        }
    }

    #[gpui::test]
    fn thread_more_opens_actions_without_selecting_the_row(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        let (shell, cx) = mixed_sidebar(cx, &runtime, directory.path(), None);
        let redraw = |cx: &mut gpui::VisualTestContext| {
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
            cx.update(|window, cx| window.draw(cx).clear());
        };
        redraw(cx);
        let row = cx.debug_bounds("chat-cloud:task-1").unwrap();
        cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
        redraw(cx);
        let more = cx.debug_bounds("chat-more-cloud:task-1").unwrap();
        let archive = cx.debug_bounds("chat-archive-cloud:task-1").unwrap();
        assert!(archive.right() < more.left());
        cx.simulate_click(more.center(), gpui::Modifiers::default());
        redraw(cx);
        for item in [
            "chat-menu-share",
            "chat-menu-new-tab",
            "chat-menu-rename",
            "chat-menu-archive",
            "chat-menu-delete",
        ] {
            assert!(cx.debug_bounds(item).is_some(), "missing {item}");
        }
        shell.read_with(cx, |shell, _| assert!(shell.cloud_conversation.is_none()));
        cx.simulate_click(more.center(), gpui::Modifiers::default());
        redraw(cx);
        shell.read_with(cx, |shell, _| assert!(!shell.chat_menu.is_open()));

        shell.update(cx, |shell, cx| shell.open_chat("local-pin".into(), cx));
        redraw(cx);
        let local = cx.debug_bounds("chat-local-1").unwrap();
        cx.simulate_mouse_move(local.center(), None, gpui::Modifiers::default());
        redraw(cx);
        let local_more = cx.debug_bounds("chat-more-local-1").unwrap();
        cx.simulate_click(local_more.center(), gpui::Modifiers::default());
        redraw(cx);
        let open_tab = cx.debug_bounds("chat-menu-new-tab").unwrap();
        cx.simulate_click(open_tab.center(), gpui::Modifiers::default());
        redraw(cx);
        shell.update(cx, |shell, cx| {
            assert_eq!(
                shell.state.read(cx).selected_chat.as_deref(),
                Some("local-1")
            );
            assert!(shell.can_close_sidebar_thread_tab("local-1", cx));
            shell.close_thread_conversation("local-1", cx);
            assert_eq!(
                shell.state.read(cx).selected_chat.as_deref(),
                Some("local-pin")
            );
        });
    }

    #[gpui::test]
    fn a_merged_pull_request_leaves_its_thread_in_the_sidebar(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        let (shell, cx) = mixed_sidebar(cx, &runtime, directory.path(), None);
        let branch = "feature/merged";
        let now = Utc::now();
        shell.update(cx, |shell, cx| {
            shell.settings.sidebar_show_pull_request = false;
            shell.state.update(cx, |state, cx| {
                let chat = state
                    .chats
                    .iter_mut()
                    .find(|chat| chat.id == "local-pin")
                    .unwrap();
                chat.source_context = Some(zuse_protocol::ConversationSourceContext {
                    checkout_id: "checkout".into(),
                    repo_root: "/projects/zuse".into(),
                    cwd: "/projects/zuse".into(),
                    branch: branch.into(),
                    head_sha: None,
                    observed_at: now,
                });
                state.fixture_sidebar_change_request(zuse_protocol::CheckoutChangeRequestStatus {
                    checkout_id: "checkout".into(),
                    device_id: "local".into(),
                    cwd: "/projects/zuse".into(),
                    branch: branch.into(),
                    error: None,
                    updated_at: now,
                    change_request: Some(zuse_protocol::ChangeRequestSummary {
                        provider: "github".into(),
                        number: 1,
                        title: "Merged change".into(),
                        url: "https://github.com/owner/zuse/pull/1".into(),
                        state: zuse_protocol::ChangeRequestState::Merged,
                        base_ref: "main".into(),
                        head_ref: branch.into(),
                        is_draft: false,
                        review_decision: Default::default(),
                    }),
                });
                cx.notify();
            });
            assert!(
                shell
                    .sidebar_visible_order(cx)
                    .contains(&"local-pin".into())
            );
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("chat-local-pin").is_some());
    }

    #[gpui::test]
    fn project_filter_menu_opens_beside_its_icon(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        let (shell, cx) = mixed_sidebar(cx, &runtime, directory.path(), None);
        shell.update(cx, |shell, cx| {
            shell.settings.sidebar_organization = SidebarOrganization::InOneList;
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            shell.update(cx, |shell, cx| shell.open_spaces_menu(window, cx));
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        let button = cx
            .debug_bounds("spaces-filter-button")
            .expect("filter button");
        let row = cx
            .debug_bounds("spaces-menu-row-0")
            .expect("project menu row");
        let threads = cx.debug_bounds("chat-local-1").unwrap();
        assert!(
            row.left() > threads.center().x,
            "the project menu opens to the right, not over the list: {row:?} vs {threads:?}"
        );
        assert!(
            row.left() >= button.right() && row.left() - button.right() < px(16.0),
            "it opens right beside its icon: {row:?} vs {button:?}"
        );
        assert_eq!(row.size.height, px(30.0));
    }

    #[gpui::test]
    fn every_personal_row_offers_archive_on_hover_in_every_layout(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        let (shell, cx) = mixed_sidebar(cx, &runtime, directory.path(), None);
        assert_rows_offer_archive(&shell, cx, false);
    }

    #[gpui::test]
    fn organization_cloud_rows_show_no_archive_in_any_layout(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        let (shell, cx) = mixed_sidebar(cx, &runtime, directory.path(), Some("org-team"));
        assert_rows_offer_archive(&shell, cx, true);
    }

    #[gpui::test]
    fn resting_on_a_row_opens_its_details_card(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        let (shell, cx) = mixed_sidebar(cx, &runtime, directory.path(), None);
        shell.update(cx, |shell, cx| {
            shell.settings.sidebar_organization = SidebarOrganization::ByProject;
            shell.sidebar_collapsed_groups.clear();
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        let row = cx.debug_bounds("chat-cloud:task-1").expect("cloud row");
        cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(
            cx.debug_bounds("chat-peek-card").is_none(),
            "the card waits for the pointer to rest"
        );
        cx.executor()
            .advance_clock(crate::tooltip::CARD_DELAY + std::time::Duration::from_millis(50));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        let card = cx
            .debug_bounds("chat-peek-card")
            .expect("details card opens");
        assert!(
            card.left() >= row.right(),
            "the card opens beside the row, not over it"
        );
        let peek = shell.read_with(cx, |shell, cx| {
            shell.chat_peek("cloud:task-1", None, "1m", cx).unwrap()
        });
        assert_eq!(peek.title, "Cloud chat");
        assert_eq!(peek.project.as_deref(), Some("Hello-World"));
        assert_eq!(peek.branch.as_deref(), Some("task/cloud"));
        assert_eq!(peek.place_label(), "Cloud · Running");
        // Leaving the row closes it.
        cx.simulate_mouse_move(
            gpui::point(row.left() + px(1.0), row.bottom() + px(400.0)),
            None,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("chat-peek-card").is_none());
    }

    #[gpui::test]
    fn project_density_shapes_rows_under_project_headings(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let directory = tempfile::tempdir().unwrap();
        let (shell, cx) = mixed_sidebar(cx, &runtime, directory.path(), None);
        let heights = |shell: &Entity<Shell>,
                       cx: &mut gpui::VisualTestContext,
                       density: settings::SidebarDensity| {
            shell.update(cx, |shell, cx| {
                shell.settings.sidebar_organization = SidebarOrganization::ByProject;
                shell.settings.sidebar_project_density = density;
                shell.sidebar_collapsed_groups.clear();
                cx.notify();
            });
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear());
            ["chat-local-1", "chat-cloud:task-1"]
                .map(|row| f32::from(cx.debug_bounds(row).expect(row).size.height))
        };
        // Detailed, the default, draws cards; Compact draws one-line rows.
        assert_eq!(
            heights(&shell, cx, settings::SidebarDensity::Detailed),
            [super::super::SIDEBAR_ROW_DOUBLE_HEIGHT; 2]
        );
        assert_eq!(
            heights(&shell, cx, settings::SidebarDensity::Compact),
            [super::super::SIDEBAR_ROW_SINGLE_HEIGHT; 2]
        );
        // Compact rows name where they run just left of the age.
        for (row, corner, mark) in [
            ("local-1", "chat-time-local-1", "chat-computer-mark-local-1"),
            (
                "cloud:task-1",
                "chat-time-cloud:task-1",
                "chat-cloud-mark-cloud:task-1",
            ),
        ] {
            let mark = cx.debug_bounds(mark).expect(mark);
            let corner = cx.debug_bounds(corner).unwrap();
            assert!(
                corner.contains(&mark.center()),
                "{row}: mark sits beside the age"
            );
        }
        // Device groups draw the same rows, divided by machine; the heading
        // names the machine, so rows drop their own mark.
        shell.update(cx, |shell, cx| {
            shell.settings.sidebar_organization = SidebarOrganization::ByDevice;
            shell.sidebar_collapsed_groups.clear();
            cx.notify();
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        for (row, mark) in [
            ("chat-local-1", "chat-computer-mark-local-1"),
            ("chat-cloud:task-1", "chat-cloud-mark-cloud:task-1"),
        ] {
            assert_eq!(
                f32::from(cx.debug_bounds(row).expect(row).size.height),
                super::super::SIDEBAR_ROW_SINGLE_HEIGHT
            );
            assert!(cx.debug_bounds(mark).is_none(), "{mark}");
        }
        // Each machine's heading leads with the kind of machine it is.
        for heading in [
            "sidebar-device-icon-local",
            "sidebar-device-icon-cloud-computers",
        ] {
            assert!(cx.debug_bounds(heading).is_some(), "{heading}");
        }
        // Density belongs to Projects and Devices: the Threads view keeps cards.
        shell.update(cx, |shell, cx| {
            shell.settings.sidebar_organization = SidebarOrganization::InOneList;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear());
        assert_eq!(
            f32::from(cx.debug_bounds("chat-local-1").unwrap().size.height),
            super::super::SIDEBAR_ROW_DOUBLE_HEIGHT
        );
    }
}

impl Shell {
    pub(super) fn render_cloud_sidebar_status(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.state.read(cx);
        if state.cloud_user_id().is_none() {
            return None;
        }
        // Background refreshes stay silent: the rows are already on screen
        // and update in place. Only a failure the user can act on shows.
        let loading = false;
        let label = state.cloud_sidebar_error()?.to_owned();
        Some(
            div()
                .id("cloud-sidebar-sync-status")
                .debug_selector(|| "cloud-sidebar-sync-status".into())
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(12.))
                .py(px(5.))
                .text_size(px(11.))
                .text_color(theme.text_muted)
                .when(loading, |row| {
                    row.child(crate::loaders::mini_mono_spinner(
                        "cloud-sidebar-sync-spinner",
                        2.,
                        theme.text_muted,
                        cx.entity_id(),
                        cx,
                    ))
                })
                .child(div().flex_1().min_w_0().child(label))
                .child(
                    div()
                        .id("cloud-sidebar-retry")
                        .debug_selector(|| "cloud-sidebar-retry".into())
                        .cursor_pointer()
                        .child("Retry")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state
                                .update(cx, |state, cx| state.retry_cloud_sidebar(cx))
                        })),
                )
                .into_any_element(),
        )
    }
}
