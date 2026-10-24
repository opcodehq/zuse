//! The device workspace's sync controls remain separate from the General account.
use super::*;
use settings::devices::{StorageEvent, WorkspaceStorage};

impl Shell {
    pub(super) fn create_devices_page(&mut self, cx: &mut Context<Self>) -> Entity<DevicesPage> {
        let state = self.state.clone();
        let page = cx.new(|cx| DevicesPage::new(state, cx));
        cx.subscribe(&page, |this, _, event, cx| match event {
            StorageEvent::Action => {
                if this.state.read(cx).engine().is_none() {
                    return;
                }
                let scope = this.state.read(cx).workspace_scope;
                match account_menu_action(scope, this.sync_flow) {
                    Some(AccountMenuAction::EnableSync) => this.start_sign_in(cx),
                    Some(AccountMenuAction::RestartPending) => this.reopen_sync_notice(cx),
                    Some(AccountMenuAction::SignOut) => this.request_sign_out(cx),
                    _ => {}
                }
            }
        })
        .detach();
        page
    }

    pub(super) fn workspace_storage_properties(&self, cx: &App) -> WorkspaceStorage {
        let state = self.state.read(cx);
        let (status, detail) = match state.workspace_scope {
            Some(WorkspaceScope::Local) => (
                "Local storage",
                "Chats are stored on this computer.".to_owned(),
            ),
            Some(WorkspaceScope::Synced) => (
                "Synced storage",
                state
                    .auth_user()
                    .map(|user| format!("Sync account: {}", user.email))
                    .unwrap_or_else(|| "This workspace syncs across your devices.".into()),
            ),
            Some(WorkspaceScope::Development) => (
                "Development workspace",
                "Chats are stored on this computer.".into(),
            ),
            None => (
                "Loading workspace",
                "Connecting to workspace storage…".into(),
            ),
        };
        let (action, enabled) = match account_menu_action(state.workspace_scope, self.sync_flow) {
            Some(AccountMenuAction::EnableSync) => (Some("Enable sync"), true),
            Some(AccountMenuAction::RestartPending) => (Some("Continue setup"), true),
            Some(AccountMenuAction::SignOut) => (Some("Sign out of sync"), true),
            Some(AccountMenuAction::SyncInProgress) => (Some("Sync setup in progress"), false),
            None => (None, false),
        };
        WorkspaceStorage {
            status: status.into(),
            detail: detail.into(),
            action,
            enabled: enabled && state.engine().is_some(),
        }
    }
}
