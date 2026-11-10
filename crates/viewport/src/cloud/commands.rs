//! API command availability is separate from a live execution connection.
use super::*;
impl WorkspaceView {
    pub(crate) fn cloud_unreviewed_command(&self) -> Option<String> {
        if !self.cloud_server_commands() {
            return None;
        }
        let session = self.selected.as_ref()?["id"].as_str()?;
        self.cloud
            .command_admissions
            .iter()
            .find(|row| {
                row["sessionId"] == session
                    && row["executionState"] == "outcome-unknown"
                    && row["unknownReviewed"] != true
            })
            .and_then(|row| row["commandId"].as_str())
            .filter(|id| !id.is_empty() && id.len() <= 512)
            .map(str::to_owned)
    }
    pub(crate) fn review_unknown_command(&mut self, cx: &mut Context<Self>) {
        if self.unknown_review_request.is_some() {
            return;
        }
        let Some(target) = self.cloud_unreviewed_command() else {
            return;
        };
        let Some(session) = self
            .selected
            .as_ref()
            .and_then(|v| v["id"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        let request = json!({"commandId":uuid::Uuid::new_v4().to_string(),"sessionId":session,"action":{"type":"review-unknown","interruptedCommandId":target}});
        let id = self.call(methods::CLOUD_TASK_COMMAND, json!({"request":request}));
        self.unknown_review_request = Some((id, session, target));
        cx.notify();
    }
    pub(crate) fn receive_unknown_review(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .unknown_review_request
            .as_ref()
            .is_none_or(|(pending, _, _)| *pending != id)
        {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let (_, session, target) = self.unknown_review_request.take().unwrap();
        self.pending.remove(&id);
        if self.selected.as_ref().and_then(|v| v["id"].as_str()) != Some(session.as_str()) {
            return true;
        }
        if frame["ok"]["reviewed"] == true && frame["ok"]["interruptedCommandId"] == target {
            if self.cloud.reviewed_commands.len() < 4096 {
                self.cloud
                    .reviewed_commands
                    .insert((session.clone(), target.clone()));
            }
            for row in &mut self.cloud.command_admissions {
                if row["sessionId"] == session && row["commandId"] == target {
                    row["unknownReviewed"] = json!(true);
                }
            }
            if let Some(chat) = self.selected.clone() {
                self.refresh_queue_snapshot(&chat);
            }
        } else {
            self.error=Some("The recovery review was not confirmed. Check its current status before sending new work.".into());
        }
        cx.notify();
        true
    }
    /// Keep API admissions separate from guest cursors and prefer canonical rows
    /// with the same immutable user message identity when the guest catches up.
    pub(crate) fn clear_cloud_admission_overlay(&mut self) {
        let provisional = std::mem::take(&mut self.cloud.admission_overlay);
        self.messages
            .retain(|message| !provisional.contains(&message.id));
    }
    pub(crate) fn restore_cloud_admission_overlay(&mut self) {
        let rows = self.cloud.command_admissions.clone();
        self.merge_cloud_admissions(&rows);
    }
    pub(crate) fn merge_cloud_admissions(&mut self, admissions: &[Value]) {
        for row in admissions.iter().filter(|v| v["unknownReviewed"] == true) {
            if self.cloud.reviewed_commands.len() < 4096
                && let Some((session, command)) =
                    row["sessionId"].as_str().zip(row["commandId"].as_str())
            {
                self.cloud
                    .reviewed_commands
                    .insert((session.to_owned(), command.to_owned()));
            }
        }
        self.clear_cloud_admission_overlay();
        if self.cloud.response_wait.as_ref().is_some_and(|bridge| {
            admissions.iter().any(|row| {
                row["commandId"] == bridge["commandId"]
                    && matches!(
                        row["executionState"].as_str(),
                        Some(
                            "completed"
                                | "cancelled"
                                | "failed"
                                | "outcome-unknown"
                                | "waiting-input"
                        )
                    )
            })
        }) {
            self.cloud.response_wait = None;
        }
        self.cloud.command_admissions = admissions.to_vec();
        for row in &mut self.cloud.command_admissions {
            if row["sessionId"]
                .as_str()
                .zip(row["commandId"].as_str())
                .is_some_and(|(sid, cid)| {
                    self.cloud
                        .reviewed_commands
                        .contains(&(sid.to_owned(), cid.to_owned()))
                })
            {
                row["unknownReviewed"] = json!(true);
            }
        }
        let Some(session) = self.selected.as_ref().and_then(|chat| chat["id"].as_str()) else {
            return;
        };
        let mut seen: std::collections::HashSet<String> =
            self.messages.iter().map(|m| m.id.clone()).collect();
        let mut pending = Vec::new();
        for row in admissions.iter().take(256) {
            if row["sessionId"].as_str().is_some_and(|id| id != session) {
                continue;
            }
            let Some(id) = row["messageId"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 512)
            else {
                continue;
            };
            if !seen.insert(id.into()) {
                continue;
            }
            let Some(text) = row["text"].as_str() else {
                continue;
            };
            let assets = serde_json::from_value::<Vec<zuse_protocol::cloud_assets::Asset>>(
                row.get("attachments").cloned().unwrap_or(json!([])),
            );
            let Ok(assets) = assets else {
                continue;
            };
            use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
            let refs = assets
                .iter()
                .filter_map(|asset| {
                    let bytes = serde_json::to_vec(asset).ok()?;
                    (bytes.len() <= 3072).then(|| {
                        format!(
                            "cloud-asset://{}/{}",
                            URL_SAFE_NO_PAD.encode(bytes),
                            asset.original_name.replace(['/', '\\'], "_")
                        )
                    })
                })
                .collect::<Vec<_>>();
            if refs.len() != assets.len() {
                continue;
            }
            self.cloud.admission_overlay.insert(id.into());
            pending.push(SessionMessageEntry {
                id: id.into(),
                role: zuse_model::MessageRole::User,
                parts: vec![zuse_model::MessagePart::Text {
                    id: format!("admission:{id}"),
                    text: zuse_protocol::message_attachments::with_attachments(text, &refs),
                }],
                created_at: row["createdAt"].as_i64().unwrap_or_default(),
                device_id: self.device_id.clone().unwrap_or_default(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            });
        }
        pending.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        self.messages.extend(pending);
    }
    pub(crate) fn cloud_server_commands(&self) -> bool {
        self.cloud.guest
            && self.cloud.guest_status["commandDeliveryMode"] == "server-admitted-v2"
            && self.cloud.guest_status["commandAdmissionVersion"] == 2
            && self.cloud.guest_status["commandApiAuthorized"] == true
            && self.cloud.guest_status["taskCommandCanEdit"] == true
            && !self.cached_only
    }
    pub(crate) fn cloud_command_cwd(&self) -> String {
        let cwd = self.cwd();
        if self.cloud.guest
            && self.cloud.guest_status["commandDeliveryMode"] == "server-admitted-v2"
            && self.selected.as_ref().and_then(|c| c["cwd"].as_str()) == Some(cwd.as_str())
        {
            ".".into()
        } else {
            cwd
        }
    }
    pub(crate) fn selected_cloud_run_selection(
        &self,
    ) -> Option<zuse_protocol::cloud_task_commands::RunSelection> {
        let chat = self.selected.as_ref()?;
        if self.model_dirty
            || self.model_save.is_some()
            || self.access_change_pending_for_selection()
            || chat["id"]
                .as_str()
                .is_some_and(|id| self.model_unconfirmed.contains(id))
        {
            return None;
        }
        if self.cloud.cached {
            // Authenticated history owns the disconnected default. A local
            // provider catalog or an unconfirmed picker cannot replace it.
            let (workspace, session, saved) = self.cloud.cached_run_selection.as_ref()?;
            if self.cloud.guest_workspace.as_deref() != Some(workspace.as_str())
                || chat["id"].as_str() != Some(session.as_str())
            {
                return None;
            }
            let target_confirmed = self.model_confirmed
                && self.model_host.confirmed
                && self.model_host.target.as_deref() == chat["deviceId"].as_str();
            if !target_confirmed {
                return Some(saved.clone());
            }
        }
        if self.model_change_pending_for_selection() {
            return None;
        }
        let provider = self.composer_provider()?;
        if provider["installed"] == false {
            return None;
        }
        let model = self.model.as_ref().filter(|m| !m.trim().is_empty())?;
        serde_json::from_value(json!({"harness":provider["id"],"model":model,"reasoning":self.reasoning,"modelOptions":self.model_options,"sandbox":self.sandbox})).ok()
    }
    pub(crate) fn submit_saved_cloud_command(&mut self, cx: &mut Context<Self>) {
        if !self.cloud_server_commands() || self.uploading > 0 || !self.send_draft.is_empty() {
            return;
        }
        if self.queue_edit.is_some() {
            self.commit_queue_edit(cx);
            return;
        }
        let Some(chat) = self.selected.clone() else {
            return;
        };
        let text = self.input.read(cx).value(cx);
        let paths = self
            .attachment_paths()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if text.trim().is_empty() && paths.is_empty() {
            return;
        }
        let Some(selection) = self.selected_cloud_run_selection() else {
            self.error=Some("Choose an available agent and model, and wait for its settings to finish saving. Your draft is preserved.".into());
            cx.notify();
            return;
        };
        let message = uuid::Uuid::new_v4().to_string();
        let draft = Draft {
            text: text.clone(),
            attachments: self.attachments.clone(),
            key: self.draft_key(),
            chat_id: chat["id"].as_str().map(str::to_owned),
            message_id: Some(message.clone()),
            host_id: self.draft_host(),
            not_sent: false,
        };
        self.persist_outgoing_draft(draft.clone(), cx);
        let command = json!({"kind":"run","messageId":message,"request":{"prompt":zuse_protocol::message_attachments::with_attachments(&text,&paths),"harness":selection.harness,"model":selection.model,"reasoning":selection.reasoning,"modelOptions":selection.model_options,"cwd":".","sandbox":selection.sandbox,"autoApprove":selection.sandbox==zuse_protocol::SandboxLevel::DangerFullAccess,"resume":null,"worktree":null,"mcp":null,"attachments":paths}});
        let id = self.call(
            methods::QUEUE_COMMAND,
            zuse_protocol::presentation::chat_request(
                &chat,
                json!({"command":command,"commandDeliveryMode":"server-admitted-v2","wake":"explicit-resume"}),
            ),
        );
        self.send_draft.insert(id, draft);
        self.input
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.attachments.clear();
        cx.notify();
    }
}

#[cfg(test)]
mod tests;
