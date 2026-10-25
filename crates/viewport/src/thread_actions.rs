//! Thread lifecycle controls keep the draft until the engine acknowledges a change.
use super::*;
use crate::{
    assets::icon,
    settings_style as s,
    shell::{BG, BORDER, MUTED, TEXT, column, row},
};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum DialogPage {
    Actions,
    Rename,
    Delete,
}
pub(super) struct ThreadDialog {
    chat: Value,
    page: DialogPage,
    error: Option<String>,
    buttons: [FocusHandle; 4],
}
#[derive(Clone)]
pub(super) enum ThreadChange {
    Rename(String),
    Archive(bool),
    Delete,
}
pub(super) struct PendingThreadAction {
    id: u64,
    chat: Value,
    change: ThreadChange,
}
impl WorkspaceView {
    pub(super) fn restoring_archived_chat(&self)->Option<&str> {
        self.thread_action.as_ref().filter(|action|matches!(&action.change,ThreadChange::Archive(false)))
            .and_then(|action|action.chat["id"].as_str())
    }

    pub(super) fn open_thread_actions(
        &mut self,
        chat: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.thread_action.is_some() {
            return;
        }
        let Some(current) = self
            .chats
            .iter()
            .find(|row| row["id"] == chat["id"] && row["deviceId"] == chat["deviceId"])
            .cloned()
        else {
            return;
        };
        let title = current["title"].as_str().unwrap_or("New thread").to_owned();
        self.thread_title
            .update(cx, |input, cx| input.set_value(title, cx));
        self.thread_dialog = Some(ThreadDialog {
            chat: current,
            page: DialogPage::Actions,
            error: None,
            buttons: std::array::from_fn(|_| cx.focus_handle()),
        });
        self.model_picker = false;
        self.access_picker = false;
        self.context_menu = false;
        self.add_menu = false;
        window.focus(&self.thread_dialog_focus, cx);
        cx.notify();
    }
    pub(super) fn close_thread_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.thread_action.is_some() {
            return;
        }
        self.thread_dialog = None;
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }
    fn thread_error(&mut self, error: &str, cx: &mut Context<Self>) {
        if let Some(dialog) = &mut self.thread_dialog {
            dialog.error = Some(error.into());
        } else if self.settings && self.settings_section == settings_view::Section::Archived {
            self.archive_error = Some(error.into());
        } else {
            self.error = Some(error.into());
        }
        cx.notify();
    }
    pub(super) fn change_thread(
        &mut self,
        chat: Value,
        change: ThreadChange,
        cx: &mut Context<Self>,
    ) {
        if self.thread_action.is_some() || !self.require_connection(cx) {
            return;
        }
        if !self.workspace_synced {
            self.thread_error(
                "Wait for the thread list to refresh before changing this thread.",
                cx,
            );
            return;
        }
        let Some(current) = self
            .chats
            .iter()
            .find(|row| row["id"] == chat["id"] && row["deviceId"] == chat["deviceId"])
            .cloned()
        else {
            self.thread_error(
                "This thread is no longer available. Refresh the thread list.",
                cx,
            );
            return;
        };
        if matches!(change, ThreadChange::Delete)
            && self.sessions.iter().any(|session| {
                session["chatId"] == current["id"]
                    && matches!(
                        session["status"].as_str(),
                        Some("working" | "awaitingInput")
                    )
            })
        {
            self.thread_error("Stop this thread’s agent before deleting it.", cx);
            return;
        }
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|row| row["id"] == current["id"]);
        if selected
            && !matches!(change, ThreadChange::Rename(_))
            && (self.file_navigation_blocked(cx)
                || self.queue_edit_navigation_blocked(cx)
                || self.model_navigation_blocked(cx)
                || self.project_action_navigation_blocked(cx))
        {
            if let Some(error) = self.error.clone() {
                self.thread_error(&error, cx);
            }
            return;
        }
        let mut params = match &change {
            ThreadChange::Rename(title) => {
                let title = title.trim();
                if title.is_empty() {
                    self.thread_error("Enter a thread name.", cx);
                    return;
                }
                json!({"op":"renameChat","chatId":current["id"],"title":title})
            }
            ThreadChange::Archive(archived) => {
                json!({"op":"setChatArchived","chatId":current["id"],"archived":archived})
            }
            ThreadChange::Delete => json!({"op":"deleteChat","chatId":current["id"]}),
        };
        params["targetDeviceId"] = current["deviceId"].clone();
        if let Some(dialog) = &mut self.thread_dialog {
            dialog.error = None;
        }
        self.archive_error = None;
        let id = self.call(methods::MUTATE, params);
        self.thread_action = Some(PendingThreadAction {
            id,
            chat: current,
            change,
        });
        cx.notify();
    }
    pub(super) fn commit_thread_rename(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = &self.thread_dialog else {
            return;
        };
        if dialog.page != DialogPage::Rename {
            return;
        }
        let chat = dialog.chat.clone();
        let title = self.thread_title.read(cx).value(cx).trim().to_owned();
        self.change_thread(chat, ThreadChange::Rename(title), cx);
    }
    pub(super) fn receive_thread_action(
        &mut self,
        id: u64,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self
            .thread_action
            .as_ref()
            .is_some_and(|action| action.id == id)
        {
            return false;
        }
        if frame.get("ok").is_none() && frame.get("err").is_none() && frame["done"] != true {
            return true;
        }
        let action = self.thread_action.take().unwrap();
        self.pending.remove(&id);
        if frame.get("ok").is_none() {
            let message = frame["err"].as_str().unwrap_or(
                "The change could not be confirmed. Refresh the thread list before trying again.",
            );
            self.thread_error(message, cx);
            return true;
        }
        if let Some(notice) = frame["ok"]["archive"]["notice"].as_str() {
            self.connection_notice = Some(notice.to_owned());
        }
        let selected = self
            .selected
            .as_ref()
            .is_some_and(|chat| chat["id"] == action.chat["id"]);
        match action.change {
            ThreadChange::Rename(title) => {
                let title = title.trim().to_owned();
                for chat in &mut self.chats {
                    if chat["id"] == action.chat["id"] {
                        chat["title"] = json!(title);
                    }
                }
                if selected {
                    self.selected.as_mut().unwrap()["title"] = json!(title);
                }
            }
            ThreadChange::Archive(archived) => {
                for chat in &mut self.chats {
                    if chat["id"] == action.chat["id"] {
                        chat["archived"] = json!(archived);
                    }
                }
                if selected {
                    self.selected.as_mut().unwrap()["archived"] = json!(archived);
                    self.rebind_history_review(cx);
                    // Keep the archived conversation open and its unsent draft intact.
                    self.show_archive = archived;
                    // Archive mode changes editability even when the file's
                    // content hash is unchanged. Keep any edits made while the
                    // action was pending, then reconcile clean previews.
                    let dirty = self.file_dirty(cx);
                    let path = self
                        .open_file
                        .as_ref()
                        .and_then(|file| file["path"].as_str())
                        .map(str::to_owned);
                    if let Some(file) = self.open_file.as_mut() {
                        if archived && file["readOnlyReason"].is_null() {
                            file["readOnlyReason"] = json!("archived");
                        }
                        if !archived && file["readOnlyReason"] == "archived" {
                            if let Some(object) = file.as_object_mut() {
                                object.remove("readOnlyReason");
                            }
                        }
                        let readonly =
                            file["readOnlyReason"].is_string() || file["truncated"] == true;
                        self.file_editor
                            .update(cx, |input, cx| input.set_readonly(readonly, cx));
                    }
                    if !dirty {
                        if let Some(path) = path {
                            self.read_file(&path, true, cx);
                        }
                    }
                    self.load_directory(&self.directory.clone());
                }
            }
            ThreadChange::Delete => {
                self.chats.retain(|chat| chat["id"] != action.chat["id"]);
                if selected {
                    self.new_chat(cx);
                }
                if let Some(key) = action.chat["id"].as_str() {
                    self.drafts.remove(key);
                }
            }
        }
        self.thread_dialog = None;
        self.input.update(cx, |input, cx| input.focus(cx));
        self.persist_drafts(cx);
        cx.notify();
        true
    }
    pub(super) fn interrupt_thread_action(&mut self) {
        if self.thread_action.take().is_some() {
            let message = "Connection interrupted. The change may have completed. Check the thread list after reconnecting before trying again.";
            if let Some(dialog) = &mut self.thread_dialog {
                dialog.error = Some(message.into());
            } else if self.settings && self.settings_section == settings_view::Section::Archived {
                self.archive_error = Some(message.into());
            } else {
                self.connection_notice = Some(message.into());
            }
        }
    }
    fn thread_dialog_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape" {
            self.close_thread_dialog(window, cx);
        } else if event.keystroke.key == "tab" {
            if let Some(dialog) = &self.thread_dialog {
                let mut targets = if dialog.page == DialogPage::Rename {
                    vec![self.thread_title.focus_handle(cx)]
                } else {
                    vec![]
                };
                let buttons = if dialog.page == DialogPage::Actions {
                    4
                } else {
                    2
                };
                targets.extend(dialog.buttons[..buttons].iter().cloned());
                let at = targets.iter().position(|handle| handle.is_focused(window));
                let next = match (at, event.keystroke.modifiers.shift) {
                    (Some(at), true) => (at + targets.len() - 1) % targets.len(),
                    (Some(at), false) => (at + 1) % targets.len(),
                    (None, true) => targets.len() - 1,
                    (None, false) => 0,
                };
                window.focus(&targets[next], cx);
            }
        }
        cx.stop_propagation();
    }
    pub(super) fn archived_thread_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let pending = self.thread_action.is_some();
        column()
            .w_full()
            .flex_none()
            .border_t_1()
            .border_color(rgb(BORDER).opacity(0.6))
            .bg(rgb(BG).opacity(0.92))
            .px_4()
            .py_2()
            .child(
                row()
                    .w_full()
                    .max_w(px(896.))
                    .mx_auto()
                    .gap(px(10.))
                    .child(icon("archive-minimalistic").size(px(16.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child("This chat is archived."),
                    )
                    .child(
                        s::button(
                            "unarchive-thread",
                            if pending {
                                "Unarchiving…"
                            } else {
                                "Unarchive"
                            },
                        )
                        .opacity(if self.connected && !pending { 1. } else { 0.45 })
                        .on_click(cx.listener(|this, _, _, cx| this.archive(cx))),
                    ),
            )
            .into_any_element()
    }
    pub(super) fn thread_dialog_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let dialog = self.thread_dialog.as_ref().unwrap();
        let chat = self.chats.iter().find(|row| {
            row["id"] == dialog.chat["id"] && row["deviceId"] == dialog.chat["deviceId"]
        });
        let available = self.connected
            && self.workspace_synced
            && chat.is_some()
            && self.thread_action.is_none();
        let archived = chat.unwrap_or(&dialog.chat)["archived"]
            .as_bool()
            .unwrap_or(false);
        let title = dialog.chat["title"]
            .as_str()
            .unwrap_or("New thread")
            .to_owned();
        let busy = self.thread_action.is_some();
        let heading = match dialog.page {
            DialogPage::Actions => "Thread actions",
            DialogPage::Rename => "Rename thread",
            DialogPage::Delete => "Delete thread?",
        };
        let button = |index, name: &'static str| {
            s::button(("thread-dialog-button", index), name)
                .track_focus(&dialog.buttons[index])
                .tab_index(0)
                .opacity(if available { 1. } else { 0.45 })
        };
        let chat_value = dialog.chat.clone();
        let contents = match dialog.page {
            DialogPage::Actions => column().gap_2()
                .child(button(0, "Rename").justify_start().child(icon("pen"))
                    .on_click(cx.listener(move |this, _, window, cx| { if !available { return; } let Some(dialog) = &mut this.thread_dialog else { return; }; dialog.page = DialogPage::Rename; window.focus(&this.thread_title.focus_handle(cx), cx); cx.notify(); })))
                .child(button(1, if archived { "Restore thread" } else { "Archive thread" }).justify_start().child(icon("archive-minimalistic"))
                    .on_click(cx.listener(move |this, _, _, cx| { if available { this.change_thread(chat_value.clone(), ThreadChange::Archive(!archived), cx); } })))
                .child(button(2, "Delete thread…").justify_start().text_color(rgb(0xec9292)).child(icon("trash-bin-minimalistic"))
                    .on_click(cx.listener(move |this, _, window, cx| { if !available { return; } let Some(dialog) = &mut this.thread_dialog else { return; }; dialog.page = DialogPage::Delete; window.focus(&dialog.buttons[0], cx); cx.notify(); })))
                .child(button(3, "Close").opacity(if busy { 0.45 } else { 1. })
                    .on_click(cx.listener(|this, _, window, cx| this.close_thread_dialog(window, cx)))).into_any_element(),
            DialogPage::Rename => column().gap_3()
                .child(s::field().h(px(36.)).child(self.thread_title.clone()))
                .child(row().justify_end().gap_2()
                    .child(button(0, "Cancel").opacity(if busy { 0.45 } else { 1. }).on_click(cx.listener(|this, _, window, cx| this.close_thread_dialog(window, cx))))
                    .child(button(1, if busy { "Saving…" } else { "Save name" }).on_click(cx.listener(move |this, _, _, cx| { if available { this.commit_thread_rename(cx); } })))).into_any_element(),
            DialogPage::Delete => column().gap_3()
                .child(s::description_text("Remove this thread from the workspace? This cannot be undone here. Project files are kept."))
                .child(row().justify_end().gap_2()
                    .child(button(0, "Cancel").opacity(if busy { 0.45 } else { 1. }).on_click(cx.listener(|this, _, window, cx| this.close_thread_dialog(window, cx))))
                    .child(button(1, if busy { "Deleting…" } else { "Delete thread" }).text_color(rgb(0xec9292))
                        .on_click(cx.listener(move |this, _, _, cx| { if available { this.change_thread(chat_value.clone(), ThreadChange::Delete, cx); } })))).into_any_element(),
        };
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .bg(black().opacity(0.55))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(
                column()
                    .id("thread-dialog")
                    .role(Role::Dialog)
                    .aria_label(heading)
                    .track_focus(&self.thread_dialog_focus)
                    .on_key_down(cx.listener(Self::thread_dialog_key))
                    .w(px(400.))
                    .max_w_full()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(BORDER))
                    .bg(rgb(BG))
                    .p_4()
                    .gap_3()
                    .shadow_lg()
                    .child(
                        div()
                            .text_size(px(16.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(TEXT))
                            .child(heading),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(title),
                    )
                    .child(contents)
                    .when(!self.connected, |view| {
                        view.child(s::description_text("Reconnect to manage this thread."))
                    })
                    .when(self.connected && chat.is_none(), |view| {
                        view.child(s::description_text(
                            "This thread is no longer in the workspace.",
                        ))
                    })
                    .when_some(dialog.error.clone(), |view, error| {
                        view.child(s::description_text(error).text_color(rgb(0xec9292)))
                    }),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    #[gpui::test]
    fn parity_thread_archive_restore_and_rename_wait_for_engine_receipts(cx: &mut TestAppContext) {
        cx.update(initialize);
        let (requests, mut sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
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
        window.update(cx, |view, window, cx| {
            view.connected = true;
            view.workspace_synced = true;
            let chat = json!({"id":"thread","deviceId":"host","title":"Original","archived":false,"config":{"harness":"codex"}});
            view.chats = vec![chat.clone()];
            view.selected = Some(chat.clone());
            view.open_thread_actions(chat.clone(), window, cx);
            view.change_thread(chat.clone(), ThreadChange::Archive(true), cx);
            let archive = view.thread_action.as_ref().unwrap().id;
            view.change_thread(chat.clone(), ThreadChange::Delete, cx);
            assert_eq!(view.thread_action.as_ref().unwrap().id, archive);
            assert_eq!(view.selected.as_ref().unwrap()["archived"], false);
            view.receive(json!({"id":archive,"ok":null}), cx);
            assert_eq!(view.selected.as_ref().unwrap()["archived"], true);
            assert!(view.show_archive);
            assert!(view.thread_dialog.is_none());
            view.archive(cx);
            let restore = view.thread_action.as_ref().unwrap().id;
            view.receive(json!({"id":restore,"ok":null}), cx);
            assert_eq!(view.selected.as_ref().unwrap()["archived"], false);
            assert!(!view.show_archive);
            view.change_thread(chat, ThreadChange::Rename("New title".into()), cx);
            let rename = view.thread_action.as_ref().unwrap().id;
            assert_eq!(view.selected.as_ref().unwrap()["title"], "Original");
            view.receive(json!({"id":rename,"ok":null}), cx);
            assert_eq!(view.selected.as_ref().unwrap()["title"], "New title");
        }).unwrap();
        let mut writes = vec![];
        while let Ok(request) = sent.try_recv() {
            if request["method"] == methods::MUTATE {
                writes.push(request["params"].clone());
            }
        }
        assert_eq!(writes.len(), 3);
        assert_eq!(writes[1]["archived"], false);
    }
    #[gpui::test]
    fn parity_thread_deletion_blocks_running_agents_and_never_replays_after_disconnect(
        cx: &mut TestAppContext,
    ) {
        cx.update(initialize);
        let (requests, mut sent) = futures::channel::mpsc::unbounded();
        let (_events, frames) = futures::channel::mpsc::unbounded();
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
        window.update(cx, |view, window, cx| {
            view.connected = true;
            view.workspace_synced = true;
            let chat = json!({"id":"thread","deviceId":"host","title":"Keep","archived":false});
            view.chats = vec![chat.clone()];
            view.open_thread_actions(chat.clone(), window, cx);
            view.sessions = vec![json!({"chatId":"thread","status":"working"})];
            view.change_thread(chat.clone(), ThreadChange::Delete, cx);
            assert!(view.thread_action.is_none());
            assert!(view.thread_dialog.as_ref().unwrap().error.as_ref().unwrap().contains("Stop"));
            view.sessions.clear();
            view.change_thread(chat.clone(), ThreadChange::Delete, cx);
            let delete = view.thread_action.as_ref().unwrap().id;
            view.receive(json!({"connection":"disconnected","interrupted":[{"id":delete,"method":methods::MUTATE,"mutation":true}]}), cx);
            assert!(view.thread_action.is_none());
            assert_eq!(view.chats, vec![chat]);
            view.receive(json!({"id":delete,"ok":null}), cx);
            assert_eq!(view.chats.len(), 1);
            assert!(view.thread_dialog.as_ref().unwrap().error.as_ref().unwrap().contains("may have completed"));
        }).unwrap();
        let mut writes = 0;
        while let Ok(request) = sent.try_recv() {
            if request["method"] == methods::MUTATE {
                writes += 1;
            }
        }
        assert_eq!(writes, 1);
    }
}
