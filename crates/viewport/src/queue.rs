//! Host-scoped queue actions wait for confirmation and a fresh queue read.
use super::*;
use zuse_protocol::presentation::{QueueAction, chat_request, queue_row_details};

pub(super) struct PendingQueueAction {
    pub chat: String,
    pub action: QueueAction,
}
impl WorkspaceView {
    pub(super) fn queue_action_blocked(&self) -> bool {
        !self.connected
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
        self.queue_unconfirmed.insert(pending.chat.clone());
        if let Some(chat) = self
            .selected
            .clone()
            .filter(|chat| chat["id"] == pending.chat)
        {
            if let Some(watch) = self.queue_watch.take() {
                self.cancel(watch);
            }
            self.queue_watch =
                Some(self.call(methods::WATCH_QUEUE, chat_request(&chat, json!({}))));
        }
        cx.notify();
        true
    }
}
