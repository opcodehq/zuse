//! Frozen cloud transcript: no engine or execution transport is installed in the child state.
use crate::localization::message as tr;
use zuse_i18n::messages as msg;
pub(crate) mod saved;
mod shared;
use crate::{
    icons,
    state::{AppState, EngineHandle},
    theme::Theme,
    transcript::Transcript,
};
use gpui::{Context, Entity, EventEmitter, Render, Task, div, prelude::*, px};
pub(crate) use shared::{open_project, open_setup};
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
    organization: Option<String>,
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
    page_head: Head,
    page_rows: Vec<zuse_model::SessionMessageEntry>,
    older_loading: bool,
    older_task: Option<Task<()>>,
    admissions: Vec<serde_json::Value>,
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
            organization: None,
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
            page_rows: match &head.transcript.frame {
                zuse_model::TranscriptFrame::Reset { reset } => reset.as_ref().clone(),
                _ => vec![],
            },
            page_head: head,
            older_loading: false,
            older_task: None,
            admissions: vec![],
        }
    }
    pub(crate) fn with_scope(
        mut self,
        organization: Option<String>,
        metadata: serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Self {
        self.organization = organization;
        self.admissions = metadata["commandAdmissions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        self.project_rows(cx);
        self
    }
    // Only presentation changes: page_head/cursor and encrypted cache certificates
    // stay canonical. This view never installs an execution engine.
    pub(crate) fn admission_rows(
        head: &Head,
        admissions: &[serde_json::Value],
        mut rows: Vec<zuse_model::SessionMessageEntry>,
    ) -> Vec<zuse_model::SessionMessageEntry> {
        use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

        let mut seen: std::collections::HashSet<_> = rows.iter().map(|m| m.id.clone()).collect();
        for entry in admissions.iter().take(256) {
            let Some(id) = entry["messageId"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 512)
            else {
                continue;
            };
            if !seen.insert(id.to_owned()) {
                continue;
            }
            let Some(text) = entry["text"].as_str() else {
                continue;
            };
            let Ok(assets) = serde_json::from_value::<Vec<zuse_protocol::cloud_assets::Asset>>(
                entry
                    .get("attachments")
                    .cloned()
                    .unwrap_or(serde_json::json!([])),
            ) else {
                continue;
            };
            let refs: Vec<_> = assets
                .iter()
                .filter_map(|asset| {
                    serde_json::to_vec(asset)
                        .ok()
                        .filter(|bytes| bytes.len() <= 3072)
                        .map(|bytes| {
                            format!(
                                "cloud-asset://{}/{}",
                                URL_SAFE_NO_PAD.encode(bytes),
                                asset.original_name.replace(['/', '\\'], "_")
                            )
                        })
                })
                .collect();
            if refs.len() != assets.len() {
                continue;
            }
            rows.push(zuse_model::SessionMessageEntry {
                id: id.into(),
                role: zuse_model::MessageRole::User,
                parts: vec![zuse_model::MessagePart::Text {
                    id: format!("admission:{id}"),
                    text: zuse_protocol::message_attachments::with_attachments(text, &refs),
                }],
                created_at: entry["createdAt"].as_i64().unwrap_or_default(),
                device_id: head.chat.device_id.clone(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            });
        }
        rows
    }
    fn project_rows(&self, cx: &mut Context<Self>) {
        let rows = Self::admission_rows(&self.page_head, &self.admissions, self.page_rows.clone());
        self._state.update(cx, |state, cx| {
            state.apply_transcript(rows);
            cx.notify();
        });
    }
    fn frozen(
        head: &Head,
        archived: bool,
        cx: &mut gpui::App,
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
        if self.busy
            || self.reading
            || self.older_loading
            || self._state.read(cx).selected_chat.as_deref() == Some(&id)
        {
            return;
        }
        self.reading = true;
        self.error = None;
        let root = self.root.clone();
        let owner = self.owner.clone();
        let organization = self.organization.clone();
        let workspace = self.workspace.workspace_id.clone();
        self.session_task = Some(cx.spawn(async move |this, cx| {
            let result = root
                .client()
                .call_scoped(
                    zuse_protocol::methods::CLOUD_HISTORY,
                    serde_json::json!({"workspaceId":workspace,"sessionId":id,"incremental":true,"expectedAccountId":owner,"expectedOrganizationId":organization}),
                )
                .await;
            let _ = this.update(cx, |this, cx| {
                this.reading = false;
                match result {
                    Ok(value) if value["accountId"].as_str()==Some(owner.as_str())&&value["organizationId"].as_str()==organization.as_deref() => match serde_json::from_value::<Head>(value["head"].clone()) {
                        Ok(head) if head.workspace_id == workspace && head.session_id == id => {
                            this.page_rows=match &head.transcript.frame {zuse_model::TranscriptFrame::Reset{reset}=>reset.as_ref().clone(),_=>vec![]};
                            this.page_head=head.clone();
                            this.admissions=value["commandAdmissions"].as_array().cloned().unwrap_or_default();
                            let archived = this.workspace.archived();
                            this._state.update(cx, |state, cx| {
                                state.chats = head.conversations.clone();
                                if !state.chats.iter().any(|chat|chat.id == head.chat.id) { state.chats.push(head.chat.clone()); }
                                for chat in &mut state.chats { chat.archived = archived; }
                                state.selected_chat = Some(head.session_id.clone());
                                let _ = state.receive_transcript_update(head.transcript.clone(), cx);
                                cx.notify();
                            });
                            this.project_rows(cx);
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
    fn load_older(&mut self, cx: &mut Context<Self>) {
        let Some(before) = self.page_head.older_message_sequence else {
            return;
        };
        if self.older_loading || self.reading || self.busy {
            return;
        }
        self.older_loading = true;
        self.error = None;
        let root = self.root.clone();
        let owner = self.owner.clone();
        let organization = self.organization.clone();
        let expected = self.page_head.clone();
        self.older_task=Some(cx.spawn(async move |this,cx| {
            let result=root.client().call_scoped(zuse_protocol::methods::CLOUD_HISTORY,serde_json::json!({"workspaceId":expected.workspace_id,"sessionId":expected.session_id,"beforeSequence":before,"cursor":expected.cursor,"incremental":true,"expectedAccountId":owner,"expectedOrganizationId":organization})).await;
            let _=this.update(cx,|this,cx| {
                this.older_loading=false;
                let page=result.ok().filter(|value|value["accountId"].as_str()==Some(owner.as_str())&&value["organizationId"].as_str()==organization.as_deref()).and_then(|value|serde_json::from_value::<Head>(value["head"].clone()).ok());
                match page {
                    Some(page) if page.workspace_id==expected.workspace_id&&page.session_id==expected.session_id&&page.runtime_generation==expected.runtime_generation&&page.cursor.epoch==expected.cursor.epoch&&page.older_message_sequence.is_none_or(|next|next<before)=> {
                        if let zuse_model::TranscriptFrame::Reset{reset}=&page.transcript.frame {
                            let seen:std::collections::HashSet<_>=this.page_rows.iter().map(|m|m.id.clone()).collect();
                            let mut rows:Vec<_>=reset.iter().filter(|m|!seen.contains(&m.id)).cloned().collect();
                            rows.append(&mut this.page_rows);this.page_rows=rows;
                            this.page_head.older_message_sequence=page.older_message_sequence;
                            this.project_rows(cx);
                        }else{this.error=Some("Earlier history returned an invalid page.".into());}
                    },
                    _=>this.error=Some("Earlier history changed or is unavailable. Refresh this conversation to retry.".into()),
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
        let organization = self.organization.clone();
        let id = self.workspace.workspace_id.clone();
        let command = self.command.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    zuse_engine::cloud::UNARCHIVE,
                    serde_json::json!({"workspaceId":id,"commandId":command,"expectedAccountId":owner,"expectedOrganizationId":organization}),
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
                            .aria_label(tr(cx, msg::chat::checkpoints_back))
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
                            .child(tr(cx, msg::chat::computer_picker_cloud_sandbox)),
                    ),
            )
            .child(zuse_viewport::cloud_archive_cards::sessions(
                &self._state.read(cx).chats,
                &self.selected_session(cx),
                self.busy || self.reading,
                |this, id, _, cx| this.select_session(id, cx),
                cx,
            ))
            .when(self.page_head.older_message_sequence.is_some(), |v| {
                v.child(
                    div()
                        .id("cloud-history-load-earlier")
                        .flex_none()
                        .px(px(16.))
                        .py(px(6.))
                        .text_size(px(12.))
                        .cursor_pointer()
                        .child(if self.older_loading {
                            "Loading earlier messages…"
                        } else {
                            "Load earlier messages"
                        })
                        .on_click(cx.listener(|this, _, _, cx| this.load_older(cx))),
                )
            })
            .when_some(self.error.clone(), |v, error| {
                v.child(
                    div()
                        .flex_none()
                        .px(px(16.))
                        .py(px(4.))
                        .text_size(px(12.))
                        .child(error),
                )
            })
            .child(div().flex_1().min_h_0().child(if self.reading {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.text_muted)
                    .child(tr(cx, msg::native::loading_saved_conversation))
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
                                        ("Restoring…").to_string()
                                    } else if self.workspace.archived() {
                                        ("Restore chat").to_string()
                                    } else if self.workspace.state == "paused" {
                                        crate::localization::text("chat-queue_tray_resume", cx)
                                    } else {
                                        crate::localization::text("simulators-reconnect", cx)
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

#[cfg(feature = "appshots-fixture")]
pub(crate) use shared::fixture;

#[cfg(feature = "appshots-fixture")]
pub(crate) use saved::fixture as saved_fixture;
