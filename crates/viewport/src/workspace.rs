//! Workspace operations share the native engine's RPC and durable queue.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};

pub(super) struct PendingFileSave {
    pub id: u64,
    pub file: Value,
    pub text: String,
}

impl WorkspaceView {
    pub(super) fn cancel(&mut self, id: u64) {
        let _ = self
            .requests
            .unbounded_send(json!({"id":id,"cancel":true,"epoch":self.connection_epoch}));
        self.pending.remove(&id);
    }

    pub(super) fn clear_workspace(&mut self, cx: &mut Context<Self>) {
        for (id, _) in std::mem::take(&mut self.git_requests) {
            self.cancel(id);
        }
        self.reset_pull_requests(cx);
        self.reset_project_actions(cx);
        self.detach_browser_terminal(cx);
        for id in [
            self.queue_watch.take(),
            self.directory_request.take(),
            self.file_request.take(),
            self.changes_request.take(),
            self.terminal_watch.take(),
        ]
        .into_iter()
        .flatten()
        {
            self.cancel(id);
        }
        self.queue.clear();
        self.entries.clear();
        self.directory.clear();
        self.directory_cursor = None;
        self.files_scroll = UniformListScrollHandle::new();
        self.file_read_baseline = None;
        self.open_file = None;
        self.changes = Value::Null;
        self.changes_error = None;
        self.git_controls
            .update(cx, |panel, cx| panel.set_target(None, cx));
        self.reset_review();
        self.reset_history(cx);
        self.review_scope = Default::default();
        self.terminal_last_seq = None;
        self.terminal_write = None;
        self.terminal_text.clear();
        self.terminal = vt100::Parser::new(28, 80, 2000);
    }

    pub(super) fn cwd(&self) -> String {
        self.selected
            .as_ref()
            .and_then(|c| c["cwd"].as_str())
            .or_else(|| self.project.as_ref().and_then(|p| p["path"].as_str()))
            .unwrap_or_default()
            .into()
    }

