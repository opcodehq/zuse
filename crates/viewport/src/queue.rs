//! Host-scoped queue actions wait for confirmation and a fresh queue read.
use super::*;
use zuse_protocol::presentation::{QueueAction, chat_request, queue_row_details};

pub(super) struct PendingQueueAction {
    pub chat: String,
    pub action: QueueAction,
}
impl WorkspaceView {
    pub(super) fn queued_primary_action(
        &self,
        item: &Value,
    ) -> Option<zuse_protocol::presentation::QueuePrimaryAction> {
        let details = queue_row_details(item);
        let provider = self.composer_provider()?;
        let supports = provider["supportsSteering"].as_bool()?;
        let mode = provider["steeringMode"].as_str()?;
        if !matches!(mode, "step-boundary" | "turn-boundary") {
            return None;
        }
        let mid_turn = supports && mode == "step-boundary";
        zuse_protocol::presentation::queue_primary_action(
            self.queue_action_blocked() || !details.valid_id || details.blocked.is_some(),
            true,
            details.attachment_count > 0,
            mid_turn,
        )
    }

    pub(super) fn submit_composer(
        &mut self,
        secondary: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.browser_voice_replaces_composer() {
            return;
        }
        if secondary
            && self.queue_edit.is_none()
            && !self.settings
            && self.input.read(cx).focus_handle(cx).is_focused(window)
            && self.input.read(cx).value(cx).trim().is_empty()
            && self.attachments.is_empty()
        {
            if self.uploading == 0 && self.creating.is_none() {
                if let Some(item) = self.queue.last().cloned() {
                    if let Some(action) = self.queued_primary_action(&item) {
                        self.queue_action(action.action(), &item, None, cx);
                    }
                }
            }
            return;
        }
        self.submit(&Submit, window, cx);
    }

    pub(super) fn move_queued_drop(
        &mut self,
        payload: &crate::queue_render::QueueDrag,
        target: &str,
        cx: &mut Context<Self>,
    ) {
        if self.selected.as_ref().is_none_or(|chat| {
            chat["id"].as_str() != Some(payload.chat.as_str())
                || chat["deviceId"].as_str() != payload.host.as_deref()
        }) || payload.id == target
        {
            return;
        }
        let Some(position) = self.queue.iter().position(|item| item["id"] == target) else {
            return;
        };
        let Some(item) = self
            .queue
            .iter()
            .find(|item| item["id"] == payload.id)
            .cloned()
        else {
            return;
        };
        self.queue_action(QueueAction::Move, &item, Some(position), cx);
    }

