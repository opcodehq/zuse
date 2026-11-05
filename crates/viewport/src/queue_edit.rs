//! A held queue row edits in the composer while the user's draft stays aside.
use super::*;
use crate::shell::{control, label, row};
use zuse_protocol::presentation::{
    QueueEditLease, chat_request, queue_edit_lease, queue_edit_review_matches,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditOp {
    Acquire,
    Renew,
    Commit,
    Discard,
    Cancel,
}
struct PendingEdit {
    id: u64,
    op: EditOp,
    ticks: u8,
    submitted: Option<Draft>,
}
pub(super) struct QueueEdit {
    tag: String,
    chat: Value,
    item: String,
    revision: Option<i64>,
    lease: Option<QueueEditLease>,
    previous: Option<Draft>,
    pending: Option<PendingEdit>,
    uncertain: bool,
    review_only: bool,
    finish_failed: bool,
    renew_ticks: u8,
    _timer: Option<Task<()>>,
}
impl WorkspaceView {
    pub(super) fn queue_edit_journal(&self, cx: &App) -> Option<(Draft, Draft)> {
        let edit = self.queue_edit.as_ref()?;
        let previous = edit.previous.clone()?;
        let mut current = self.queue_edit_copy(cx);
        current.host_id = edit.chat["deviceId"].as_str().map(str::to_owned);
        Some((previous, current))
    }
    pub(super) fn queue_edit_navigation_blocked(&mut self, cx: &mut Context<Self>) -> bool {
        if self.queue_edit.is_some() {
            self.error =
                Some("Save or cancel the queued-message edit before changing chats.".into());
            cx.notify();
            true
        } else {
            false
        }
    }
    fn queue_edit_host_matches(&self, edit: &QueueEdit) -> bool {
        self.selected.as_ref().is_some_and(|chat| {
            chat["id"] == edit.chat["id"] && chat["deviceId"] == edit.chat["deviceId"]
        })
    }
    pub(super) fn begin_queue_edit(&mut self, item: Value, cx: &mut Context<Self>) {
        if self.queue_action_blocked()
            || self.queue_edit.is_some()
            || self.uploading > 0
            || self.creating.is_some()
            || !self.send_draft.is_empty()
        {
            return;
        }
        let Some(chat) = self.selected.clone() else {
            return;
        };
        if !self.queue.iter().any(|row| row["id"] == item["id"]) {
            return;
        }
        let Some(id) = item["id"].as_str().filter(|id| !id.is_empty()) else {
            return;
        };
        let tag = uuid::Uuid::new_v4().to_string();
        let request = self.call(
            methods::BEGIN_QUEUED_MESSAGE_EDIT,
            chat_request(
                &chat,
                json!({"id":id,"revision":item["revision"],"editorDeviceId":self.device_id,"editorInstanceId":tag}),
            ),
        );
        self.queue_edit = Some(QueueEdit {
            tag: tag.clone(),
            chat,
            item: id.into(),
            revision: item["revision"].as_i64(),
            lease: None,
            previous: None,
            pending: Some(PendingEdit {
                id: request,
                op: EditOp::Acquire,
                ticks: 0,
                submitted: None,
            }),
            uncertain: false,
            review_only: false,
            finish_failed: false,
            renew_ticks: 0,
            _timer: None,
        });
        let timer = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(5))
                    .await;
                match this.update(cx, |this, cx| this.queue_edit_tick(&tag, cx)) {
                    Ok(true) => {}
                    _ => break,
                }
            }
        });
        self.queue_edit.as_mut().unwrap()._timer = Some(timer);
        cx.notify();
    }
    fn queue_edit_tick(&mut self, tag: &str, cx: &mut Context<Self>) -> bool {
        let command_online = self.connected || self.cloud_server_commands();
        let host_matches = self
            .queue_edit
            .as_ref()
            .is_some_and(|edit| self.queue_edit_host_matches(edit));
        let Some(edit) = self.queue_edit.as_mut().filter(|edit| edit.tag == tag) else {
            return false;
        };
        if !host_matches {
            edit.uncertain = true;
        }
        if let Some(pending) = &mut edit.pending {
            pending.ticks = pending.ticks.saturating_add(1);
            if pending.ticks >= 3 {
                let pending = edit.pending.take().unwrap();
                edit.uncertain = true;
                edit.finish_failed |= matches!(
                    pending.op,
                    EditOp::Commit | EditOp::Cancel | EditOp::Discard
                );
                self.cancel(pending.id);
                self.error = Some("The queue edit request was not confirmed. Your text is preserved; check the host or keep a copy before leaving.".into());
                cx.notify();
            }
            return true;
        }
        if edit.uncertain || edit.review_only || !command_online {
            return true;
        }
        edit.renew_ticks = edit.renew_ticks.saturating_add(1);
        if edit.lease.is_some() && edit.renew_ticks >= 4 {
            self.check_queue_edit(cx);
        }
        true
    }
    pub(super) fn check_queue_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.queue_edit.as_ref() else {
            return;
        };
        if (!self.connected && !self.cloud_server_commands())
            || !self.queue_edit_host_matches(edit)
            || edit.pending.is_some()
        {
            return;
        }
        let Some(lease) = edit.lease.as_ref() else {
            return;
        };
        let params = chat_request(&edit.chat, json!({"id":edit.item,"leaseId":lease.id}));
        let id = self.call(methods::RENEW_QUEUED_MESSAGE_EDIT, params);
        self.queue_edit.as_mut().unwrap().pending = Some(PendingEdit {
            id,
            op: EditOp::Renew,
            ticks: 0,
            submitted: None,
        });
        cx.notify();
    }
    pub(super) fn interrupt_queue_edit(&mut self, cx: &mut Context<Self>) {
        let mut request = None;
        if let Some(edit) = self.queue_edit.as_mut() {
            edit.finish_failed |= edit.pending.as_ref().is_some_and(|pending| {
                matches!(
                    pending.op,
                    EditOp::Commit | EditOp::Cancel | EditOp::Discard
                )
            });
            request = edit.pending.take().map(|pending| pending.id);
            edit.uncertain = true;
            cx.notify();
        }
        if let Some(id) = request {
            self.cancel(id);
        }
    }
    fn queue_edit_copy(&self, cx: &App) -> Draft {
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
            not_sent: false,
            ..Default::default()
        }
    }
    fn leave_queue_edit(&mut self, keep_copy: bool, cx: &mut Context<Self>) {
        let Some(edit) = self.queue_edit.take() else {
            return;
        };
        if let Some(pending) = edit.pending {
            self.cancel(pending.id);
        }
        if let Some(previous) = edit.previous {
            if keep_copy {
                let mut draft = self.queue_edit_copy(cx);
                draft.host_id = edit.chat["deviceId"].as_str().map(str::to_owned);
                if !draft.text.is_empty() || !draft.attachments.is_empty() {
                    self.uncertain_drafts.push(draft);
                }
                self.connection_notice = Some("A copy of the queue edit was kept. Check the queue and transcript before restoring it; its save may have completed.".into());
            }
            self.composer_generation = self.composer_generation.wrapping_add(1);
            self.uploading = 0;
            self.attachments = previous.attachments;
            self.input.update(cx, |input, cx| {
                input.set_value(previous.text, cx);
                input.focus(cx);
            });
            self.sync_previews(true, cx);
        }
        self.refresh_queue_snapshot(&edit.chat);
        cx.notify();
    }
    pub(super) fn cancel_queue_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.queue_edit.as_ref() else {
            return;
        };
        if edit.lease.is_none()
            || edit.uncertain
            || (!self.connected && !self.cloud_server_commands())
            || !self.queue_edit_host_matches(edit)
        {
            self.leave_queue_edit(edit.previous.is_some(), cx);
            return;
        }
        if edit.pending.is_some() {
            return;
        }
        self.finish_queue_edit(EditOp::Cancel, cx);
    }
    pub(super) fn commit_queue_edit(&mut self, cx: &mut Context<Self>) {
        if self.access_change_pending_for_selection() || self.uploading > 0 {
            self.error = Some(
                "Wait for uploads and permission changes before saving the queue edit.".into(),
            );
            cx.notify();
            return;
        }
        let op = if self.input.read(cx).value(cx).trim().is_empty() && self.attachments.is_empty() {
            EditOp::Discard
        } else {
            EditOp::Commit
        };
        self.finish_queue_edit(op, cx);
    }
    fn finish_queue_edit(&mut self, op: EditOp, cx: &mut Context<Self>) {
        let Some(edit) = self.queue_edit.as_ref() else {
            return;
        };
        if edit.pending.is_some() {
            return;
        }
        if (!self.connected && !self.cloud_server_commands())
            || edit.uncertain
            || !self.queue_edit_host_matches(edit)
        {
            self.error = Some(
                "Check the queue edit with its host before saving. Your text is preserved.".into(),
            );
            cx.notify();
            return;
        }
        let Some(lease) = edit.lease.as_ref() else {
            return;
        };
        let submitted = self.queue_edit_copy(cx);
        let params = chat_request(
            &edit.chat,
            json!({"id":edit.item,"revision":edit.revision,"leaseId":lease.id,"expectedTextHash":lease.base_text_hash,
            "action":match op {EditOp::Cancel=>"cancel",EditOp::Discard=>"discard",_=>"commit"},
            "text":submitted.text,"attachments":self.attachment_paths()}),
        );
        let id = self.call(methods::FINISH_QUEUED_MESSAGE_EDIT, params);
        self.queue_edit.as_mut().unwrap().pending = Some(PendingEdit {
            id,
            op,
            ticks: 0,
            submitted: Some(submitted),
        });
        cx.notify();
    }
    pub(super) fn receive_queue_edit(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .queue_edit
            .as_ref()
            .and_then(|edit| edit.pending.as_ref())
            .is_some_and(|pending| pending.id == id)
        {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let pending = self.queue_edit.as_mut().unwrap().pending.take().unwrap();
        self.pending.remove(&id);
        let invalid_reply = Value::Null;
        let reply = if frame.get("err").is_none() {
            &frame["ok"]
        } else {
            &invalid_reply
        };
        if pending.op == EditOp::Acquire {
            if let Some(lease) = queue_edit_lease(reply) {
                self.queue_edit.as_mut().unwrap().lease = Some(lease.clone());
                if self.uploading > 0
                    || !self.send_draft.is_empty()
                    || !self.queue_edit_host_matches(self.queue_edit.as_ref().unwrap())
                {
                    self.finish_queue_edit(EditOp::Cancel, cx);
                } else {
                    let previous = self.queue_edit_copy(cx);
                    self.queue_edit.as_mut().unwrap().previous = Some(previous);
                    let parsed = zuse_protocol::message_attachments::parse(&lease.text);
                    let mut paths = lease.attachments;
                    for file in parsed.attachments {
                        if !paths.contains(&file.path) {
                            paths.push(file.path);
                        }
                    }
                    self.attachments = paths
                        .into_iter()
                        .map(|path| {
                            let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_owned();
                            json!({"path":path,"name":name})
                        })
                        .collect();
                    self.composer_generation = self.composer_generation.wrapping_add(1);
                    self.input.update(cx, |input, cx| {
                        input.set_value(parsed.text, cx);
                        input.focus(cx);
                    });
                    self.sync_previews(true, cx);
                    self.queue_edit.as_mut().unwrap().renew_ticks = 0;
                }
            } else {
                self.error = Some(if reply["outcome"] == "locked" {
                    "This queued message is being edited in another window or device.".into()
                } else if reply["outcome"] == "missing" {
                    "This queued message has already been taken.".into()
                } else {
                    frame["err"].as_str().unwrap_or("The host did not confirm a valid edit lease. Check the queue before trying again.").to_owned()
                });
                self.leave_queue_edit(false, cx);
            }
        } else if pending.op == EditOp::Renew {
            let edit = self.queue_edit.as_mut().unwrap();
            edit.uncertain = reply["outcome"] != "renewed"
                || !reply["expiresAtMs"]
                    .as_i64()
                    .is_some_and(|expires| expires > 0);
            if !edit.uncertain {
                edit.finish_failed = false;
            }
            edit.renew_ticks = 0;
            if edit.uncertain {
                self.error=Some("The edit lease could not be renewed. Your changes are preserved; review the queue before saving.".into());
            }
            self.reconcile_queue_edit(cx);
        } else {
            let expected = match pending.op {
                EditOp::Cancel => "cancelled",
                EditOp::Discard => "discarded",
                _ => "committed",
            };
            if reply["outcome"] == expected {
                let changed = pending.op != EditOp::Cancel
                    && pending.submitted.as_ref().is_some_and(|sent| {
                        sent.text != self.input.read(cx).value(cx)
                            || sent.attachments != self.attachments
                    });
                self.leave_queue_edit(changed, cx);
            } else {
                let edit = self.queue_edit.as_mut().unwrap();
                edit.uncertain = true;
                edit.finish_failed = true;
                self.error=Some(match reply["outcome"].as_str() {Some("conflict")=>"The queued text changed elsewhere. Your edit is preserved; keep a copy and review the latest row.",
                    Some("missing"|"lost")=>"The queued row or edit lease is no longer available. Keep a copy before leaving.",
                    _=>frame["err"].as_str().unwrap_or("The host did not confirm this queue edit. Check its state before trying again.")}.into());
                self.reconcile_queue_edit(cx);
            }
        }
        cx.notify();
        true
    }
    pub(super) fn reconcile_queue_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.queue_edit.as_ref() else {
            return;
        };
        if !self.queue_edit_host_matches(edit) || edit.finish_failed {
            return;
        }
        let Some(lease) = edit.lease.as_ref() else {
            return;
        };
        let review = self
            .queue
            .iter()
            .any(|row| row["id"] == edit.item && queue_edit_review_matches(row, &lease.id));
        if review && edit.pending.is_none() {
            let edit = self.queue_edit.as_mut().unwrap();
            edit.review_only = true;
            edit.uncertain = false;
            cx.notify();
        }
    }
    pub(super) fn queue_edit_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(edit) = self.queue_edit.as_ref() else {
            return div().into_any_element();
        };
        let uncertain = edit.uncertain
            || (!self.connected && !self.cloud_server_commands())
            || !self.queue_edit_host_matches(edit);
        let pending = edit.pending.as_ref().map(|pending| pending.op);
        let can_save = edit.lease.is_some()
            && !uncertain
            && pending.is_none()
            && self.uploading == 0
            && !self.access_change_pending_for_selection();
        row()
            .id("queued-message-editor")
            .min_h(px(28.))
            .mb_1()
            .px(px(10.))
            .gap_2()
            .rounded(px(6.))
            .bg(rgb(0x202020).opacity(0.35))
            .child(
                label(if pending == Some(EditOp::Acquire) {
                    "Opening queued message…"
                } else if edit.review_only {
                    "Review queued-message changes"
                } else {
                    "Editing queued message"
                })
                .text_size(px(11.))
                .flex_1()
                .min_w_0(),
            )
            .when(
                uncertain && edit.lease.is_some() && pending.is_none(),
                |view| {
                    view.child(
                        control("check-queue-edit", "Check host")
                            .h(px(24.))
                            .text_size(px(11.))
                            .on_click(cx.listener(|this, _, _, cx| this.check_queue_edit(cx))),
                    )
                },
            )
            .child(
                control(
                    "save-queue-edit",
                    if pending == Some(EditOp::Commit) || pending == Some(EditOp::Discard) {
                        "Saving…"
                    } else {
                        "Save changes"
                    },
                )
                .h(px(24.))
                .text_size(px(11.))
                .when(!can_save, |view| view.opacity(0.4))
                .when(can_save, |view| {
                    view.on_click(cx.listener(|this, _, _, cx| this.commit_queue_edit(cx)))
                }),
            )
            .child(
                control(
                    "cancel-queue-edit",
                    if uncertain && edit.previous.is_some() {
                        "Keep copy and exit"
                    } else {
                        "Cancel"
                    },
                )
                .h(px(24.))
                .text_size(px(11.))
                .on_click(cx.listener(|this, _, _, cx| this.cancel_queue_edit(cx))),
            )
            .into_any_element()
    }
}
