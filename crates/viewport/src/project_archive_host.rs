//! Browser archive reads stay separate from the active conversation transport.
use super::*;
use crate::project_archive as ui;
#[derive(Default)]
pub(super) struct ArchiveRoute {
    pub project: Option<String>,
    selected: Option<String>,
    session: Option<String>,
    search: Option<Entity<input::TextInput>>,
    read: Option<u64>,
    action: Option<(u64, Value, String, Option<String>)>,
    view: Option<Entity<WorkspaceView>>,
    loading: bool,
    read_error: Option<String>,
    restore_error: Option<String>,
    restore_target: Option<String>,
    pub menu: Option<(String, Point<Pixels>)>,
    menu_focus: Option<FocusHandle>,
}
impl WorkspaceView {
    pub(super) fn close_project_archives(&mut self) {
        if let Some(id) = self.project_archive.read.take() {
            self.cancel(id);
        }
        self.project_archive.project = None;
        self.project_archive.selected = None;
        self.project_archive.session = None;
        self.project_archive.view = None;
        self.project_archive.menu = None;
    }
    pub(super) fn project_archive_menu(
        &mut self,
        id: String,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.spaces.iter().any(|s| s.id == id) {
            return;
        }
        let focus = self
            .project_archive
            .menu_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        self.project_archive.menu = Some((id, position));
        window.focus(&focus, cx);
        cx.notify();
    }
    pub(super) fn open_project_archives(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.spaces.iter().any(|s| s.id == id) {
            return;
        }
        self.close_plugins_browse();
        self.close_project_archives();
        if self.project_archive.search.is_none() {
            let search = cx.new(|cx| input::TextInput::new("Filter archived chats…", window, cx));
            cx.observe(&search, |_, _, cx| cx.notify()).detach();
            self.project_archive.search = Some(search);
        }
        self.project_archive
            .search
            .as_ref()
            .unwrap()
            .update(cx, |input, cx| input.set_value(String::new(), cx));
        self.project_archive.project = Some(id);
        self.project_archive.restore_error = None;
        self.settings = false;
        self.project_picker = false;
        self.model_picker = false;
        self.access_picker = false;
        self.agent_picker = false;
        self.add_menu = false;
        self.context_menu = false;
        window.focus(&self.workspace_focus, cx);
        cx.notify();
    }
    fn archive_sessions(&self) -> Vec<zuse_protocol::Chat> {
        let Some(thread) = self.project_archive.selected.as_deref() else {
            return vec![];
        };
        let mut rows: Vec<zuse_protocol::Chat> = self
            .chats
            .iter()
            .filter_map(|c| serde_json::from_value(c.clone()).ok())
            .filter(|c: &zuse_protocol::Chat| {
                c.thread_id() == thread
                    && c.space_id.as_deref() == self.project_archive.project.as_deref()
            })
            .collect();
        rows.sort_by_key(|c| c.created_at);
        rows
    }
    fn read_archive_session(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.archive_sessions().into_iter().find(|c| c.id == id) else {
            return;
        };
        if let Some(request) = self.project_archive.read.take() {
            self.cancel(request);
        }
        self.project_archive.loading = true;
        self.project_archive.read_error = None;
        self.project_archive.session = Some(id.clone());
        let value = serde_json::to_value(chat).unwrap();
        let (requests, _discard) = futures::channel::mpsc::unbounded();
        let (_events, events) = futures::channel::mpsc::unbounded();
        let view = cx.new(|cx| {
            let mut child = WorkspaceView::new(Transport { requests, events }, window, cx);
            child.cached_only = true;
            child.selected = Some(value.clone());
            child.messages.clear();
            child.error = None;
            child
        });
        self.project_archive.view = Some(view);
        if !self.connected {
            self.project_archive.loading = false;
            self.project_archive.read_error =
                Some("Computer unavailable. Reconnect to load this transcript.".into());
        } else {
            self.project_archive.read = Some(self.call(
                methods::WATCH_DOC_MESSAGES,
                zuse_protocol::presentation::chat_request(&value, json!({})),
            ));
        }
        cx.notify();
    }
    fn archive_action(&mut self, action: ui::Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            ui::Action::Open(id) => {
                if self.chats.iter().any(|c| {
                    c["id"] == id
                        && c["archived"] == true
                        && c["spaceId"].as_str() == self.project_archive.project.as_deref()
                }) {
                    self.project_archive.selected = Some(id.clone());
                    self.project_archive.restore_error = None;
                    let first = self
                        .archive_sessions()
                        .first()
                        .map(|c| c.id.clone())
                        .unwrap_or(id);
                    self.read_archive_session(first, window, cx);
                }
            }
            ui::Action::Session(id) => self.read_archive_session(id, window, cx),
            ui::Action::Back => {
                if let Some(id) = self.project_archive.read.take() {
                    self.cancel(id);
                }
                self.project_archive.selected = None;
                self.project_archive.session = None;
                self.project_archive.view = None;
                self.project_archive.read_error = None;
            }
            ui::Action::Retry => {
                if let Some(id) = self.project_archive.session.clone() {
                    self.read_archive_session(id, window, cx)
                }
            }
            ui::Action::Restore(id) => {
                if self.project_archive.action.is_some() {
                    return;
                }
                let Some(chat) = self
                    .chats
                    .iter()
                    .find(|c| {
                        c["id"] == id
                            && c["archived"] == true
                            && c["threadId"].is_null()
                            && c["spaceId"].as_str() == self.project_archive.project.as_deref()
                    })
                    .cloned()
                else {
                    return;
                };
                self.project_archive.restore_target = Some(id.clone());
                if !self.connected {
                    self.project_archive.restore_error =
                        Some("Computer unavailable. Reconnect to restore this chat.".into());
                    cx.notify();
                    return;
                }
                let project = self.project_archive.project.clone().unwrap();
                self.project_archive.restore_error = None;
                let request = self.call(
                    methods::MUTATE,
                    zuse_protocol::presentation::chat_request(
                        &chat,
                        json!({"op":"setChatArchived","archived":false}),
                    ),
                );
                self.project_archive.action = Some((
                    request,
                    chat,
                    project,
                    self.project_archive.selected.clone(),
                ));
            }
        }
        // Transcript view construction creates its own focus handles.
        // Keep keyboard navigation on the visible workspace after installing it.
        window.focus(&self.workspace_focus, cx);
        cx.notify();
    }
    pub(super) fn receive_project_archives(
        &mut self,
        frame: &Value,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(id) = frame["id"].as_u64() else {
            return false;
        };
        if self.project_archive.read == Some(id) {
            if let Some(value) = frame.get("item").or_else(|| frame.get("ok")) {
                if let (Some(view), Ok(update)) = (
                    self.project_archive.view.clone(),
                    serde_json::from_value::<TranscriptUpdate>(value.clone()),
                ) {
                    let result =
                        view.update(cx, |view, cx| view.apply_archive_transcript(update, cx));
                    self.project_archive.loading = false;
                    if let Err(error) = result {
                        self.project_archive.read_error = Some(error.to_string());
                    }
                } else {
                    self.project_archive.loading = false;
                    self.project_archive.read_error =
                        Some("Could not read saved transcript. Retry to load it again.".into());
                }
            }
            if frame["err"].is_string() || frame["done"] == true {
                self.project_archive.loading = false;
                if let Some(error) = frame["err"].as_str() {
                    self.project_archive.read_error = Some(error.to_owned());
                }
                self.project_archive.read = None;
                self.pending.remove(&id);
            }
            cx.notify();
            return true;
        }
        if let Some((request, _, _, _)) = &self.project_archive.action {
            if *request == id {
                let (_, mut chat, project, selection) = self.project_archive.action.take().unwrap();
                self.pending.remove(&id);
                if frame.get("ok").is_some() {
                    chat["archived"] = json!(false);
                    for current in &mut self.chats {
                        if current["id"] == chat["id"] || current["threadId"] == chat["id"] {
                            current["archived"] = json!(false);
                        }
                    }
                    if self.project_archive.project.as_ref() == Some(&project)
                        && self.project_archive.selected == selection
                        && !self.settings
                        && !self.plugins_browse
                    {
                        self.select(chat, cx);
                    }
                } else {
                    self.project_archive.restore_error = Some(
                        frame["err"]
                            .as_str()
                            .unwrap_or("Unarchive could not finish. Try again.")
                            .to_owned(),
                    );
                }
                cx.notify();
                return true;
            }
        }
        false
    }
    pub(super) fn project_archives_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(project) = self.project_archive.project.clone() else {
            return ui::centered("Select a project to view archived chats.").into_any_element();
        };
        let chats: Vec<zuse_protocol::Chat> = self
            .chats
            .iter()
            .filter_map(|c| serde_json::from_value(c.clone()).ok())
            .collect();
        if let Some(thread) = self
            .project_archive
            .selected
            .as_ref()
            .and_then(|id| {
                chats
                    .iter()
                    .find(|c| &c.id == id && c.archived && c.space_id.as_deref() == Some(&project))
            })
            .cloned()
            .map(ui::Thread::from_chat)
        {
            let body = if let Some(error) = self.project_archive.read_error.clone() {
                ui::failure(error, cx, Self::archive_action)
            } else if let Some(view) = self.project_archive.view.clone() {
                if !self.project_archive.loading && view.read(cx).messages.is_empty() {
                    ui::centered("No messages in this session.").into_any_element()
                } else {
                    view.update(cx, |view, cx| view.transcript(cx))
                }
            } else {
                ui::centered("Loading transcript…").into_any_element()
            };
            return ui::preview(
                &thread,
                &self.archive_sessions(),
                self.project_archive.session.as_deref().unwrap_or_default(),
                body,
                self.project_archive.loading,
                self.project_archive.restore_error.as_deref().filter(|_| {
                    self.project_archive.restore_target.as_deref() == Some(thread.chat.id.as_str())
                }),
                self.project_archive.action.is_some(),
                cx,
                Self::archive_action,
            );
        }
        let Some(space) = self.spaces.iter().find(|s| s.id == project) else {
            return ui::centered("Select a project to view archived chats.").into_any_element();
        };
        let search = self.project_archive.search.as_ref().unwrap();
        let query = search.read(cx).value(cx);
        ui::list(
            space.display_name(),
            &query,
            search.clone().into_any_element(),
            &ui::rows(&chats, &project, &query),
            self.project_archive
                .action
                .as_ref()
                .and_then(|(_, chat, _, _)| chat["id"].as_str()),
            self.project_archive
                .restore_target
                .as_deref()
                .zip(self.project_archive.restore_error.as_deref()),
            window.viewport_size().width < px(800.),
            cx,
            Self::archive_action,
        )
    }
    pub(super) fn project_archive_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_archive.selected.is_some() {
            self.archive_action(ui::Action::Back, window, cx);
        } else {
            self.close_project_archives();
            window.focus(&self.input.focus_handle(cx), cx);
            cx.notify();
        }
    }
    pub(super) fn project_archive_menu_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (project, position) = self.project_archive.menu.clone()?;
        let focus = self.project_archive.menu_focus.clone()?;
        let opened = project.clone();
        Some(
            deferred(
                anchored().anchor(Anchor::TopLeft).position(position).child(
                    div()
                        .id("project-actions-menu")
                        .role(Role::Menu)
                        .track_focus(&focus)
                        .w(px(196.))
                        .p(px(4.))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(crate::palette::rgb(crate::shell::BORDER))
                        .bg(crate::palette::rgb(crate::shell::RAISED))
                        .shadow_lg()
                        .occlude()
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.project_archive.menu = None;
                            cx.notify();
                        }))
                        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                            match event.keystroke.key.as_str() {
                                "escape" => {
                                    this.project_archive.menu = None;
                                    cx.notify();
                                    cx.stop_propagation();
                                }
                                "enter" | "space" => {
                                    this.open_project_archives(opened.clone(), window, cx);
                                    cx.stop_propagation();
                                }
                                _ => {}
                            }
                        }))
                        .child(
                            div()
                                .id("project-actions-archives")
                                .role(Role::MenuItem)
                                .aria_label("Archived chats")
                                .tab_index(0)
                                .cursor_pointer()
                                .h(px(32.))
                                .px(px(8.))
                                .rounded(px(8.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .text_size(px(13.))
                                .hover(|d| d.bg(crate::settings_style::frame_bg()))
                                .child(crate::assets::icon("project-archive").size(px(14.)))
                                .child("Archived chats")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.open_project_archives(project.clone(), window, cx)
                                })),
                        ),
                ),
            )
            .priority(3)
            .into_any_element(),
        )
    }
}
