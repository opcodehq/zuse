//! Archived threads keep their transcript and draft, with an explicit restore action.
use super::Composer;
use crate::localization::message as tr;
use crate::{icons, theme::Theme};
use gpui::{Context, Div, Role, div, prelude::*, px};
use zuse_i18n::messages as msg;
use zuse_protocol::Chat;
use zuse_rpc::methods;

impl Composer {
    fn unarchive_thread(&mut self, chat: Chat, cx: &mut Context<Self>) {
        if self.unarchiving.is_some() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if !self.state.read(cx).chats.iter().any(|current| {
            current.id == chat.id && current.device_id == chat.device_id && current.archived
        }) {
            return;
        }
        self.state
            .update(cx, |state, _| state.clear_chat_archive_intent(&chat.id));
        let id = chat.id;
        let params = serde_json::json!({"op":"setChatArchived","chatId":id,"archived":false,"targetDeviceId":chat.device_id});
        self.unarchiving = Some(id.clone());
        self.unarchive_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::MUTATE, params).await;
            let _ = this.update(cx, |this, cx| {
                this.unarchiving = None;
                match result {
                    Err(error) => {
                        this.failure = Some(format!("Unarchive failed: {error}").into());
                        this.failure_key = Some(id);
                    }
                    Ok(_) if this.failure_key.as_deref() == Some(id.as_str()) => {
                        this.failure = None;
                        this.failure_key = None;
                    }
                    Ok(_) => {}
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    pub(super) fn render_archived_footer(&self, chat: &Chat, cx: &mut Context<Self>) -> Div {
        let theme = Theme::of(cx).clone();
        let busy = self.unarchiving.is_some();
        let connected = self.state.read(cx).engine().is_some();
        let target = chat.clone();
        let button = div()
            .id("unarchive-thread")
            .role(Role::Button)
            .aria_label(tr(cx, msg::native::unarchive_thread))
            .tab_index(0)
            .h(px(28.))
            .px(px(12.))
            .rounded(px(7.))
            .border_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .text_size(px(11.))
            .text_color(theme.text)
            .opacity(if busy || !connected { 0.45 } else { 1. })
            .when(!busy && connected, |view| view.cursor_pointer())
            .child(if busy {
                crate::localization::text("chat-archived_chats_page_unarchiving", cx)
            } else {
                crate::localization::text("chat-archived_chats_page_unarchive", cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| this.unarchive_thread(target.clone(), cx)));
        let row = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .w_full()
            .max_w(px(896.))
            .mx_auto()
            .child(
                icons::icon(icons::ARCHIVE_MINIMALISTIC)
                    .size(px(16.))
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.text_muted)
                            .child(tr(cx, msg::chat::archived_chats_page_this_chat_is_archived)),
                    )
                    .when_some(
                        self.failure
                            .clone()
                            .filter(|_| self.failure_key.as_deref() == Some(chat.id.as_str())),
                        |view, error| {
                            view.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.text_muted)
                                    .child(error),
                            )
                        },
                    ),
            )
            .child(button);
        div()
            .w_full()
            .border_t_1()
            .border_color(theme.border.opacity(0.6))
            .bg(theme.bg.opacity(0.92))
            .px(px(16.))
            .py(px(8.))
            .child(row)
    }
}
