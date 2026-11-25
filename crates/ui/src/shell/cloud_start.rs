//! Cloud-only projects use the current native new-conversation canvas.
use super::*;
impl Shell {
    pub(super) fn open_new_cloud_session(
        &mut self,
        project: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        self.open_new_cloud_session_in(project, None, cx);
    }

    /// A new chat on Cloud with `project`, keeping `local_space` (the same
    /// repository's checkout here) selected so Local and Worktree stay one
    /// pick away in the workspace menu.
    pub(super) fn open_new_cloud_session_in(
        &mut self,
        project: serde_json::Value,
        local_space: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).cloud_access().withholds_cloud() {
            self.open_settings(SettingsSection::Billing, cx);
            return;
        }
        let project = self
            .state
            .read(cx)
            .cloud_sidebar_projects()
            .into_iter()
            .find(|row| {
                row["projectId"] == project["projectId"] && row["accountId"] == project["accountId"]
            });
        let Some(project) = project else {
            self.open_settings(SettingsSection::Devices, cx);
            return;
        };
        self.start_cloud_canvas(project, local_space, cx);
    }

    /// The new-chat canvas with Cloud as its destination on `project`. New
    /// cloud chats are for Cloud subscribers; anyone else lands on Billing
    /// (or General when signed out) to subscribe first.
    pub(super) fn start_cloud_canvas(
        &mut self,
        project: serde_json::Value,
        local_space: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).cloud_access().withholds_cloud() {
            self.open_settings(SettingsSection::Billing, cx);
            return;
        }
        self.sync_cloud_tab_scope(cx);
        if !self.cloud_tab_navigation_ready(cx) {
            return;
        }
        // The canvas in front keeps its choices before its project changes.
        self.track_current_cloud_canvas(cx);
        self.state
            .update(cx, |state, _| state.cloud_project = Some(project.clone()));
        // Cloud is a destination of the native composer, not another window.
        // Bypass open_new_session's cloud-only dispatch to avoid recursion.
        self.reset_new_session_canvas(local_space, cx);
    }
}
