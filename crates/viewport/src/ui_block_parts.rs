//! `show_ui` blocks in the browser transcript: the shared renderer, with
//! form replies sent as the selected chat's next message.
use super::*;
use crate::ui_block::{UiBlockEvent, UiBlockPalette, UiBlockView};
use std::cell::RefCell;

/// The transcript renders through `&self`; block views are created lazily.
#[derive(Default)]
pub(super) struct UiBlocks {
    views: RefCell<HashMap<String, Entity<UiBlockView>>>,
    /// Replies the engine has not answered yet, by request id. Each block
    /// stays "Sending" until its request comes back.
    replies: HashMap<u64, Entity<UiBlockView>>,
}

impl WorkspaceView {
    pub(super) fn ui_block_part(
        &self,
        part: &Value,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let view = self.ui_block_view(part, interactive, cx);
        div().w_full().child(view).into_any_element()
    }

    /// The block's view, made on first sight and kept while its spec holds.
    fn ui_block_view(
        &self,
        part: &Value,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> Entity<UiBlockView> {
        let id = part["id"].as_str().unwrap_or_default().to_owned();
        let spec = part["spec"].as_str().unwrap_or_default();
        let existing = self
            .ui_blocks
            .views
            .borrow()
            .get(&id)
            .filter(|view| view.read(cx).source() == spec)
            .cloned();
        let view = existing.unwrap_or_else(|| {
            let view = cx
                .new(|cx| UiBlockView::new(spec, UiBlockPalette::reference(true), interactive, cx));
            let block = id.clone();
            cx.subscribe(&view, move |this, view, event: &UiBlockEvent, cx| {
                let UiBlockEvent::Send { text } = event;
                match this.send_ui_block_reply(text.clone(), cx) {
                    Some(request) => {
                        this.ui_blocks.replies.insert(request, view);
                    }
                    None => {
                        view.update(cx, |view, cx| view.mark_send_failed(cx));
                        // A guard names its own reason; otherwise say which
                        // block's answer did not go out.
                        this.error.get_or_insert_with(|| {
                            format!("Couldn't send your response from {block}. Open this chat to send it.")
                        });
                        cx.notify();
                    }
                }
            })
            .detach();
            self.ui_blocks.views.borrow_mut().insert(id, view.clone());
            view
        });
        view.update(cx, |view, cx| view.set_interactive(interactive, cx));
        view
    }

    /// Send a UI block's reply as the selected chat's next message, leaving
    /// the composer's draft alone. Queued behind a running turn. Checks what
    /// [`Self::submit`] checks before a send, and returns the request id, or
    /// `None` when nothing went out.
    fn send_ui_block_reply(&mut self, text: String, cx: &mut Context<Self>) -> Option<u64> {
        if !self.require_connection(cx) || self.transcript_blocked() {
            return None;
        }
        let chat = self.selected.clone()?;
        if chat["archived"] == true {
            self.error = Some("Unarchive this thread before sending another message.".into());
            return None;
        }
        if self.model_change_pending_for_selection() {
            self.error = Some("Wait for the model choices to finish saving before sending.".into());
            return None;
        }
        if self.access_change_pending_for_selection() {
            self.error = Some("Wait for the saved permission mode before sending.".into());
            return None;
        }
        if !self.workspace_synced || !self.chats.iter().any(|row| row["id"] == chat["id"]) {
            self.error = Some("Wait for the conversation list to refresh before sending.".into());
            return None;
        }
        let request = if self.is_working() {
            self.call(
                methods::QUEUE_MESSAGE,
                zuse_protocol::presentation::chat_request(
                    &chat,
                    json!({"text": text, "holdForTurnEnd": true, "attachments": []}),
                ),
            )
        } else {
            let provider = self.composer_provider().cloned()?;
            if provider["installed"] == false {
                self.error = Some(format!(
                    "{} is not installed on the engine host.",
                    provider["name"].as_str().unwrap_or("This agent")
                ));
                return None;
            }
            let command = json!({"kind":"run","messageId":uuid::Uuid::new_v4().to_string(),"request":{"prompt":text,"harness":provider["id"],"model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options,"cwd":self.cwd(),"sandbox":self.sandbox,"plan":chat["config"]["plan"] == true,"autoApprove":self.sandbox==zuse_protocol::SandboxLevel::DangerFullAccess,"resume":null,"attachments":[],"worktree":null}});
            self.call(
                methods::QUEUE_COMMAND,
                zuse_protocol::presentation::chat_request(&chat, json!({"command": command})),
            )
        };
        // A cached view never sends, and a closed channel already set the error.
        (request != 0 && self.pending.contains_key(&request) && self.connected).then_some(request)
    }

    /// Settle a block waiting on this frame's request: "Sent" once the engine
    /// accepts it, unlocked for a retry when it refuses. Leaves the frame for
    /// the usual handling (an error still shows in the banner).
    pub(super) fn receive_ui_block_reply(&mut self, frame: &Value, cx: &mut Context<Self>) {
        if frame.get("ok").is_none() && frame.get("err").is_none() {
            return;
        }
        let Some(view) = frame["id"]
            .as_u64()
            .and_then(|id| self.ui_blocks.replies.remove(&id))
        else {
            return;
        };
        let accepted = frame.get("ok").is_some();
        view.update(cx, |view, cx| {
            if accepted {
                view.mark_sent(cx);
            } else {
                view.mark_send_failed(cx);
            }
        });
    }

    /// The connection dropped with replies unanswered: unlock their blocks.
    pub(super) fn interrupt_ui_block_replies(&mut self, cx: &mut Context<Self>) {
        for (_, view) in self.ui_blocks.replies.drain() {
            view.update(cx, |view, cx| view.mark_send_failed(cx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, WindowHandle};
    use std::prelude::v1::test;

    const SPEC: &str = r#"root = FollowUps([{label: "Retry", prompt: "Retry the build"}])"#;

    /// A connected viewer on a working chat, with one rendered block.
    fn open(
        cx: &mut TestAppContext,
    ) -> (
        WindowHandle<WorkspaceView>,
        WeakEntity<UiBlockView>,
        futures::channel::mpsc::UnboundedReceiver<Value>,
        futures::channel::mpsc::UnboundedSender<Value>,
    ) {
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
        let block = window
            .update(cx, |view, _, cx| {
                view.connected = true;
                view.workspace_synced = true;
                view.connection_epoch = 1;
                view.device_id = Some("viewer".into());
                let chat = json!({"id": "chat", "deviceId": "host",
                    "config": {"harness": "claude-code", "sandbox": "workspace-write"}});
                view.selected = Some(chat.clone());
                view.chats = vec![chat];
                view.begin_transcript(cx);
                let watch = view.transcript_watch.unwrap();
                view.receive(json!({"id": watch, "item": {"reset": []}}), cx);
                view.model_confirmed = true;
                view.model_host.target = Some("host".into());
                view.model_host.confirmed = true;
                view.model_host.rows = vec![json!({"id": "claude-code", "installed": true})];
                view.sessions = vec![json!({"chatId": "chat", "status": "working"})];
                view.ui_block_view(&json!({"id": "block", "spec": SPEC}), true, cx)
                    .downgrade()
            })
            .unwrap();
        (window, block, rx, events)
    }

    fn reply_request(requests: &mut futures::channel::mpsc::UnboundedReceiver<Value>) -> u64 {
        let mut found = None;
        while let Ok(frame) = requests.try_recv() {
            if frame["method"] == methods::QUEUE_MESSAGE {
                assert_eq!(frame["params"]["text"], "Retry the build");
                found = frame["id"].as_u64();
            }
        }
        found.expect("the reply is queued behind the running turn")
    }

    #[gpui::test]
    fn a_reply_shows_sent_only_once_the_engine_accepts_it(cx: &mut TestAppContext) {
        let (window, block, mut requests, _events) = open(cx);
        block
            .update(cx, |block, cx| block.test_send("Retry the build", cx))
            .unwrap();
        cx.run_until_parked();
        let id = reply_request(&mut requests);
        assert_eq!(
            block.read_with(cx, |b, _| b.test_status()).unwrap(),
            "Sending"
        );
        window
            .update(cx, |view, _, cx| {
                view.receive(json!({"id": id, "ok": {}}), cx)
            })
            .unwrap();
        assert_eq!(block.read_with(cx, |b, _| b.test_status()).unwrap(), "Sent");
    }

    #[gpui::test]
    fn a_refused_reply_unlocks_the_block_for_a_retry(cx: &mut TestAppContext) {
        let (window, block, mut requests, _events) = open(cx);
        block
            .update(cx, |block, cx| block.test_send("Retry the build", cx))
            .unwrap();
        cx.run_until_parked();
        let id = reply_request(&mut requests);
        window
            .update(cx, |view, _, cx| {
                view.receive(json!({"id": id, "err": "queue is full"}), cx)
            })
            .unwrap();
        assert_eq!(block.read_with(cx, |b, _| b.test_status()).unwrap(), "Idle");
    }

    #[gpui::test]
    fn a_reply_waits_for_the_conversation_list_to_sync(cx: &mut TestAppContext) {
        let (window, block, mut requests, _events) = open(cx);
        window
            .update(cx, |view, _, _| view.workspace_synced = false)
            .unwrap();
        block
            .update(cx, |block, cx| block.test_send("Retry the build", cx))
            .unwrap();
        cx.run_until_parked();
        assert!(
            std::iter::from_fn(|| requests.try_recv().ok())
                .all(|frame| frame["method"] != methods::QUEUE_MESSAGE)
        );
        assert_eq!(block.read_with(cx, |b, _| b.test_status()).unwrap(), "Idle");
        window
            .update(cx, |view, _, _| {
                assert_eq!(
                    view.error.as_deref(),
                    Some("Wait for the conversation list to refresh before sending.")
                );
            })
            .unwrap();
    }
}
