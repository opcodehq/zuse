//! Workspace operations share the native engine's RPC and durable queue.
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};

impl WorkspaceView {
    pub(super) fn cancel(&mut self, id: u64) {
        let _ = self.requests.unbounded_send(json!({"id":id,"cancel":true}));
        self.pending.remove(&id);
    }

    pub(super) fn clear_workspace(&mut self) {
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
        if let Some(terminal) = self.terminal_id.take() {
            self.call(methods::CLOSE_TERMINAL, json!({"terminalId":terminal}));
        }
        self.queue.clear();
        self.entries.clear();
        self.directory.clear();
        self.open_file = None;
        self.changes = Value::Null;
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

    pub(super) fn project_name(&self) -> String {
        let cwd = self.cwd();
        cwd.trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or("Local workspace")
            .into()
    }

    pub(super) fn is_working(&self) -> bool {
        self.selected.as_ref().is_some_and(|chat| {
            self.sessions
                .iter()
                .any(|s| s["chatId"] == chat["id"] && s["status"] == "working")
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
        let id = self.call(
            methods::LIST_WORKSPACE_DIRECTORY,
            json!({"chatId":chat["id"],"directory":directory}),
        );
        self.directory_request = Some(id);
    }

    pub(super) fn open_entry(&mut self, entry: Value, cx: &mut Context<Self>) {
        let path = entry["path"].as_str().unwrap_or_default();
        if entry["kind"] == "directory" {
            self.load_directory(path);
        } else if let Some(chat) = &self.selected {
            let id = self.call(
                methods::READ_WORKSPACE_FILE,
                json!({"chatId":chat["id"],"path":path}),
            );
            self.file_request = Some(id);
        }
        cx.notify();
    }

    pub(super) fn set_panel(&mut self, tab: &str, cx: &mut Context<Self>) {
        self.right_open = true;
        self.right_tab = tab.into();
        match tab {
            "recovery" => self.inspect_recovery(),
            "files" => {
                let dir = self.directory.clone();
                self.load_directory(&dir);
            }
            "changes" if !self.cwd().is_empty() => {
                let id = self.call(methods::GET_CHECKOUT_DIFF, json!({"cwd":self.cwd(),"mode":"workingTree","chatId":self.selected.as_ref().map(|c|&c["id"])}));
                self.changes_request = Some(id);
            }
            "terminal" if self.terminal_id.is_none() => {
                self.call(methods::OPEN_TERMINAL, json!({"chatId":self.selected.as_ref().map(|c|c["id"].as_str().unwrap_or_default()).unwrap_or("browser-canvas"),"cwd":self.cwd(),"cols":80,"rows":28}));
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn receive_terminal(&mut self, event: &Value) {
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
        let Some(id) = self.terminal_id.clone() else {
            return;
        };
        let command = self.terminal_input.read(cx).value(cx);
        self.call(
            methods::WRITE_TERMINAL,
            json!({"terminalId":id,"data":STANDARD.encode(format!("{command}\r"))}),
        );
        self.terminal_input
            .update(cx, |input, cx| input.set_value(String::new(), cx));
    }

    pub(super) fn queue_action(
        &mut self,
        method: &str,
        item: &Value,
        position: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if let Some(chat) = &self.selected {
            let mut params = json!({"chatId":chat["id"],"id":item["id"]});
            if let Some(position) = position {
                params["toIndex"] = json!(position);
            }
            self.call(method, params);
        }
        cx.notify();
    }

    pub(super) fn interrupt(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = &self.selected {
            self.call(
                methods::QUEUE_COMMAND,
                json!({"chatId":chat["id"],"command":{"kind":"interrupt"}}),
            );
        }
        cx.notify();
    }

    pub(super) fn archive(&mut self, cx: &mut Context<Self>) {
        if let Some(chat) = &self.selected {
            self.call(
                methods::MUTATE,
                json!({"op":"setChatArchived","chatId":chat["id"],"archived":true}),
            );
            self.new_chat(cx);
        }
    }

    pub(super) fn add_project(&mut self, cx: &mut Context<Self>) {
        let path = self.project_input.read(cx).value(cx);
        if !path.trim().is_empty() {
            self.call(methods::ADD_REPO, json!({"path":path.trim()}));
        }
        cx.notify();
    }

    pub(super) fn choose_provider(&mut self, index: usize, cx: &mut Context<Self>) {
        self.provider_index = index;
        self.model = None;
        self.models.clear();
        self.settings = false;
        if let Some(provider) = self.providers.get(index) {
            self.call(methods::LIST_MODELS, json!({"harness":provider["id"]}));
        }
        cx.notify();
    }

    pub(super) fn attachment_paths(&self) -> Vec<&str> {
        self.attachments
            .iter()
            .filter_map(|a| a["path"].as_str())
            .collect()
    }

    pub(super) fn pick_attachments(&mut self) {
        let _ = self
            .requests
            .unbounded_send(json!({"ui":"pickAttachments","generation":self.composer_generation}));
    }
}

impl WorkspaceView {
    pub(super) fn file_dirty(&self, cx: &App) -> bool {
        self.open_file
            .as_ref()
            .and_then(|file| file["text"].as_str())
            .is_some_and(|text| text != self.file_editor.read(cx).value(cx))
    }

    pub(super) fn save_file(&mut self, cx: &mut Context<Self>) {
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
        let id=self.call(methods::WRITE_WORKSPACE_FILE,json!({
            "chatId":chat["id"],"path":file["path"],"text":text,
            "expectedCheckoutId":file["checkoutId"],"expectedContentHash":file["contentHash"],
            "encoding":file["encoding"],"lineEnding":file["lineEnding"].as_str().unwrap_or("lf"),
        }));
        self.save_request = Some((id, text));
        cx.notify();
    }

    pub(super) fn close_file(&mut self, cx: &mut Context<Self>) {
        if self.file_dirty(cx) {
            self.error = Some("Save or discard your file edits before closing the editor.".into());
        } else {
            self.open_file = None;
        }
        cx.notify();
    }

    pub(super) fn discard_file_edits(&mut self, cx: &mut Context<Self>) {
        if let (Some(file), Some(chat)) = (&self.open_file, &self.selected) {
            let id = self.call(
                methods::READ_WORKSPACE_FILE,
                json!({"chatId":chat["id"],"path":file["path"]}),
            );
            self.file_request = Some(id);
            self.error = None;
        }
        cx.notify();
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
        self.attachments = draft.attachments;
        self.input
            .update(cx, |input, cx| input.set_value(draft.text, cx));
    }

    pub(super) fn restore_failed_draft(&mut self, draft: Draft, cx: &mut Context<Self>) {
        if self.draft_key() == draft.key && self.input.read(cx).value(cx).is_empty() {
            self.attachments = draft.attachments;
            self.input
                .update(cx, |input, cx| input.set_value(draft.text, cx));
        } else {
            self.drafts.insert(draft.key.clone(), draft);
        }
    }
}
