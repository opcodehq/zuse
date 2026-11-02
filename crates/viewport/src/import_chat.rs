//! Shared recent-provider-chat menu, with host-owned import and ordinary chat navigation.
use crate::localization::message as tr;
use crate::{
    assets::icon,
    creation_style,
    input::TextInput,
    palette, settings_style as s,
    shell::{MUTED, TEXT},
};
use gpui::{prelude::*, *};
use serde_json::{Value, json};
use std::collections::HashMap;
use zuse_i18n::messages as msg;
use zuse_protocol::external_chat::{self as wire, Imported, Row};

gpui::actions!(import_chat_menu, [DismissImportMenu]);
#[derive(Clone, PartialEq, Eq)]
pub struct Target {
    pub device: Option<String>,
    pub owner: String,
}
#[derive(Clone)]
pub struct Request {
    pub id: u64,
    pub method: &'static str,
    pub params: Value,
}
#[derive(Clone)]
pub enum Event {
    Imported(Imported, Target),
    ReturnFocus(Option<Point<Pixels>>),
}
pub struct ImportMenu {
    target: Option<Target>,
    generation: u64,
    next: u64,
    pending: HashMap<u64, (u64, bool)>,
    open: bool,
    loading: bool,
    importing: bool,
    rows: Vec<Row>,
    error: Option<String>,
    attempt: Option<wire::ImportRequest>,
    search: Option<Entity<TextInput>>,
    focus_search: bool,
    focus: FocusHandle,
    active: usize,
    scroll: ScrollHandle,
    bounds: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
}
impl EventEmitter<Request> for ImportMenu {}
impl EventEmitter<Event> for ImportMenu {}
impl ImportMenu {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.bind_keys([KeyBinding::new(
            "escape",
            DismissImportMenu,
            Some("ImportChatMenu"),
        )]);
        Self {
            target: None,
            generation: 0,
            next: 0,
            pending: HashMap::new(),
            open: false,
            loading: false,
            importing: false,
            rows: vec![],
            error: None,
            attempt: None,
            search: None,
            focus_search: false,
            focus: cx.focus_handle().tab_stop(true),
            active: 0,
            scroll: ScrollHandle::new(),
            bounds: Default::default(),
        }
    }
    pub fn set_target(&mut self, target: Option<Target>, cx: &mut Context<Self>) {
        if self.target == target {
            return;
        }
        self.target = target;
        self.generation += 1;
        self.open = false;
        self.rows.clear();
        self.pending.clear();
        self.attempt = None;
        self.loading = false;
        self.importing = false;
        self.error = None;
        self.focus_search = false;
        if let Some(search) = &self.search {
            search.update(cx, |search, cx| search.set_value(String::new(), cx));
        }
        cx.notify();
    }
    pub fn busy(&self) -> bool {
        self.importing
    }
    fn request(
        &mut self,
        method: &'static str,
        mut params: Value,
        import: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = &self.target else {
            return;
        };
        params["targetDeviceId"] = json!(target.device);
        self.next += 1;
        self.pending.insert(self.next, (self.generation, import));
        cx.emit(Request {
            id: self.next,
            method,
            params,
        });
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        self.error = None;
        self.request(wire::LIST, json!({}), false, cx);
        cx.notify();
    }
    pub fn receive(&mut self, id: u64, result: Result<Value, String>, cx: &mut Context<Self>) {
        let Some((generation, import)) = self.pending.remove(&id) else {
            return;
        };
        if generation != self.generation {
            return;
        }
        if import {
            self.importing = false;
            match result.and_then(|v| {
                serde_json::from_value::<Imported>(v)
                    .map_err(|_| "The import response was interrupted. Retry to recover it.".into())
            }) {
                Ok(imported) => {
                    self.attempt = None;
                    self.open = false;
                    self.error = None;
                    if let Some(search) = &self.search {
                        search.update(cx, |search, cx| search.set_value(String::new(), cx));
                    }
                    if let Some(target) = self.target.clone() {
                        cx.emit(Event::Imported(imported, target));
                    }
                    cx.emit(Event::ReturnFocus(None));
                }
                Err(error) => self.error = Some(error),
            }
        } else {
            self.loading = false;
            match result.and_then(|v| {
                serde_json::from_value::<Vec<Row>>(v)
                    .map_err(|_| "Couldn't read provider chats.".into())
            }) {
                Ok(rows) => {
                    self.rows = rows.into_iter().filter(Row::importable).collect();
                    self.error = None;
                }
                Err(error) => self.error = Some(error),
            }
        }
        cx.notify();
    }
    fn filtered(&self, cx: &App) -> Vec<Row> {
        let query = self
            .search
            .as_ref()
            .map(|s| s.read(cx).value(cx))
            .unwrap_or_default();
        self.rows
            .iter()
            .filter(|row| row.matches(&query))
            .cloned()
            .collect()
    }
    fn choose(&mut self, row: Row, cx: &mut Context<Self>) {
        if self.importing {
            return;
        }
        let request = match self.attempt.as_ref().filter(|attempt| attempt.id == row.id) {
            Some(attempt) => attempt.clone(),
            None => wire::ImportRequest {
                id: row.id,
                request_id: uuid::Uuid::new_v4().to_string(),
            },
        };
        self.attempt = Some(request.clone());
        self.importing = true;
        self.error = None;
        self.request(
            wire::IMPORT,
            serde_json::to_value(request).unwrap(),
            true,
            cx,
        );
        cx.notify();
    }
    fn close(&mut self, pointer: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        self.open = false;
        if let Some(search) = &self.search {
            search.update(cx, |s, cx| s.set_value(String::new(), cx));
        }
        cx.emit(Event::ReturnFocus(pointer));
        cx.notify();
    }
    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.close(None, cx);
            return;
        }
        self.open = true;
        self.focus_search = true;
        self.active = 0;
        self.scroll.set_offset(Point::default());
        self.load(cx);
        cx.notify();
    }
    fn key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let rows = self.filtered(cx);
        let count = rows.len();
        match e.keystroke.key.as_str() {
            "escape" => self.close(None, cx),
            "up" if count > 0 => self.active = self.active.saturating_sub(1),
            "down" if count > 0 => self.active = (self.active + 1).min(count - 1),
            "home" if count > 0 => self.active = 0,
            "end" if count > 0 => self.active = count - 1,
            "enter" => {
                if let Some(row) = rows.get(self.active) {
                    self.choose(row.clone(), cx);
                }
            }
            _ => return,
        }
        let row_height = 42.;
        let y = self.active as f32 * row_height;
        let mut offset = self.scroll.offset();
        if -f32::from(offset.y) > y {
            offset.y = px(-y);
        } else if -f32::from(offset.y) + 208. < y + row_height {
            offset.y = px(208. - y - row_height);
        }
        self.scroll.set_offset(offset);
        cx.stop_propagation();
        cx.notify();
    }
}
impl Render for ImportMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.search.is_none() {
            let search = cx.new(|cx| {
                TextInput::new("Search chats…", window, cx).with_single_line_metrics(12., 16.2)
            });
            cx.observe(&search, |this, _, cx| {
                this.active = 0;
                this.scroll.set_offset(Point::default());
                cx.notify();
            })
            .detach();
            self.search = Some(search);
        }
        if self.focus_search {
            self.focus_search = false;
            window.focus(&self.search.as_ref().unwrap().read(cx).focus_handle(cx), cx);
        }
        let measure = self.bounds.clone();
        let mut view = div().relative().flex_none().child(
            creation_style::chip("import-chat-trigger", "Import chat", "chat-import")
                .track_focus(&self.focus)
                .aria_expanded(self.open)
                .text_color(palette::rgb(MUTED))
                .when(self.open, |view| view.bg(s::frame_bg()))
                .child(
                    canvas(move |b, _, _| measure.set(Some(b)), |_, _, _, _| {})
                        .absolute()
                        .left_0()
                        .top_0()
                        .size_full(),
                )
                .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)))
                .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        this.toggle(cx);
                        cx.stop_propagation();
                    }
                })),
        );
        if !self.open {
            return view;
        }
        let bounds = self.bounds.clone();
        let rows = self.filtered(cx);
        let mut list = div()
            .id("import-chat-list")
            .max_h(px(208.))
            .overflow_x_hidden()
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        if self.rows.is_empty() {
            let caption = if self.loading {
                "Finding chats…"
            } else if self.error.is_some() {
                "Couldn't load chats. Close and reopen this menu to retry."
            } else {
                "No recent chats found."
            };
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(8.))
                    .text_size(px(12.))
                    .line_height(px(16.2))
                    .text_color(palette::rgb(MUTED))
                    .child(caption),
            );
        } else if rows.is_empty() {
            let query = self.search.as_ref().unwrap().read(cx).value(cx);
            list = list.child(
                div()
                    .px_2()
                    .py_3()
                    .text_size(px(12.))
                    .child(format!("No chats match “{}”.", query.trim())),
            );
        } else {
            list = list.children(rows.into_iter().enumerate().map(|(index, row)| {
                let provider = match row.provider {
                    zuse_protocol::HarnessId::ClaudeCode => "claude-mark",
                    _ => "codex-mark",
                };
                let project = (!matches!(row.project_name.trim().chars().count(), 0 | 1))
                    .then(|| row.project_name.clone());
                let current = self.attempt.as_ref().is_some_and(|a| a.id == row.id);
                let chosen = row.clone();
                div()
                    .id(("import-chat-row", index))
                    .role(Role::MenuItem)
                    .aria_label(format!("{} · {}", row.title, row.project_path))
                    .min_h(px(32.))
                    .px_2()
                    .py_1()
                    .rounded(px(7.))
                    .flex()
                    .items_start()
                    .gap_2()
                    .cursor_pointer()
                    .text_color(palette::rgb(TEXT))
                    .when(index == self.active, |v| v.bg(s::frame_bg()))
                    .when(self.importing, |v| v.opacity(0.64))
                    .on_hover(cx.listener(move |this, hover, _, cx| {
                        if *hover {
                            this.active = index;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(chosen.clone(), cx)))
                    .child(icon(provider).mx(px(-2.)).mt(px(2.)).size(px(14.)))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11.))
                                    .line_height(px(16.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(row.title),
                            )
                            .when_some(project, |v, p| {
                                v.child(
                                    div()
                                        .truncate()
                                        .text_size(px(9.))
                                        .line_height(px(12.))
                                        .text_color(palette::rgb(MUTED))
                                        .child(p),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .mt(px(2.))
                            .text_size(px(9.))
                            .line_height(px(12.))
                            .text_color(palette::rgb(MUTED))
                            .when(self.importing && current, |view| {
                                view.child(icon("loading").size(px(12.)))
                            })
                            .when(!(self.importing && current), |view| {
                                view.child(relative_time(row.updated_at))
                            }),
                    )
            }));
        }
        let mut card = div()
            .id("import-chat-menu")
            .role(Role::Menu)
            .aria_label(tr(cx, msg::chat::chat_landing_recent_provider_chats))
            .occlude()
            .key_context("ImportChatMenu")
            .w(px(288.).min(window.viewport_size().width - px(16.)))
            .p(px(3.))
            .rounded(px(10.))
            .bg(s::frame_bg())
            .border_1()
            .border_color(s::card_border())
            .shadow_lg()
            .text_color(palette::rgb(TEXT))
            .on_action(cx.listener(|this, _: &DismissImportMenu, _, cx| {
                this.close(None, cx);
                cx.stop_propagation();
            }))
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.key(e, cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                if !bounds.get().is_some_and(|b| b.contains(&e.position)) {
                    this.close(Some(e.position), cx);
                }
            }))
            .child(
                div()
                    .p(px(4.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_2()
                            .pt(px(6.))
                            .pb_1()
                            .text_size(px(11.))
                            .line_height(px(14.85))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette::rgb(MUTED))
                            .child(tr(cx, msg::chat::chat_landing_recent_provider_chats)),
                    )
                    .child(
                        div().px_1().pb(px(6.)).child(
                            div()
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_2()
                                .rounded(px(7.))
                                .border_1()
                                .border_color(s::card_border())
                                .when(
                                    self.search
                                        .as_ref()
                                        .unwrap()
                                        .read(cx)
                                        .focus_handle(cx)
                                        .is_focused(window),
                                    |view| view.border_color(palette::rgb(0x89ca21)),
                                )
                                .bg(palette::tone(
                                    rgba(0x090b0980).into(),
                                    rgba(0xffffff80).into(),
                                ))
                                .child(icon("magnifer").size(px(14.)))
                                .child(self.search.as_ref().unwrap().clone()),
                        ),
                    )
                    .child(list),
            );
        if self.error.is_some() && !self.rows.is_empty() {
            card = card.child(
                div()
                    .px_2()
                    .py_2()
                    .text_size(px(11.))
                    .text_color(palette::rgb(0xef7878))
                    .child(self.error.clone().unwrap()),
            );
        }
        view = view.child(
            div().absolute().bottom_full().left_0().child(deferred(
                anchored()
                    .anchor(Anchor::BottomLeft)
                    .offset(point(px(0.), px(-4.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(card),
            )),
        );
        view
    }
}
fn relative_time(timestamp: i64) -> String {
    let age = (chrono::Utc::now().timestamp_millis() - timestamp).max(0) / 1000;
    if age < 60 {
        "now".into()
    } else if age < 3600 {
        format!("{}m", age / 60)
    } else if age < 86400 {
        format!("{}h", age / 3600)
    } else {
        format!("{}d", age / 86400)
    }
}
