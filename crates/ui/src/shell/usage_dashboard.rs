//! Recorded token usage for the main pane and Settings.
use super::*;
use zuse_viewport::usage_dashboard::{Event, Project, Properties, UsageDashboard};

impl Shell {
    pub(super) fn open_usage(
        &mut self,
        project: Option<String>,
        project_scope: bool,
        cx: &mut Context<Self>,
    ) {
        let project = project.or_else(|| self.state.read(cx).selected_space.clone());
        self.enter_usage(project.clone(), project_scope, cx);
        self.nav.push(NavEntry::Usage {
            project,
            project_scope,
        });
        self.close_space_menu(cx);
        self.close_user_menu(cx);
        cx.notify();
    }
    pub(super) fn enter_usage(
        &mut self,
        project: Option<String>,
        project_scope: bool,
        cx: &mut Context<Self>,
    ) {
        self.command_palette = None;
        self.set_voice_stage_open(false, cx);
        self.usage_project = project;
        self.usage_project_scope = project_scope;
        self.usage_pending = true;
        self.set_route(Route::Usage, cx);
    }
    fn usage_properties(&self, embedded: bool, cx: &App) -> Properties {
        let state = self.state.read(cx);
        let project = if embedded {
            state.selected_space_row()
        } else {
            self.usage_project
                .as_ref()
                .and_then(|id| state.spaces.iter().find(|s| &s.id == id))
        };
        let account = state.cloud_account.as_ref();
        Properties {
            available: state.engine().is_some(),
            owner: serde_json::json!([
                account.map(|a| &a["user"]["id"]),
                account.map(|a| &a["orgId"])
            ])
            .to_string(),
            target: project
                .map(|s| s.device_id.clone())
                .or_else(|| state.effective_device_id()),
            project: project.map(|s| Project {
                id: s.id.clone(),
                label: s.display_name().to_owned(),
            }),
        }
    }
    pub(super) fn hide_usage_dashboard(&self, cx: &mut Context<Self>) {
        if let Some(dashboard) = &self.usage_dashboard
            && dashboard.read(cx).visible()
        {
            dashboard.update(cx, |dashboard, cx| dashboard.hide(cx));
        }
    }
    pub(super) fn usage_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.usage_dashboard.is_none() {
            let dashboard = cx.new(|cx| UsageDashboard::new(window, cx));
            cx.subscribe(&dashboard, |this, dashboard, event, cx| match event {
                Event::ScopeChanged(project_scope) => {
                    if this.route != Route::Usage {
                        return;
                    }
                    this.usage_project_scope = *project_scope;
                    if matches!(this.nav.current(), NavEntry::Usage { .. }) {
                        this.nav.replace(NavEntry::Usage {
                            project: this.usage_project.clone(),
                            project_scope: *project_scope,
                        });
                    }
                }
                Event::Close => {
                    this.set_route(Route::Chat, cx);
                    this.focus_composer(cx);
                    this.nav.push(NavEntry::Chat(
                        this.state
                            .read(cx)
                            .selected_chat
                            .clone()
                            .unwrap_or_default(),
                    ));
                    cx.notify();
                }
                Event::Cancel(id) => {
                    this.usage_tasks.remove(id);
                }
                Event::Call { id, method, params } => {
                    let Some(engine) = this.state.read(cx).engine().map(|e| e.controller()) else {
                        dashboard.update(cx, |dashboard, cx| {
                            dashboard.receive(
                                *id,
                                Err("Computer unavailable. Reconnect and refresh usage.".into()),
                                cx,
                            )
                        });
                        return;
                    };
                    let id = *id;
                    let method = *method;
                    let params = params.clone();
                    let dashboard = dashboard.clone();
                    let job = gpui_tokio::Tokio::spawn(cx, async move {
                        engine
                            .client()
                            .call_scoped(method, params)
                            .await
                            .map_err(|e| e.to_string())
                    });
                    let task = cx.spawn(async move |this, cx| {
                        let result = job.await.unwrap_or_else(|e| Err(e.to_string()));
                        let _ = this.update(cx, |this, cx| {
                            this.usage_tasks.remove(&id);
                            dashboard.update(cx, |dashboard, cx| dashboard.receive(id, result, cx));
                        });
                    });
                    this.usage_tasks.insert(id, task);
                }
            })
            .detach();
            self.usage_dashboard = Some(dashboard);
        }
        let dashboard = self.usage_dashboard.clone().unwrap();
        let embedded = self.route == Route::Settings(SettingsSection::Usage);
        let properties = self.usage_properties(embedded, cx);
        let (was_embedded, visible) = dashboard.read_with(cx, |dashboard, _| {
            (dashboard.embedded(), dashboard.visible())
        });
        let reopen = !embedded && std::mem::take(&mut self.usage_pending);
        let open = !visible || was_embedded != embedded || reopen;
        let project_scope = !embedded && self.usage_project_scope;
        dashboard.update(cx, |dashboard, cx| {
            dashboard.set_embedded(embedded, cx);
            if open {
                dashboard.open(properties, project_scope, cx);
            } else {
                dashboard.set_properties(properties, cx);
            }
        });
        if open && !embedded {
            window.focus(&self.unfocused, cx);
        }
        dashboard.into_any_element()
    }
}
