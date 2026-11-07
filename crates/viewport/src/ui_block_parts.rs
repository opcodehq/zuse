//! `show_ui` blocks in the browser transcript: the shared renderer, with
//! form replies sent as the selected chat's next message.
use super::*;
use crate::ui_block::{UiBlockEvent, UiBlockPalette, UiBlockView};
use std::cell::RefCell;

/// The transcript renders through `&self`; block views are created lazily.
#[derive(Default)]
pub(super) struct UiBlocks(RefCell<HashMap<String, Entity<UiBlockView>>>);

impl WorkspaceView {
    pub(super) fn ui_block_part(
        &self,
        part: &Value,
        interactive: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = part["id"].as_str().unwrap_or_default().to_owned();
        let spec = part["spec"].as_str().unwrap_or_default();
        let existing = self
            .ui_blocks
            .0
            .borrow()
            .get(&id)
            .filter(|view| view.read(cx).source() == spec)
            .cloned();
        let view = existing.unwrap_or_else(|| {
            let view =
                cx.new(|cx| UiBlockView::new(spec, UiBlockPalette::reference(true), interactive, cx));
            let block = id.clone();
            cx.subscribe(&view, move |this, view, event: &UiBlockEvent, cx| {
                let UiBlockEvent::Send { text } = event;
                let sent = this.send_ui_block_reply(text.clone(), cx);
                view.update(cx, |view, cx| {
                    if sent {
                        view.mark_sent(cx);
                    } else {
                        view.mark_send_failed(cx);
                    }
                });
                if !sent {
                    this.error = Some(format!(
                        "Couldn't send your response from {block}. Open this chat to send it."
                    ));
                    cx.notify();
                }
            })
            .detach();
            self.ui_blocks.0.borrow_mut().insert(id, view.clone());
            view
        });
        view.update(cx, |view, cx| view.set_interactive(interactive, cx));
        div().w_full().child(view).into_any_element()
    }

    /// Send a UI block's reply as the selected chat's next message, leaving
    /// the composer's draft alone. Queued behind a running turn.
    fn send_ui_block_reply(&mut self, text: String, cx: &mut Context<Self>) -> bool {
        if !self.require_connection(cx) || self.transcript_blocked() {
            return false;
        }
        let Some(chat) = self.selected.clone() else {
            return false;
        };
        if chat["archived"] == true {
            return false;
        }
        if self.is_working() {
            self.call(
                methods::QUEUE_MESSAGE,
                zuse_protocol::presentation::chat_request(
                    &chat,
                    json!({"text": text, "holdForTurnEnd": true, "attachments": []}),
                ),
            );
            return true;
        }
        let Some(provider) = self.composer_provider().cloned() else {
            return false;
        };
        let command = json!({"kind":"run","messageId":uuid::Uuid::new_v4().to_string(),"request":{"prompt":text,"harness":provider["id"],"model":self.model,"reasoning":self.reasoning,"modelOptions":self.model_options,"cwd":self.cwd(),"sandbox":self.sandbox,"plan":chat["config"]["plan"] == true,"autoApprove":self.sandbox==zuse_protocol::SandboxLevel::DangerFullAccess,"resume":null,"attachments":[],"worktree":null}});
        self.call(
            methods::QUEUE_COMMAND,
            zuse_protocol::presentation::chat_request(&chat, json!({"command": command})),
        );
        true
    }
}
