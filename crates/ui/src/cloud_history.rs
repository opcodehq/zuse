//! Frozen cloud transcript: no engine or execution transport is installed in the child state.
use crate::{
    icons,
    state::{AppState, EngineHandle},
    theme::Theme,
    transcript::Transcript,
};
use gpui::{Context, Entity, EventEmitter, Render, Task, div, prelude::*, px};
use zuse_model::cloud::{Head, Workspace};

pub enum Event {
    Back,
    Restored(String),
}
pub struct CloudHistory {
    root: EngineHandle,
    workspace: Workspace,
    view: Entity<Transcript>,
    _state: Entity<AppState>,
    busy: bool,
    error: Option<String>,
    command: String,
    task: Option<Task<()>>,
}
impl EventEmitter<Event> for CloudHistory {}
impl CloudHistory {
    pub fn new(
        root: EngineHandle,
        workspace: Workspace,
        head: Head,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = cx.new(|cx| {
            let mut state = AppState::new();
            state.connection = zuse_protocol::view::ConnectionStatus::Ready;
            state.chats = head.conversations.clone();
            if !state.chats.iter().any(|c| c.id == head.chat.id) {
                state.chats.push(head.chat.clone());
            }
            for chat in &mut state.chats {
                chat.archived = true;
            }
            state.selected_chat = Some(head.session_id.clone());
            let _ = state.receive_transcript_update(head.transcript.clone(), cx);
            state
        });
        let view = cx.new(|cx| Transcript::new(state.clone(), cx));
        Self {
            root,
            workspace,
            view,
            _state: state,
            busy: false,
            error: None,
            command: uuid::Uuid::new_v4().to_string(),
            task: None,
        }
    }
    fn unarchive(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.workspace.state != "archived" {
            return;
        }
        self.busy = true;
        self.error = None;
        let engine = self.root.clone();
        let id = self.workspace.workspace_id.clone();
        let command = self.command.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    zuse_engine::cloud::UNARCHIVE,
                    serde_json::json!({"workspaceId":id,"commandId":command}),
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(_) => cx.emit(Event::Restored(id)),
                    Err(error) => this.error = Some(error.to_string()),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }
}
impl Render for CloudHistory {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let title = self
            ._state
            .read(cx)
            .selected_chat_row()
            .and_then(|c| c.title.clone())
            .unwrap_or_else(|| self.workspace.branch.clone());
        div()
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .h(px(48.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .id("cloud-history-back")
                            .cursor_pointer()
                            .role(gpui::Role::Button)
                            .aria_label("Back to archived sessions")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Back)))
                            .child(icons::icon(icons::ALT_ARROW_LEFT).size(px(16.))),
                    )
                    .child(div().flex_1().text_size(px(13.)).child(title))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child("Cloud"),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.view.clone()))
            .child(
                div()
                    .flex_none()
                    .border_t_1()
                    .border_color(theme.border.opacity(0.6))
                    .bg(theme.bg.opacity(0.92))
                    .px(px(16.))
                    .py(px(8.))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(896.))
                            .mx_auto()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                icons::icon(icons::ARCHIVE_MINIMALISTIC)
                                    .size(px(16.))
                                    .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.))
                                    .text_color(theme.text_muted)
                                    .child(if self.workspace.state == "archiving" {
                                        "Archiving this chat…"
                                    } else {
                                        "This chat is archived."
                                    })
                                    .when_some(self.error.clone(), |view, error| {
                                        view.child(div().text_size(px(11.)).child(error))
                                    }),
                            )
                            .child(
                                div()
                                    .id("cloud-unarchive-thread")
                                    .role(gpui::Role::Button)
                                    .aria_label("Unarchive cloud thread")
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
                                    .opacity(if self.busy || self.workspace.state != "archived" {
                                        0.45
                                    } else {
                                        1.
                                    })
                                    .when(!self.busy && self.workspace.state == "archived", |v| {
                                        v.cursor_pointer()
                                    })
                                    .child(if self.busy {
                                        "Unarchiving…"
                                    } else {
                                        "Unarchive"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| this.unarchive(cx))),
                            ),
                    ),
            )
    }
}
