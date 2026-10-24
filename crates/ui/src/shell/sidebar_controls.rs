//! The shared sidebar controls use the controller's General-account authority.
use super::*;
use zuse_viewport::sidebar_controls::{Event, Properties, SidebarControls};

impl Shell {
    pub(super) fn create_sidebar_controls(
        state: Entity<AppState>,
        cx: &mut Context<Self>,
    ) -> Entity<SidebarControls> {
        let controls = cx.new(SidebarControls::new);
        cx.observe(&state, |this, _, cx| this.update_sidebar_controls(cx))
            .detach();
        cx.subscribe(&controls, move |this, controls, event, cx| match event {
            Event::Cancel(id) => {
                this.sidebar_control_tasks.remove(id);
            }
            Event::CloudSettings => this.open_settings(SettingsSection::Cloud, cx),
            Event::ProviderAccounts => this.open_settings(SettingsSection::Agents, cx),
            Event::Call { id, method, params } => {
                let Some(engine) = state.read(cx).engine().map(|e| e.controller()) else {
                    controls.update(cx, |controls, cx| {
                        controls.receive(
                            *id,
                            Err("The account connection is unavailable.".into()),
                            cx,
                        )
                    });
                    return;
                };
                let id = *id;
                let method = *method;
                let params = params.clone();
                let controls = controls.clone();
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
                        this.sidebar_control_tasks.remove(&id);
                        controls.update(cx, |controls, cx| controls.receive(id, result, cx));
                    });
                });
                this.sidebar_control_tasks.insert(id, task);
            }
        })
        .detach();
        controls
    }
    pub(super) fn update_sidebar_controls(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let properties = Properties {
            account: state.cloud_account.clone(),
            available: state.engine().is_some(),
            target: state.effective_device_id(),
            working: state
                .sidebar_chats(Utc::now(), None)
                .into_iter()
                .filter(|(s, _)| *s == zuse_protocol::ChatIndicator::Working)
                .count(),
        };
        self.sidebar_controls
            .update(cx, |controls, cx| controls.set_properties(properties, cx));
    }
}
