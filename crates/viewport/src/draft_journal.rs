//! Persist only local copies; engine delivery and queue leases are never replayed.
use super::*;
use zuse_protocol::browser_drafts::DraftJournal;

impl WorkspaceView {
    pub(super) fn retry_draft_storage(&mut self, cx: &mut Context<Self>) {
        if self.draft_journal_ready {
            self.draft_journal_last = None;
            self.persist_drafts(cx);
        } else if self.device_id.is_some() {
            let _ = self
                .requests
                .unbounded_send(json!({"ui":"loadDrafts","deviceId":self.device_id}));
        }
    }
    pub(super) fn draft_host(&self) -> Option<String> {
        self.selected
            .as_ref()
            .and_then(|chat| chat["deviceId"].as_str())
            .map(str::to_owned)
            .or_else(|| self.device_id.clone())
    }
    fn current_draft(&self, cx: &App) -> Draft {
        Draft {
            text: self.input.read(cx).value(cx),
            attachments: self.attachments.clone(),
            key: self.draft_key(),
            chat_id: self
                .selected
                .as_ref()
                .and_then(|chat| chat["id"].as_str())
                .map(str::to_owned),
            host_id: self.draft_host(),
            not_sent: true,
            ..Default::default()
        }
    }
    fn journal(&self, cx: &App) -> DraftJournal {
        let mut drafts = self.drafts.clone();
        let mut recovery = DraftJournal::default();
        for draft in self.uncertain_drafts.iter().chain(self.send_draft.values()) {
            recovery.keep_recovery(draft.clone());
        }
        let current = if let Some((mut previous, edit)) = self.queue_edit_journal(cx) {
            previous.not_sent = true;
            recovery.keep_recovery(edit);
            previous
        } else {
            self.current_draft(cx)
        };
        if current.is_empty() {
            drafts.remove(&current.key);
        } else {
            drafts.insert(current.key.clone(), current);
        }
        let mut drafts = drafts
            .into_values()
            .filter(|draft| !draft.is_empty())
            .collect::<Vec<_>>();
        drafts.sort_by(|a, b| a.key.cmp(&b.key));
        recovery.recovery.sort_by(|a, b| {
            a.key
                .cmp(&b.key)
                .then(a.message_id.cmp(&b.message_id))
                .then(a.text.cmp(&b.text))
        });
        DraftJournal { drafts, ..recovery }
    }
    pub(super) fn persist_drafts(&mut self, cx: &mut Context<Self>) {
        if !self.draft_journal_ready {
            return;
        }
        self.write_draft_journal(self.journal(cx));
    }
    pub(super) fn persist_outgoing_draft(&mut self, draft: Draft, cx: &App) {
        if !self.draft_journal_ready {
            return;
        }
        let mut journal = self.journal(cx);
        journal.drafts.retain(|row| row.key != draft.key);
        journal.keep_recovery(draft);
        self.write_draft_journal(journal);
    }
    fn write_draft_journal(&mut self, journal: DraftJournal) {
        let value = serde_json::to_value(journal).unwrap();
        if self.draft_journal_last.as_ref() == Some(&value) {
            return;
        }
        self.draft_journal_last = Some(value.clone());
        let _ = self
            .requests
            .unbounded_send(json!({"ui":"saveDrafts","deviceId":self.device_id,"value":value}));
    }
    pub(super) fn receive_draft_journal(&mut self, frame: &Value, cx: &mut Context<Self>) -> bool {
        let Some(result) = frame.get("browserDrafts") else {
            return false;
        };
        if result["deviceId"].as_str() != self.device_id.as_deref() {
            return true;
        }
        if let Some(error) = result["error"].as_str() {
            self.draft_journal_error = Some(error.to_owned());
        } else {
            self.draft_journal_error = None;
        }
        if let Some(value) = result.get("journal") {
            match DraftJournal::decode(&value.to_string()) {
                Ok(journal) => {
                    self.stash_draft(cx);
                    for draft in journal.drafts {
                        if let Some(current) = self.drafts.get(&draft.key) {
                            if current != &draft && !self.uncertain_drafts.contains(&draft) {
                                self.uncertain_drafts.push(draft);
                            }
                        } else {
                            self.drafts.insert(draft.key.clone(), draft);
                        }
                    }
                    for draft in journal.recovery {
                        if !draft.is_empty() && !self.uncertain_drafts.contains(&draft) {
                            self.uncertain_drafts.push(draft);
                        }
                    }
                    if let Some(copies) = result["copies"].as_array() {
                        for copy in copies {
                            if let Ok(draft) = serde_json::from_value::<Draft>(copy.clone()) {
                                if !draft.is_empty()
                                    && !self.uncertain_drafts.contains(&draft)
                                    && !self.drafts.values().any(|current| current == &draft)
                                {
                                    self.uncertain_drafts.push(draft);
                                }
                            }
                        }
                    }
                    self.draft_journal_ready = true;
                    if self.queue_edit.is_none()
                        && self.input.read(cx).value(cx).is_empty()
                        && self.attachments.is_empty()
                    {
                        self.restore_draft(cx);
                    }
                    if !self.uncertain_drafts.is_empty() {
                        self.connection_notice = Some("Browser drafts were recovered. Copies from other tabs may also appear. Review the chat and queue before restoring a pending delivery or queue edit.".into());
                    }
                }
                Err(error) => self.draft_journal_error = Some(error.into()),
            }
        }
        cx.notify();
        true
    }
}
