//! Browser transport for the shared recorded-usage dashboard.
use super::*;
use crate::usage_dashboard::{Event, Project, Properties, UsageDashboard};
impl WorkspaceView {
    pub(super) fn request_usage(
        &mut self,
        project: Option<String>,
        project_scope: bool,
        cx: &mut Context<Self>,
    ) {
        self.close_project_archives();
        self.close_plugins_browse();
        self.usage_project = project
            .or_else(|| {
                self.selected
                    .as_ref()
                    .and_then(|c| c["spaceId"].as_str())
                    .map(str::to_owned)
            })
            .or_else(|| {
                self.project
                    .as_ref()
                    .and_then(|p| p["spaceId"].as_str())
                    .map(str::to_owned)
            });
        self.usage_project_scope = project_scope;
        self.usage_open = true;
        self.usage_pending = true;
        self.settings = false;
        self.project_picker = false;
        self.model_picker = false;
        self.access_picker = false;
        self.agent_picker = false;
        self.add_menu = false;
        self.context_menu = false;
        cx.notify();
    }
    pub(super) fn close_usage(&mut self, cx: &mut Context<Self>) {
        if self.usage_open {
            self.usage_open = false;
            if let Some(dashboard) = self.usage_dashboard.clone() {
                dashboard.update(cx, |dashboard, cx| dashboard.hide(cx));
            }
        }
    }
    fn usage_properties(&self) -> Properties {
        let project = self
            .usage_project
            .as_ref()
            .and_then(|id| self.spaces.iter().find(|s| &s.id == id));
        Properties {
            available: self.connected,
            owner: json!([
                self.cloud_account["user"]["id"],
                self.cloud_account["orgId"]
            ])
            .to_string(),
            target: project.map(|s| s.device_id.clone()),
            project: project.map(|s| Project {
                id: s.id.clone(),
                label: s.display_name().to_owned(),
            }),
        }
    }
    pub(super) fn usage_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.usage_dashboard.is_none() {
            let dashboard = cx.new(|cx| UsageDashboard::new(window, cx));
            cx.subscribe_in(
                &dashboard,
                window,
                |this, dashboard, event, window, cx| match event {
                    Event::Call { id, method, params } => {
                        if !this.connected {
                            dashboard.update(cx, |dashboard, cx| {
                                dashboard.receive(
                                *id,
                                Err("Computer unavailable. Reconnect and refresh usage.".into()),
                                cx,
                            )
                            });
                        } else {
                            let remote = this.call(method, params.clone());
                            this.usage_requests.insert(remote, *id);
                        }
                    }
                    Event::Cancel(id) => {
                        let remote = this
                            .usage_requests
                            .iter()
                            .find_map(|(remote, local)| (*local == *id).then_some(*remote));
                        if let Some(remote) = remote {
                            this.usage_requests.remove(&remote);
                            this.cancel(remote);
                        }
                    }
                    Event::Close => {
                        this.close_usage(cx);
                        window.focus(&this.workspace_focus, cx);
                        cx.notify();
                    }
                },
            )
            .detach();
            self.usage_dashboard = Some(dashboard);
        }
        let dashboard = self.usage_dashboard.clone().unwrap();
        let properties = self.usage_properties();
        if self.usage_pending {
            self.usage_pending = false;
            dashboard.update(cx, |dashboard, cx| {
                dashboard.open(properties, self.usage_project_scope, cx)
            });
            window.focus(&self.workspace_focus, cx);
        } else {
            dashboard.update(cx, |dashboard, cx| dashboard.set_properties(properties, cx));
        }
        dashboard.into_any_element()
    }
    pub(super) fn receive_usage(&mut self, id: u64, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(local) = self.usage_requests.remove(&id) else {
            return false;
        };
        self.pending.remove(&id);
        let result = frame.get("ok").cloned().ok_or_else(|| {
            frame["err"]
                .as_str()
                .unwrap_or("Could not load recorded usage.")
                .to_owned()
        });
        if let Some(dashboard) = self.usage_dashboard.clone() {
            dashboard.update(cx, |dashboard, cx| dashboard.receive(local, result, cx));
        }
        true
    }
}