    pub(super) fn known_projects(&self) -> Vec<Value> {
        let mut projects = self.repos.clone();
        for space in &self.spaces {
            if zuse_protocol::projectless::is_projectless_cwd(Some(&space.path)) {
                continue;
            }
            if let Some(project) = projects.iter_mut().find(|p| p["path"] == space.path) {
                project["name"] = json!(space.display_name());
            } else {
                projects.push(json!({"path":space.path,"name":space.display_name()}));
            }
        }
        projects.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or_default().to_lowercase())
        });
        projects
    }
    pub(super) fn project_name(&self) -> String {
        let cwd = self.cwd();
        if zuse_protocol::projectless::is_projectless_cwd(Some(&cwd)) {
            return "Local workspace".into();
        }
        if let Some(space) = self
            .selected
            .as_ref()
            .and_then(|c| c["spaceId"].as_str())
            .and_then(|id| self.spaces.iter().find(|s| s.id == id))
        {
            return space.display_name().to_owned();
        }
        cwd.trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or("Local workspace")
            .into()
    }

    pub(super) fn is_working(&self) -> bool {
        self.selected.as_ref().is_some_and(|chat| {
            self.sessions.iter().any(|s| {
                s["chatId"] == chat["id"]
                    && matches!(s["status"].as_str(), Some("working" | "awaitingInput"))
            })
        })
    }

    pub(super) fn load_directory(&mut self, directory: &str) {
        if self.selected.is_none() {
            return;
        }
        if let Some(id) = self.directory_request.take() {
            self.cancel(id);
        }
        let chat = self.selected.as_ref().unwrap();
        self.directory_append = false;
        if directory != self.directory {
            self.files_scroll.scroll_to_item(0, ScrollStrategy::Top);
        }
        let id = self.call(
            methods::LIST_WORKSPACE_DIRECTORY,
            json!({"chatId":chat["id"],"directory":directory,"targetDeviceId":chat["deviceId"]}),
        );
        self.directory_request = Some(id);
    }

    pub(super) fn load_more_directory(&mut self, cx: &mut Context<Self>) {
        if self.directory_request.is_some() {
            return;
        }
        let (Some(chat), Some(cursor)) = (self.selected.as_ref(), self.directory_cursor.as_ref())
        else {
            return;
        };
        let id=self.call(methods::LIST_WORKSPACE_DIRECTORY,json!({"chatId":chat["id"],"directory":self.directory,"cursor":cursor,"targetDeviceId":chat["deviceId"]}));
        self.directory_append = true;
        self.directory_request = Some(id);
        cx.notify();
    }
    pub(super) fn open_entry(&mut self, entry: Value, cx: &mut Context<Self>) {
        let path = entry["path"].as_str().unwrap_or_default();
        if entry["kind"] == "directory" {
            self.load_directory(path);
        } else {
            if self.file_navigation_blocked(cx) {
                return;
            }
            self.read_file(path, false, cx);
        }
        cx.notify();
    }

    pub(super) fn set_panel(&mut self, tab: &str, cx: &mut Context<Self>) {
        self.right_open = true;
        self.right_tab = tab.into();
        match tab {
            "recovery" => self.inspect_recovery(),
            "actions" => self.refresh_project_actions(),
            "files" => {
                let dir = self.directory.clone();
                self.load_directory(&dir);
            }
            "changes" if !self.cwd().is_empty() => self.refresh_changes(cx),
            "history" if self.history.page.is_none() => self.refresh_history(cx),
            "pullRequests" => self.refresh_pull_requests(cx),
            "terminal" if self.terminal_id.is_none() => {
                if !self.restore_browser_terminal(cx) {
                    self.open_browser_terminal(cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn receive_terminal(&mut self, event: &Value) {
        if let Some(seq) = event["seq"].as_u64() {
            if self.terminal_last_seq.is_some_and(|last| seq <= last) {
                return;
            }
            self.terminal_last_seq = Some(seq);
        }
        if event["type"] == "data" {
            if let Some(data) = event["data"].as_str().and_then(|s| STANDARD.decode(s).ok()) {
                self.terminal.process(&data);
                self.terminal_text = self.terminal.screen().contents();
            }
        } else if event["type"] == "exit" {
            self.terminal_text
                .push_str(&format!("\n[Process exited: {}]", event["exitCode"]));
        }
    }

    pub(super) fn run_terminal_command(&mut self, cx: &mut Context<Self>) {
        if !self.require_connection(cx) || self.terminal_write.is_some() {
            return;
        }
        let Some(id) = self.terminal_id.clone() else {
            return;
        };
        let command = self.terminal_input.read(cx).value(cx);
        let request = self.call(
            methods::WRITE_TERMINAL,
            json!({"terminalId":id,"data":STANDARD.encode(format!("{command}\r")),"targetDeviceId":self.terminal_host}),
        );
        self.terminal_write = Some((request, command));
    }

    pub(super) fn interrupt(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = &self.selected {
            self.call(
                methods::QUEUE_COMMAND,
                zuse_protocol::presentation::chat_request(
                    chat,
                    json!({"command":{"kind":"interrupt"}}),
                ),
            );
        }
        cx.notify();
    }

    pub(super) fn archive(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = self.selected.clone() {
            let archived = !chat["archived"].as_bool().unwrap_or(false);
            self.change_thread(chat, thread_actions::ThreadChange::Archive(archived), cx);
        }
    }

    pub(super) fn add_project(&mut self, cx: &mut Context<Self>) {
        let path = self.project_input.read(cx).value(cx);
        if !path.trim().is_empty() {
            self.call(methods::ADD_REPO, json!({"path":path.trim()}));
        }
        cx.notify();
    }

    pub(super) fn attachment_paths(&self) -> Vec<&str> {
        self.attachments
            .iter()
            .filter_map(|a| a["path"].as_str())
            .collect()
    }

    /// Admit a browser paste only from this composer's focused editor.
    pub fn paste_attachments(
        &mut self,
        ticket: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.settings || !self.input.read(cx).focus_handle(cx).is_focused(window) {
            return false;
        }
        if !self.connected {
            self.error = Some("Reconnect to the engine before uploading files.".into());
            cx.notify();
            return false;
        }
        self.requests
            .unbounded_send(json!({"ui":"pasteAttachments","ticket":ticket,
            "generation":self.composer_generation,"epoch":self.connection_epoch}))
            .is_ok()
    }

    pub(super) fn pick_attachments(&mut self) {
        let _ = self
            .requests
            .unbounded_send(json!({"ui":"pickAttachments","generation":self.composer_generation,"epoch":self.connection_epoch}));
    }
}

impl WorkspaceView {
    pub(super) fn file_navigation_blocked(&mut self, cx: &mut Context<Self>) -> bool {
        let message = if self.file_reconnect_needed || self.file_reconcile.is_some() {
            Some("Wait for the file to be verified after reconnecting.")
        } else if self.save_request.is_some() {
            Some("Wait for the current save before opening or closing a file.")
        } else if self.file_dirty(cx) {
            Some("Save or discard your file edits before continuing.")
        } else {
            None
        };
        if let Some(message) = message {
            self.error = Some(message.into());
            cx.notify();
            true
        } else {
            false
        }
    }
    pub(super) fn read_file(&mut self, path: &str, discard: bool, cx: &mut Context<Self>) {
        if !self.require_connection(cx) {
            return;
        }
        if self.file_reconnect_needed || self.file_reconcile.is_some() {
            self.error = Some("Wait for the file to be verified after reconnecting.".into());
            cx.notify();
            return;
        }
        if self.save_request.is_some() || (!discard && self.file_navigation_blocked(cx)) {
            return;
        }
        let Some(chat) = self.selected.clone() else {
            return;
        };
        if let Some(id) = self.file_request.take() {
            self.cancel(id);
        }
        self.file_read_baseline = Some(self.file_editor.read(cx).value(cx));
        let id = self.call(
            methods::READ_WORKSPACE_FILE,
            json!({"chatId":chat["id"],"path":path,"targetDeviceId":chat["deviceId"]}),
        );
        self.file_request = Some(id);
        self.error = None;
        cx.notify();
    }
    pub(super) fn file_dirty(&self, cx: &App) -> bool {
        self.open_file
            .as_ref()
            .and_then(|file| file["text"].as_str())
            .is_some_and(|text| text != self.file_editor.read(cx).value(cx))
    }

    pub(super) fn save_file(&mut self, cx: &mut Context<Self>) {
        if !self.require_connection(cx) {
            return;
        }
        if self.file_reconnect_needed || self.file_reconcile.is_some() {
            self.error = Some("Wait for the file to be verified after reconnecting.".into());
            cx.notify();
            return;
        }
        if !self.file_dirty(cx) {
            return;
        }
        let (Some(file), Some(chat)) = (&self.open_file, &self.selected) else {
            return;
        };
        if self.save_request.is_some()
            || file["readOnlyReason"].is_string()
            || file["truncated"] == true
        {
            return;
        }
        let text = self.file_editor.read(cx).value(cx);
        let snapshot = file.clone();
        let id=self.call(methods::WRITE_WORKSPACE_FILE,json!({
            "chatId":chat["id"],"targetDeviceId":chat["deviceId"],"path":file["path"],"text":text,
            "expectedCheckoutId":file["checkoutId"],"expectedContentHash":file["contentHash"],
            "encoding":file["encoding"],"lineEnding":file["lineEnding"].as_str().unwrap_or("lf"),
        }));
        self.save_request = Some(PendingFileSave {
            id,
            file: snapshot,
            text,
        });
        cx.notify();
    }

    pub(super) fn close_file(&mut self, cx: &mut Context<Self>) {
        if self.file_navigation_blocked(cx) {
            return;
        }
        if let Some(id) = self.file_request.take() {
            self.cancel(id);
        }
        self.file_read_baseline = None;
        self.open_file = None;
        cx.notify();
    }

    pub(super) fn discard_file_edits(&mut self, cx: &mut Context<Self>) {
        let path = self
            .open_file
            .as_ref()
            .and_then(|file| file["path"].as_str())
            .map(str::to_owned);
        if let Some(path) = path {
            self.read_file(&path, true, cx);
        }
    }
}

impl WorkspaceView {
    pub(super) fn draft_key(&self) -> String {
        self.selected
            .as_ref()
            .and_then(|c| c["id"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "new:{}",
                    self.project
                        .as_ref()
                        .and_then(|p| p["path"].as_str())
                        .unwrap_or_default()
                )
            })
    }

    pub(super) fn stash_draft(&mut self, cx: &App) {
        let key = self.draft_key();
        let draft = Draft {
            text: self.input.read(cx).value(cx),
            attachments: self.attachments.clone(),
            key: key.clone(),
            chat_id: self
                .selected
                .as_ref()
                .and_then(|chat| chat["id"].as_str())
                .map(str::to_owned),
            host_id: self.draft_host(),
            not_sent: true,
            ..Default::default()
        };
        if draft.text.is_empty() && draft.attachments.is_empty() {
            self.drafts.remove(&key);
        } else {
            self.drafts.insert(key, draft);
        }
    }

    pub(super) fn restore_draft(&mut self, cx: &mut Context<Self>) {
        let draft = self
            .drafts
            .get(&self.draft_key())
            .cloned()
            .unwrap_or_default();
        if !draft.attachments.is_empty() && draft.host_id != self.draft_host() {
            if !self.uncertain_drafts.contains(&draft) {
                self.uncertain_drafts.push(draft);
            }
            self.attachments.clear();
            self.input
                .update(cx, |input, cx| input.set_value(String::new(), cx));
            return;
        }
        self.attachments = draft.attachments;
        self.input
            .update(cx, |input, cx| input.set_value(draft.text, cx));
        self.sync_previews(true, cx);
    }

    pub(super) fn restore_failed_draft(&mut self, draft: Draft, cx: &mut Context<Self>) {
        if self.queue_edit.is_some() {
            self.uncertain_drafts.push(draft);
            self.error =
                Some("Finish the queued-message edit before restoring another draft.".into());
            cx.notify();
            return;
        }
        let current = self.draft_key() == draft.key;
        if current && self.input.read(cx).value(cx).is_empty() && self.attachments.is_empty() {
            self.attachments = draft.attachments;
            self.input
                .update(cx, |input, cx| input.set_value(draft.text, cx));
        } else if current || self.drafts.contains_key(&draft.key) {
            if !self.uncertain_drafts.contains(&draft) {
                self.uncertain_drafts.push(draft);
            }
        } else {
            self.drafts.insert(draft.key.clone(), draft);
        }
    }
}
