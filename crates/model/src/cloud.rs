//! Read-only cloud context shared by native and browser viewports.
use crate::schema::SessionMessageEntry;
use crate::transcript_delta::TranscriptUpdate;
use serde::{Deserialize, Serialize};
use zuse_protocol::Chat;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub epoch: String,
    pub version: i64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Head {
    pub schema_version: u32,
    pub payload_format: String,
    pub workspace_id: String,
    pub session_id: String,
    pub runtime_generation: i64,
    pub cursor: Cursor,
    pub chat: Chat,
    pub conversations: Vec<Chat>,
    pub transcript: TranscriptUpdate,
    pub queue: serde_json::Value,
    pub queue_paused: bool,
    pub older_message_sequence: Option<i64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub schema_version: u32,
    pub payload_format: String,
    pub workspace_id: String,
    pub session_id: String,
    pub runtime_generation: i64,
    pub cursor: Cursor,
    pub before_sequence: i64,
    pub entries: Vec<SessionMessageEntry>,
    pub older_message_sequence: Option<i64>,
}
#[derive(Default, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMirrorCleanup {
    pub state: String,
    pub removed: usize,
    pub preserved: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
impl LocalMirrorCleanup {
    pub fn pending() -> Self {
        Self {
            state: "pending".into(),
            ..Self::default()
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub workspace_id: String,
    pub chat_id: String,
    pub initial_session_id: String,
    pub branch: String,
    pub state: String,
    pub desired_state: String,
    pub status_code: String,
    pub revision: i64,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub rpc_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_mirror_cleanup: Option<LocalMirrorCleanup>,
}
impl Workspace {
    pub fn archived(&self) -> bool {
        matches!(self.state.as_str(), "archiving" | "archived") || self.desired_state == "archived"
    }
    pub fn deleting(&self) -> bool {
        self.desired_state == "deleted" && self.state != "deleted"
    }
    pub fn archive_visible(&self) -> bool {
        self.archived()
            || self.deleting()
            || (self.state == "deleted" && (self.cleanup_pending() || self.cleanup_failed()))
    }
    pub fn cleanup_pending(&self) -> bool {
        self.local_mirror_cleanup
            .as_ref()
            .is_some_and(|s| s.state == "pending")
    }
    pub fn cleanup_failed(&self) -> bool {
        self.local_mirror_cleanup
            .as_ref()
            .is_some_and(|s| s.state == "error")
    }
    pub fn cleanup_label(&self) -> Option<&str> {
        self.local_mirror_cleanup
            .as_ref()
            .and_then(|s| match s.state.as_str() {
                "pending" => Some("Cleaning up the local mirror…"),
                "error" => s.error.as_deref(),
                _ => None,
            })
    }
    pub fn delete_failed(&self) -> bool {
        self.deleting() && self.status_code.starts_with("cloud_")
    }
    pub fn deletion_label(&self) -> &'static str {
        match self.status_code.as_str() {
            _ if self.delete_failed() => "Deletion could not finish. Retry to continue cleanup.",
            "delete-waiting-for-execution" => "Stopping background work…",
            "delete-provider-cleanup" => "Removing cloud workspace…",
            "delete-purging-data" => "Removing saved history and backups…",
            "delete-clearing-metadata" => "Finishing deletion…",
            _ => "Deleting cloud thread…",
        }
    }
    pub fn label(&self) -> &str {
        self.title
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.branch)
    }
}
#[derive(Clone, Default)]
pub struct ArchivedCloud {
    pub account: Option<String>,
    pub rows: Vec<Workspace>,
    pub selected: Option<String>,
    pub head: Option<Head>,
    pub loading: bool,
    pub busy: bool,
    pub error: Option<String>,
    pub revision: u64,
}
impl ArchivedCloud {
    pub fn clear(&mut self) {
        let rev = self.revision.wrapping_add(1);
        *self = Self::default();
        self.revision = rev;
    }
    pub fn select(&mut self, id: &str) -> Option<u64> {
        if !self
            .rows
            .iter()
            .any(|w| w.workspace_id == id && w.archived())
        {
            return None;
        }
        self.revision = self.revision.wrapping_add(1);
        self.selected = Some(id.into());
        self.head = None;
        self.error = None;
        self.loading = true;
        Some(self.revision)
    }
    pub fn receive(&mut self, revision: u64, head: Head) -> bool {
        if self.revision != revision || self.selected.as_deref() != Some(head.workspace_id.as_str())
        {
            return false;
        }
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|r| r.workspace_id == head.workspace_id)
        {
            row.title = head.chat.title.clone();
        }
        self.head = Some(head);
        self.loading = false;
        true
    }
    pub fn list(&mut self, account: String, mut rows: Vec<Workspace>) {
        if self.account.as_deref() != Some(&account) {
            self.clear();
        }
        for row in &mut rows {
            if row.title.is_none() {
                row.title = self
                    .rows
                    .iter()
                    .find(|old| old.workspace_id == row.workspace_id)
                    .and_then(|old| old.title.clone());
            }
        }
        self.account = Some(account);
        self.rows = rows;
        if self.selected.as_ref().is_some_and(|id| {
            !self
                .rows
                .iter()
                .any(|r| &r.workspace_id == id && r.archived())
        }) {
            self.back();
        }
    }
    pub fn update_workspace(&mut self, mut row: Workspace) {
        if let Some(old) = self
            .rows
            .iter_mut()
            .find(|r| r.workspace_id == row.workspace_id)
        {
            if row.title.is_none() {
                row.title = old.title.clone();
            }
            *old = row;
        } else {
            self.rows.push(row);
        }
        self.back();
    }
    pub fn deleting(&self) -> bool {
        self.rows.iter().any(Workspace::deleting)
    }
    pub fn needs_poll(&self) -> bool {
        self.rows
            .iter()
            .any(|r| r.deleting() || r.cleanup_pending())
    }
    pub fn back(&mut self) {
        self.selected = None;
        self.head = None;
        self.loading = false;
        self.busy = false;
        self.error = None;
        self.revision = self.revision.wrapping_add(1);
    }
    pub fn restored(&mut self, id: &str) {
        self.rows.retain(|w| w.workspace_id != id);
        self.back();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn workspace(id: &str) -> Workspace {
        Workspace {
            workspace_id: id.into(),
            chat_id: format!("chat-{id}"),
            initial_session_id: format!("session-{id}"),
            branch: "feature".into(),
            state: "archived".into(),
            desired_state: "archived".into(),
            status_code: "archived".into(),
            revision: 1,
            title: None,
            rpc_protocol: Some("zuse-rpc-v1".into()),
            local_mirror_cleanup: None,
        }
    }
    #[test]
    fn account_changes_and_navigation_invalidate_pending_history() {
        let mut state = ArchivedCloud::default();
        state.list("alice".into(), vec![workspace("a")]);
        let first = state.select("a").unwrap();
        state.back();
        assert!(state.revision > first);
        state.select("a");
        state.list("bob".into(), vec![workspace("b")]);
        assert!(state.selected.is_none());
        assert!(state.head.is_none());
        assert!(state.select("a").is_none());
        assert!(state.select("b").is_some());
        let revision = state.revision;
        state.clear();
        assert!(state.revision > revision);
        assert!(state.rows.is_empty());
        assert!(state.account.is_none());
    }
    #[test]
    fn confirmed_unarchive_removes_only_its_workspace() {
        let mut state = ArchivedCloud::default();
        state.list("alice".into(), vec![workspace("a"), workspace("b")]);
        state.select("a");
        state.busy = true;
        state.restored("a");
        assert_eq!(state.rows.len(), 1);
        assert_eq!(state.rows[0].workspace_id, "b");
        assert!(state.selected.is_none());
        assert!(!state.busy);
        assert!(state.rows[0].archived());
    }
    #[test]
    fn deleting_rows_keep_progress_and_clear_saved_history_until_the_tombstone_arrives() {
        let mut state = ArchivedCloud::default();
        let mut row = workspace("cloud");
        row.title = Some("Original thread".into());
        state.list("account".into(), vec![row.clone()]);
        state.select("cloud").unwrap();
        row.title = None;
        row.state = "deleting".into();
        row.desired_state = "deleted".into();
        row.status_code = "delete-purging-data".into();
        state.update_workspace(row.clone());
        assert!(state.selected.is_none());
        assert!(state.head.is_none());
        assert!(state.rows[0].archive_visible());
        assert!(state.deleting());
        assert_eq!(state.rows[0].label(), "Original thread");
        row.status_code = "cloud_object_purge_failed".into();
        state.list("account".into(), vec![row.clone()]);
        assert!(state.rows[0].delete_failed());
        row.state = "deleted".into();
        row.status_code = "deleted".into();
        state.list("account".into(), vec![row]);
        assert!(!state.deleting());
        assert!(!state.rows[0].archive_visible());
    }
    #[test]
    fn tombstones_stay_visible_until_local_cleanup_finishes_or_is_retried() {
        let mut row = workspace("cloud");
        row.state = "deleted".into();
        row.desired_state = "deleted".into();
        row.local_mirror_cleanup = Some(LocalMirrorCleanup::pending());
        let mut state = ArchivedCloud::default();
        state.list("account".into(), vec![row.clone()]);
        assert!(state.needs_poll());
        assert!(state.rows[0].archive_visible());
        assert!(state.select("cloud").is_none());
        row.local_mirror_cleanup.as_mut().unwrap().state = "error".into();
        state.list("account".into(), vec![row.clone()]);
        assert!(!state.needs_poll());
        assert!(state.rows[0].cleanup_failed());
        assert!(state.rows[0].archive_visible());
        row.local_mirror_cleanup.as_mut().unwrap().state = "complete".into();
        state.list("account".into(), vec![row]);
        assert!(!state.needs_poll());
        assert!(!state.rows[0].archive_visible());
    }
}
