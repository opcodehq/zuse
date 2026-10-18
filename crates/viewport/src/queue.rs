//! Host-scoped queue actions wait for confirmation and a fresh queue read.
use super::*;
use zuse_protocol::presentation::{QueueAction, chat_request, queue_row_details};

pub(super) struct PendingQueueAction {
    pub chat: String,
    pub action: QueueAction,
}
impl WorkspaceView {
    pub(super) fn queue_action_blocked(&self) -> bool {
        self.queue_edit.is_some()
            || self.model_change_pending_for_selection()
            || self.transcript_blocked()
            || !self.connected
            || !self.workspace_synced
            || self
                .selected
                .as_ref()
                .and_then(|chat| chat["id"].as_str())
                .is_none_or(|chat| {
                    self.queue_unconfirmed.contains(chat)
                        || self
                            .queue_actions
                            .values()
                            .any(|action| action.chat == chat)
                })
    }
    pub(super) fn queue_action(
        &mut self,
        action: QueueAction,
        item: &Value,
        position: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if !self.require_connection(cx) || self.queue_action_blocked() {
            return;
        }
        let Some(chat) = self.selected.clone() else {
            return;
        };
        let Some(chat_id) = chat["id"].as_str().map(str::to_owned) else {
            return;
        };
        let Some(current) = self.queue.iter().find(|row| row["id"] == item["id"]) else {
            return;
        };
        if !queue_row_details(current).allows(action) {
            self.error = Some("This queued message is blocked for editing/review, or needs an explicit send with its attachments.".into());
            cx.notify();
            return;
        }
        let mut params = json!({"id":current["id"]});
        if action == QueueAction::Move {
            let Some(position) = position.filter(|index| *index < self.queue.len()) else {
                return;
            };
            params["toIndex"] = json!(position);
        }
        let request = self.call(action.method(), chat_request(&chat, params));
        self.queue_actions.insert(
            request,
            PendingQueueAction {
                chat: chat_id,
                action,
            },
        );
        cx.notify();
    }
    pub(super) fn receive_queue_action(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.queue_actions.contains_key(&id) {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let pending = self.queue_actions.remove(&id).unwrap();
        self.pending.remove(&id);
        let error = if let Some(error) = frame["err"].as_str() {
            Some(error.to_owned())
        } else {
            match frame.get("ok").and_then(|reply| pending.action.confirmed(reply)) {
                Some(true) => None,
                Some(false) => Some("That queued message changed or was already taken. Refreshing its current state.".into()),
                None => Some("The host did not confirm the queue action. Check the refreshed queue before trying again.".into()),
            }
        };
        if let Some(error) = error {
            self.error = Some(error);
        }
        if let Some(chat) = self
            .selected
            .clone()
            .filter(|chat| chat["id"] == pending.chat)
        {
            self.refresh_queue_snapshot(&chat);
        } else {
            self.queue_unconfirmed.insert(pending.chat);
        }
        cx.notify();
        true
    }
    pub(super) fn refresh_queue_snapshot(&mut self, chat: &Value) {
        if let Some(id) = chat["id"].as_str() {
            self.queue_unconfirmed.insert(id.to_owned());
        }
        if !self.connected {
            return;
        }
        // An editor can retain the previous host after a handoff. Read from the
        // current chat authority; never send its old lease to the new host.
        let Some(chat) = self
            .selected
            .clone()
            .filter(|selected| selected["id"] == chat["id"])
        else {
            return;
        };
        if let Some(watch) = self.queue_watch.take() {
            self.cancel(watch);
        }
        self.queue_watch = Some(self.call(methods::WATCH_QUEUE, chat_request(&chat, json!({}))));
    }
    pub(super) fn rebind_chat_host(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.selected.clone() else {
            return;
        };
        let requests = self
            .queue_actions
            .iter()
            .filter(|(_, action)| chat["id"] == action.chat)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in requests {
            self.queue_actions.remove(&id);
            self.cancel(id);
        }
        self.interrupt_queue_edit(cx);
        self.prefetch_model_catalogs();
        self.stop_transcript();
        // Until the new snapshot arrives, old approval cards must not submit
        // answers to a different host. Canceled subscription frames are ignored.
        self.messages.clear();
        self.reset_context_status();
        self.queue.clear();
        let inputs = self.input_pending.keys().copied().collect::<Vec<_>>();
        for id in inputs {
            self.cancel(id);
        }
        self.reset_inputs(cx);
        self.reset_tool_results();
        self.reset_previews(cx);
        self.refresh_queue_snapshot(&chat);
        self.begin_transcript(cx);
        self.connection_notice = Some("This chat moved to another computer. Check any action already sent before trying it again.".into());
    }
}
