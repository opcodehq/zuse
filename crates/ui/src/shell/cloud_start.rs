//! Cloud-only projects use the current native new-conversation canvas.
use super::*;
impl Shell {
    pub(super) fn open_new_cloud_session(
        &mut self,
        project: serde_json::Value,
        cx: &mut Context<Self>,
    ) {
        self.sync_cloud_tab_scope(cx);
        if !self.cloud_tab_navigation_ready(cx) {
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
        self.state
            .update(cx, |state, _| state.cloud_project = Some(project.clone()));
        // Cloud is a destination of the native composer, not another window.
        // Bypass open_new_session's cloud-only dispatch to avoid recursion.
        self.reset_new_session_canvas(None, cx);
    }
}
