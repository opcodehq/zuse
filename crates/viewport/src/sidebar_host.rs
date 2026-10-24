//! Browser host wiring for the shared account and usage controls.
use super::*;
use crate::sidebar_controls::{Event, Properties, SidebarControls};
impl WorkspaceView {
    pub(super) fn create_sidebar_controls(cx: &mut Context<Self>) -> Entity<SidebarControls> {
        let controls = cx.new(SidebarControls::new);
        cx.subscribe(&controls, |this, _, event, cx| match event {
            Event::Call { id, method, params } => {
                let remote = this.call(method, params.clone());
                this.sidebar_control_requests.insert(remote, *id);
            }
            Event::Cancel(id) => {
                let remote = this
                    .sidebar_control_requests
                    .iter()
                    .find_map(|(remote, local)| (*local == *id).then_some(*remote));
                if let Some(remote) = remote {
                    this.sidebar_control_requests.remove(&remote);
                    this.cancel(remote);
                }
            }
            Event::CloudSettings => {
                this.settings = true;
                this.select_settings_section(crate::settings_view::Section::Cloud, cx);
            }
            Event::ProviderAccounts => {
                this.settings = true;
                this.select_settings_section(crate::settings_view::Section::Usage, cx);
            }
        })
        .detach();
        controls
    }
    pub(super) fn update_sidebar_controls(&self, cx: &mut Context<Self>) {
        let properties = Properties {
            account: (!self.cloud_account.is_null()).then(|| self.cloud_account.clone()),
            available: self.connected,
            target: None,
            working: self
                .sessions
                .iter()
                .filter(|session| session["status"] == "working")
                .count(),
        };
        self.sidebar_controls
            .update(cx, |controls, cx| controls.set_properties(properties, cx));
    }
    pub(super) fn receive_sidebar_controls(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(local) = self.sidebar_control_requests.remove(&id) else {
            return false;
        };
        self.pending.remove(&id);
        let result = frame.get("ok").cloned().ok_or_else(|| {
            frame["err"]
                .as_str()
                .unwrap_or("The account connection was interrupted.")
                .to_owned()
        });
        self.sidebar_controls
            .update(cx, |controls, cx| controls.receive(local, result, cx));
        true
    }
}
