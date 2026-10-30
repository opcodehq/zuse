//! Frozen cloud transcript: no engine or execution transport is installed in the child state.
pub(crate) mod saved;
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
    Connect(String),
    Resume {
        session: String,
        text: String,
        send: bool,
    },
}
pub struct CloudHistory {
    root: EngineHandle,
    owner: String,
    workspace: Workspace,
    view: Entity<Transcript>,
    composer: Entity<crate::composer::Composer>,
    _composer_events: gpui::Subscription,
    _state: Entity<AppState>,
    busy: bool,
    error: Option<String>,
    command: String,
    task: Option<Task<()>>,
    reading: bool,
    session_task: Option<Task<()>>,
}
impl EventEmitter<Event> for CloudHistory {}
impl CloudHistory {
    pub fn new(
        root: EngineHandle,
        workspace: Workspace,
        head: Head,
        owner: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let (state, view) = Self::frozen(&head, workspace.archived(), cx);
        let composer = cx.new(|cx| crate::composer::Composer::new(state.clone(), cx));
        composer.update(cx, |composer, cx| {
            composer.set_cached_cloud(workspace.state == "paused", cx)
        });
        let events = cx.subscribe(&composer, |this: &mut Self, _, event, cx| {
            if let crate::composer::ComposerEvent::ResumeCloud { text, send } = event {
                cx.emit(Event::Resume {
                    session: this.selected_session(cx),
                    text: text.clone(),
                    send: *send,
                });
            }
        });
        Self {
            root,
            owner,
            workspace,
            view,
            composer,
            _composer_events: events,
            _state: state,
            busy: false,
            error: None,
            command: uuid::Uuid::new_v4().to_string(),
            task: None,
            reading: false,
            session_task: None,
        }
    }
    fn frozen(
        head: &Head,
        archived: bool,
        cx: &mut Context<Self>,
    ) -> (Entity<AppState>, Entity<Transcript>) {
        let state = cx.new(|cx| {
            let mut state = AppState::new();
            state.connection = zuse_protocol::view::ConnectionStatus::Ready;
            state.chats = head.conversations.clone();
            if !state.chats.iter().any(|c| c.id == head.chat.id) {
                state.chats.push(head.chat.clone());
            }
            for chat in &mut state.chats {
                chat.archived = archived;
            }
            state.selected_chat = Some(head.session_id.clone());
            let _ = state.receive_transcript_update(head.transcript.clone(), cx);
            state
        });
        let view = cx.new(|cx| Transcript::new(state.clone(), cx));
        (state, view)
    }
    pub(crate) fn update_workspace(&mut self, workspace: Workspace, cx: &mut Context<Self>) {
        if workspace.workspace_id == self.workspace.workspace_id {
            self.composer.update(cx, |composer, cx| {
                composer.set_cached_cloud(workspace.state == "paused", cx)
            });
            self.workspace = workspace;
            cx.notify();
        }
    }
    pub(crate) fn selected_session(&self, cx: &gpui::App) -> String {
        self._state
            .read(cx)
            .selected_chat
            .clone()
            .unwrap_or_else(|| self.workspace.initial_session_id.clone())
    }
    fn select_session(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy || self.reading || self._state.read(cx).selected_chat.as_deref() == Some(&id) {
            return;
        }
        self.reading = true;
        self.error = None;
        let root = self.root.clone();
        let owner = self.owner.clone();
        let workspace = self.workspace.workspace_id.clone();
        self.session_task = Some(cx.spawn(async move |this, cx| {
            let result = root
                .client()
                .call_scoped(
                    zuse_protocol::methods::CLOUD_HISTORY,
                    serde_json::json!({"workspaceId":workspace,"sessionId":id,"expectedAccountId":owner,"expectedOrganizationId":null}),
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.reading = false;
                match result {
                    Ok(value) if value["accountId"].as_str()==Some(owner.as_str()) => match serde_json::from_value::<Head>(value["head"].clone()) {
                        Ok(head) if head.workspace_id == workspace && head.session_id == id => {
                            let archived = this.workspace.archived();
                            this._state.update(cx, |state, cx| {
                                state.chats = head.conversations.clone();
                                if !state.chats.iter().any(|chat|chat.id == head.chat.id) { state.chats.push(head.chat.clone()); }
                                for chat in &mut state.chats { chat.archived = archived; }
                                state.selected_chat = Some(head.session_id.clone());
                                let _ = state.receive_transcript_update(head.transcript.clone(), cx);
                                cx.notify();
                            });
                        }
                        _ => {
                            this.error =
                                Some("Saved conversation is unavailable for this workspace.".into())
                        }
                    },
                    Ok(_) => this.error = Some("Your account changed. Reopen this history.".into()),
                    Err(error) => this.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn unarchive(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.workspace.state != "archived" {
            return;
        }
        self.busy = true;
        self.error = None;
        let engine = self.root.clone();
        let owner = self.owner.clone();
        let id = self.workspace.workspace_id.clone();
        let command = self.command.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    zuse_engine::cloud::UNARCHIVE,
                    serde_json::json!({"workspaceId":id,"commandId":command,"expectedAccountId":owner,"expectedOrganizationId":null}),
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(value) if value["accountId"].as_str()==Some(owner.as_str()) && value["workspace"]["workspaceId"]==id => cx.emit(Event::Restored(id)),
                    Ok(_) => this.error=Some("This restore could not be verified. Refresh to check its state.".into()),
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
        zuse_viewport::palette::set_light(theme.appearance.is_light());
        let title = self
            ._state
            .read(cx)
            .selected_chat_row()
            .and_then(|c| c.title.clone())
            .unwrap_or_else(|| self.workspace.branch.clone());
        let content = div()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.text)
            .flex()
            .flex_col()
            .min_h_0()
            .child(
                div()
                    .h(px(40.))
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
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .role(gpui::Role::Button)
                            .aria_label("Back")
                            .tab_index(0)
                            .text_color(theme.text_muted)
                            .hover(|view| view.bg(theme.element_hover))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Back)))
                            .child(
                                icons::icon(icons::ALT_ARROW_LEFT)
                                    .size(px(16.))
                                    .text_color(theme.text_muted),
                            ),
                    )
                    .child(div().flex_1().text_size(px(13.)).child(title))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child("Cloud"),
                    ),
            )
            .child(zuse_viewport::cloud_archive_cards::sessions(
                &self._state.read(cx).chats,
                &self.selected_session(cx),
                self.busy || self.reading,
                |this, id, _, cx| this.select_session(id, cx),
                cx,
            ))
            .child(div().flex_1().min_h_0().child(if self.reading {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.text_muted)
                    .child("Loading saved conversation…")
                    .into_any_element()
            } else {
                self.view.clone().into_any_element()
            }));
        if !self.workspace.archived() {
            return content.child(self.composer.clone()).into_any_element();
        }
        content.child(
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
                                icons::icon(if self.workspace.archived() {
                                    icons::ARCHIVE_MINIMALISTIC
                                } else {
                                    icons::CLOUD
                                })
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
                                        if self.workspace.archived() {
                                            "This chat is archived."
                                        } else if self.workspace.status_code.starts_with("cloud_") {
                                            "Pause needs attention. Reopen this thread to check its state."
                                        } else if self.workspace.state == "pausing" {
                                            "Pausing cloud workspace…"
                                        } else if self.workspace.state == "paused" {
                                            "Cloud workspace paused."
                                        } else {
                                            "Saved conversation. Reconnect to continue."
                                        }
                                    })
                                    .when_some(self.error.clone(), |view, error| {
                                        view.child(div().text_size(px(11.)).child(error))
                                    }),
                            )
                            .child(
                                div()
                                    .id("cloud-unarchive-thread")
                                    .role(gpui::Role::Button)
                                    .aria_label(if self.workspace.archived() {
                                        "Unarchive cloud thread"
                                    } else {
                                        "Resume cloud workspace"
                                    })
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
                                    .opacity(
                                        if self.busy
                                            || (self.workspace.archived()
                                                && self.workspace.state != "archived")
                                        {
                                            0.45
                                        } else {
                                            1.
                                        },
                                    )
                                    .when(
                                        !self.busy
                                            && (!self.workspace.archived()
                                                || self.workspace.state == "archived"),
                                        |v| v.cursor_pointer(),
                                    )
                                    .child(if self.busy {
                                        "Restoring…"
                                    } else if self.workspace.archived() {
                                        "Restore chat"
                                    } else if self.workspace.state == "paused" {
                                        "Resume"
                                    } else {
                                        "Reconnect"
                                    })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if this.workspace.archived() {
                                            this.unarchive(cx)
                                        } else {
                                            cx.emit(Event::Connect(this.selected_session(cx)))
                                        }
                                    })),
                            ),
                    ),
            ).into_any_element()
    }
}
