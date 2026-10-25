//! Project-scoped archives use a separate read-only transcript state.
use crate::{state::AppState, theme::Theme, transcript::Transcript};
use gpui::{prelude::*, *};
use serde_json::json;
use zuse_protocol::{Chat, methods};
use zuse_viewport::{input::TextInput, project_archive as ui};

pub enum Event {
    Restored(String),
}
pub struct ProjectArchive {
    state: Entity<AppState>,
    project: String,
    search: Entity<TextInput>,
    selected: Option<String>,
    session: Option<String>,
    frozen: Entity<AppState>,
    transcript: Entity<Transcript>,
    reading: bool,
    read_error: Option<String>,
    restore_error: Option<String>,
    restore_target: Option<String>,
    busy: Option<String>,
    revision: u64,
    read: Option<Task<()>>,
    restore: Option<Task<()>>,
    _state: Subscription,
    _search: Subscription,
}
impl EventEmitter<Event> for ProjectArchive {}
impl ProjectArchive {
    pub fn new(
        state: Entity<AppState>,
        project: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| TextInput::new("Search titles, previews or branches…", window, cx));
        let search_sub = cx.observe(&search, |_, _, cx| cx.notify());
        let frozen = cx.new(|_| AppState::new());
        let transcript = cx.new(|cx| Transcript::new(frozen.clone(), cx));
        let observe = cx.observe(&state, |this: &mut Self, _, cx| {
            if this.selected.as_ref().is_some_and(|id| {
                !this.state.read(cx).chats.iter().any(|c| {
                    &c.id == id && c.space_id.as_deref() == Some(&this.project) && c.archived
                })
            }) && this.busy.is_none()
            {
                this.back(cx);
            }
            cx.notify();
        });
        Self {
            state,
            project,
            search,
            selected: None,
            session: None,
            frozen,
            transcript,
            reading: false,
            read_error: None,
            restore_error: None,
            restore_target: None,
            busy: None,
            revision: 0,
            read: None,
            restore: None,
            _state: observe,
            _search: search_sub,
        }
    }
    pub fn on_hidden(&mut self, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        self.read = None;
        self.reading = false;
        cx.notify();
    }
    pub fn on_visible(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.session.clone() {
            self.select_session(id, cx);
        }
    }
    pub fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.selected.is_none() {
            return false;
        }
        self.back(cx);
        true
    }
    fn back(&mut self, cx: &mut Context<Self>) {
        self.revision = self.revision.wrapping_add(1);
        self.read = None;
        self.selected = None;
        self.session = None;
        self.reading = false;
        self.read_error = None;
        cx.notify();
    }
    fn sessions(&self, cx: &App) -> Vec<Chat> {
        let Some(thread) = self.selected.as_deref() else {
            return vec![];
        };
        let mut rows: Vec<_> = self
            .state
            .read(cx)
            .chats
            .iter()
            .filter(|c| c.thread_id() == thread && c.space_id.as_deref() == Some(&self.project))
            .cloned()
            .collect();
        rows.sort_by_key(|c| c.created_at);
        rows
    }
    fn select_session(&mut self, id: String, cx: &mut Context<Self>) {
        let Some(chat) = self.sessions(cx).into_iter().find(|c| c.id == id) else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.read_error =
                Some("Computer unavailable. Reconnect to load this transcript.".into());
            cx.notify();
            return;
        };
        self.revision = self.revision.wrapping_add(1);
        let revision = self.revision;
        self.read = None;
        self.session = Some(id.clone());
        self.reading = true;
        self.read_error = None;
        let sessions = self.sessions(cx);
        self.frozen.update(cx, |state, cx| {
            state.connection = zuse_protocol::view::ConnectionStatus::Ready;
            state.chats = sessions;
            for c in &mut state.chats {
                c.archived = true;
            }
            state.selected_chat = Some(id.clone());
            state.transcript.clear();
            cx.notify();
        });
        self.read = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .subscribe_scoped(
                    methods::WATCH_DOC_MESSAGES,
                    json!({"chatId":id,"targetDeviceId":chat.device_id}),
                )
                .await;
            let mut stream = match result {
                Ok(stream) => stream,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        if this.revision == revision {
                            this.reading = false;
                            this.read_error = Some(error.to_string());
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            while let Some(value) = stream.recv().await {
                let keep = this
                    .update(cx, |this, cx| {
                        if this.revision != revision {
                            return false;
                        }
                        match serde_json::from_value::<zuse_doc::TranscriptUpdate>(value) {
                            Ok(update) => {
                                let result = this.frozen.update(cx, |state, cx| {
                                    state.receive_transcript_update(update, cx)
                                });
                                match result {
                                    Ok(()) => {
                                        this.reading = false;
                                        this.read_error = None;
                                    }
                                    Err(error) => {
                                        this.read_error = Some(format!(
                                            "Could not read saved transcript: {error:?}"
                                        ));
                                        this.reading = false;
                                        cx.notify();
                                        return false;
                                    }
                                }
                            }
                            Err(_) => {
                                this.reading = false;
                                this.read_error = Some(
                                    "Could not read saved transcript. Retry to load it again."
                                        .into(),
                                );
                                cx.notify();
                                return false;
                            }
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| {
                if this.revision == revision && this.reading {
                    this.reading = false;
                    this.read_error =
                        Some("Could not read saved transcript. Retry to load it again.".into());
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }
    fn restore(&mut self, id: String, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let Some(chat) = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|c| {
                c.id == id
                    && c.archived
                    && c.thread_id.is_none()
                    && c.space_id.as_deref() == Some(&self.project)
            })
            .cloned()
        else {
            return;
        };
        self.restore_target = Some(id.clone());
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.restore_error =
                Some("Computer unavailable. Reconnect to restore this chat.".into());
            cx.notify();
            return;
        };
        self.busy = Some(id.clone());
        self.restore_error = None;
        let selection = self.selected.clone();
        self.restore=Some(cx.spawn(async move|this,cx| {
            let result=engine.client().call(methods::MUTATE,json!({"op":"setChatArchived","chatId":id,"archived":false,"targetDeviceId":chat.device_id})).await;
            let _=this.update(cx,|this,cx| {
                this.busy=None;
                match result {Ok(_) if this.selected==selection=>cx.emit(Event::Restored(id)),Ok(_)=>{},Err(error)=>this.restore_error=Some(error.to_string())}
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn action(&mut self, action: ui::Action, _: &mut Window, cx: &mut Context<Self>) {
        match action {
            ui::Action::Open(id) => {
                self.selected = Some(id.clone());
                self.restore_error = None;
                let first = self
                    .sessions(cx)
                    .first()
                    .map(|c| c.id.clone())
                    .unwrap_or(id);
                self.select_session(first, cx);
            }
            ui::Action::Session(id) => self.select_session(id, cx),
            ui::Action::Restore(id) => self.restore(id, cx),
            ui::Action::Back => self.back(cx),
            ui::Action::Retry => {
                if let Some(id) = self.session.clone() {
                    self.select_session(id, cx)
                }
            }
        }
    }
}
impl Render for ProjectArchive {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        zuse_viewport::palette::set_light(Theme::of(cx).appearance.is_light());
        if let Some(thread) = self
            .selected
            .as_ref()
            .and_then(|id| {
                self.state
                    .read(cx)
                    .chats
                    .iter()
                    .find(|c| &c.id == id)
                    .cloned()
            })
            .map(ui::Thread::from_chat)
        {
            let body = if let Some(error) = self.read_error.clone() {
                ui::failure(error, cx, Self::action)
            } else if !self.reading && self.frozen.read(cx).transcript.is_empty() {
                ui::centered("No messages in this session.").into_any_element()
            } else {
                self.transcript.clone().into_any_element()
            };
            return ui::preview(
                &thread,
                &self.sessions(cx),
                self.session.as_deref().unwrap_or_default(),
                body,
                self.reading,
                self.restore_error
                    .as_deref()
                    .filter(|_| self.restore_target.as_deref() == Some(thread.chat.id.as_str())),
                self.busy.is_some(),
                cx,
                Self::action,
            );
        }
        let query = self.search.read(cx).value(cx);
        let state = self.state.read(cx);
        let Some(project) = state.spaces.iter().find(|s| s.id == self.project) else {
            return ui::centered("Select a project to view archived chats.").into_any_element();
        };
        let rows = ui::rows(&state.chats, &self.project, &query);
        let name = project.display_name().to_owned();
        ui::list(
            &name,
            &query,
            self.search.clone().into_any_element(),
            &rows,
            self.busy.as_deref(),
            self.restore_target
                .as_deref()
                .zip(self.restore_error.as_deref()),
            window.viewport_size().width < px(800.),
            cx,
            Self::action,
        )
    }
}