    pub(super) fn queue_action_blocked(&self) -> bool {
        self.queue_edit.is_some()
            || self.model_change_pending_for_selection()
            || self.access_change_pending_for_selection()
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
        self.reset_project_actions(cx);
        self.detach_browser_terminal(cx);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    struct Fixture {
        window: WindowHandle<WorkspaceView>,
        requests: UnboundedReceiver<Value>,
        _events: UnboundedSender<Value>,
    }
    fn fixture(cx: &mut TestAppContext) -> Fixture {
        cx.update(initialize);
        let (requests, rx) = futures::channel::mpsc::unbounded();
        let (events, frames) = futures::channel::mpsc::unbounded();
        let window = cx.add_window(|window, cx| {
            WorkspaceView::new(
                Transport {
                    requests,
                    events: frames,
                },
                window,
                cx,
            )
        });
        window.update(cx,|view,window,cx|{
            view.connected=true;view.workspace_synced=true;view.connection_epoch=1;view.device_id=Some("viewer".into());
            let chat=json!({"id":"chat","deviceId":"host","config":{"harness":"claude-code","sandbox":"workspace-write"}});
            view.selected=Some(chat.clone());view.chats=vec![chat];view.begin_transcript(cx);let watch=view.transcript_watch.unwrap();view.receive(json!({"id":watch,"item":{"reset":[]}}),cx);view.model_confirmed=true;
            view.model_host.target=Some("host".into());view.model_host.confirmed=true;
            view.model_host.rows=vec![json!({"id":"claude-code","installed":true,"supportsSteering":true,"steeringMode":"step-boundary"})];
            view.sessions=vec![json!({"chatId":"chat","status":"working"})];
            view.queue=vec![json!({"id":"older","text":"first"}),json!({"id":"newest","text":"second"})];
            window.focus(&view.input.read(cx).focus_handle(cx),cx);
        }).unwrap();
        Fixture {
            window,
            requests: rx,
            _events: events,
        }
    }
    fn mutations(f: &mut Fixture) -> Vec<Value> {
        let mut out = vec![];
        while let Ok(v) = f.requests.try_recv() {
            if matches!(
                v["method"].as_str(),
                Some(
                    methods::STEER_QUEUED_MESSAGE_NOW
                        | methods::SEND_QUEUED_MESSAGE_NOW
                        | methods::MOVE_QUEUED_MESSAGE
                        | methods::QUEUE_MESSAGE
                        | methods::QUEUE_COMMAND
                )
            ) {
                out.push(v);
            }
        }
        out
    }

    #[gpui::test]
    fn queue_shortcut_uses_the_newest_rows_actual_delivery_action(cx: &mut TestAppContext) {
        for attachments in [false, true] {
            let mut f = fixture(cx);
            f.window
                .update(cx, |view, window, cx| {
                    if attachments {
                        view.queue[1]["attachments"] = json!(["/host/image.png"]);
                    }
                    view.submit_composer(false, window, cx);
                    view.submit_composer(true, window, cx);
                    view.submit_composer(true, window, cx); // acknowledgement is still pending
                })
                .unwrap();
            let writes = mutations(&mut f);
            assert_eq!(writes.len(), 1);
            assert_eq!(
                writes[0]["method"],
                if attachments {
                    methods::SEND_QUEUED_MESSAGE_NOW
                } else {
                    methods::STEER_QUEUED_MESSAGE_NOW
                }
            );
            assert_eq!(
                writes[0]["params"],
                json!({"chatId":"chat","targetDeviceId":"host","id":"newest"})
            );
        }
    }
    #[gpui::test]
    fn queue_shortcut_keeps_content_queued_and_never_skips_a_blocked_latest_row(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx);
        f.window
            .update(cx, |view, window, cx| {
                view.queue[1]["deliveryGate"] = json!({"kind":"reviewRequired"});
                view.submit_composer(true, window, cx);
                view.queue[1]["deliveryGate"] = Value::Null;
                view.access_unconfirmed = Some("chat".into());
                view.submit_composer(true, window, cx);
                view.access_unconfirmed = None;
                view.input.update(cx, |input, cx| {
                    input.set_value("continue carefully".into(), cx)
                });
                view.attachments = vec![json!({"path":"/host/context.txt"})];
            })
            .unwrap();
        assert!(mutations(&mut f).is_empty());
        f.window
            .update(cx, |view, window, cx| {
                view.submit_composer(true, window, cx)
            })
            .unwrap();
        let writes = mutations(&mut f);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0]["method"], methods::QUEUE_MESSAGE);
        assert_eq!(
            writes[0]["params"],
            json!({"chatId":"chat","targetDeviceId":"host","text":"continue carefully","holdForTurnEnd":true,"attachments":["/host/context.txt"]})
        );
    }
    #[gpui::test]
    fn queue_primary_label_follows_execution_host_and_drag_rechecks_row_ids(
        cx: &mut TestAppContext,
    ) {
        let mut f = fixture(cx);
        f.window
            .update(cx, |view, _, cx| {
                use zuse_protocol::presentation::QueuePrimaryAction;
                assert_eq!(
                    view.queued_primary_action(&view.queue[0]),
                    Some(QueuePrimaryAction::Steer)
                );
                view.model_host.rows[0]["steeringMode"] = json!("turn-boundary");
                assert_eq!(
                    view.queued_primary_action(&view.queue[0]),
                    Some(QueuePrimaryAction::SendNext)
                );
                view.model_host.confirmed = false;
                assert_eq!(view.queued_primary_action(&view.queue[0]), None);
                let mut drag = crate::queue_render::QueueDrag {
                    chat: "chat".into(),
                    host: Some("retired-host".into()),
                    id: "older".into(),
                    title: "first".into(),
                };
                view.move_queued_drop(&drag, "newest", cx);
                drag.host = Some("host".into());
                view.move_queued_drop(&drag, "older", cx);
                view.move_queued_drop(&drag, "removed", cx);
                view.queue
                    .insert(0, json!({"id":"inserted","text":"new row"}));
                view.move_queued_drop(&drag, "newest", cx);
            })
            .unwrap();
        let writes = mutations(&mut f);
        assert_eq!(writes.len(), 1);
        assert_eq!(
            writes[0]["params"],
            json!({"chatId":"chat","targetDeviceId":"host","id":"older","toIndex":2})
        );
    }
}
